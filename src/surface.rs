//! The e-ink display, reached through the rm2fb client shim.
//!
//! The app must run under `LD_PRELOAD=/opt/lib/librm2fb_client.so.1`: the
//! shim interposes `open("/dev/fb0")` and hands us an mmap-able RGB565
//! shared buffer plus working MXCFB update ioctls, while the rm2display
//! server owns the real panel.

use crate::fb;
use crate::log;
use std::ffi::CString;
use std::io;

/// Half-open pixel rectangle `[x0,x1) x [y0,y1)`, always clamped to screen.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    pub fn full() -> Rect {
        Rect::new(0, 0, fb::WIDTH, fb::HEIGHT)
    }

    pub fn around(x: i32, y: i32, r: i32) -> Rect {
        Rect::new(x - r, y - r, x + r + 1, y + r + 1)
    }

    pub fn include(&mut self, other: Rect) {
        self.x0 = self.x0.min(other.x0);
        self.y0 = self.y0.min(other.y0);
        self.x1 = self.x1.max(other.x1);
        self.y1 = self.y1.max(other.y1);
    }

    pub fn clamped(mut self) -> Rect {
        self.x0 = self.x0.clamp(0, fb::WIDTH);
        self.y0 = self.y0.clamp(0, fb::HEIGHT);
        self.x1 = self.x1.clamp(0, fb::WIDTH);
        self.y1 = self.y1.clamp(0, fb::HEIGHT);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn width(&self) -> i32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> i32 {
        self.y1 - self.y0
    }
}

pub struct Surface {
    fd: libc::c_int,
    map: *mut u16,
    /// 8-bit luma copy of everything we have drawn; the source of truth for
    /// page rasterization and the dissolve animations.
    pub shadow: Vec<u8>,
    dirty: Option<Rect>,
    marker: u32,
    last_sent_marker: u32,
}

impl Surface {
    /// Open and mmap the framebuffer. Fails with a user-actionable message
    /// when the rm2fb client shim is not preloaded.
    pub fn open() -> io::Result<Surface> {
        let path = CString::new("/dev/fb0").unwrap();
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
        if fd < 0 {
            return Err(preload_hint("open(/dev/fb0)"));
        }
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                fb::FB_LEN,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if map == libc::MAP_FAILED {
            let err = preload_hint("mmap(/dev/fb0)");
            unsafe { libc::close(fd) };
            return Err(err);
        }
        log!("framebuffer mapped: {}x{} RGB565", fb::WIDTH, fb::HEIGHT);
        Ok(Surface {
            fd,
            map: map as *mut u16,
            shadow: vec![255u8; (fb::WIDTH * fb::HEIGHT) as usize],
            dirty: None,
            marker: 0,
            last_sent_marker: 0,
        })
    }

    #[inline]
    fn rgb565(gray: u8) -> u16 {
        let v = gray as u16;
        ((v >> 3) << 11) | ((v >> 2) << 5) | (v >> 3)
    }

    /// Paint one pixel of solid gray (0 = black, 255 = white).
    #[inline]
    pub fn set_gray(&mut self, x: i32, y: i32, gray: u8) {
        if x < 0 || y < 0 || x >= fb::WIDTH || y >= fb::HEIGHT {
            return;
        }
        let idx = (y * fb::WIDTH + x) as usize;
        self.shadow[idx] = gray;
        unsafe { *self.map.add(idx) = Self::rgb565(gray) };
    }

    /// Filled circle of gray with radius r.
    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, gray: u8) {
        for dy in -r..=r {
            let half = ((r * r - dy * dy) as f32).sqrt() as i32;
            let y = cy + dy;
            for x in (cx - half)..=(cx + half) {
                self.set_gray(x, y, gray);
            }
        }
    }

    /// Half of a filled circle: only pixels where (x+y) is even/odd matches
    /// `parity`. Two passes with parity 0 then 1 reproduce fill_circle, but
    /// spread over time the ink "blooms" — a speckle first, solid a beat
    /// later — instead of printing crisply.
    pub fn fill_circle_dither(&mut self, cx: i32, cy: i32, r: i32, gray: u8, parity: i32) {
        for dy in -r..=r {
            let half = ((r * r - dy * dy) as f32).sqrt() as i32;
            let y = cy + dy;
            for x in (cx - half)..=(cx + half) {
                if (x + y) & 1 == parity {
                    self.set_gray(x, y, gray);
                }
            }
        }
    }

    pub fn fill_white(&mut self) {
        self.shadow.fill(255);
        unsafe {
            std::ptr::write_bytes(self.map, 0xFF, fb::FB_LEN / 2);
        }
    }

    /// Expand the pending dirty region (flushed later, coalesced).
    pub fn mark_dirty(&mut self, rect: Rect) {
        match &mut self.dirty {
            Some(d) => d.include(rect),
            None => self.dirty = Some(rect),
        }
    }

    /// Send the accumulated dirty region as one partial update.
    pub fn flush_dirty(&mut self, waveform: u32) {
        if let Some(rect) = self.dirty.take() {
            self.send_update(rect, waveform, fb::UPDATE_MODE_PARTIAL);
        }
    }

    /// Push a screen region to the panel. Ioctl failures are logged and
    /// dropped: a lost frame is recoverable, a panic on the device is not.
    pub fn send_update(&mut self, rect: Rect, waveform: u32, mode: u32) {
        let rect = rect.clamped();
        if rect.is_empty() {
            return;
        }
        self.marker = self.marker.wrapping_add(1).max(1);
        let mut data = fb::MxcfbUpdateData {
            update_region: fb::MxcfbRect {
                top: rect.y0 as u32,
                left: rect.x0 as u32,
                width: rect.width() as u32,
                height: rect.height() as u32,
            },
            waveform_mode: waveform,
            update_mode: mode,
            update_marker: self.marker,
            temp: fb::TEMP_USE_AMBIENT,
            flags: 0,
            dither_mode: 0,
            quant_bit: 0,
            alt_buffer_data: fb::MxcfbAltBufferData::default(),
        };
        let rc = unsafe { libc::ioctl(self.fd, fb::MXCFB_SEND_UPDATE, &mut data) };
        if rc < 0 {
            log!("MXCFB_SEND_UPDATE failed: {}", io::Error::last_os_error());
        } else {
            self.last_sent_marker = self.marker;
        }
    }

    /// Block until the panel has consumed everything sent so far.
    pub fn wait_idle(&mut self) {
        if self.last_sent_marker == 0 {
            return;
        }
        let mut data = fb::MxcfbUpdateMarkerData {
            update_marker: self.last_sent_marker,
            collision_test: 0,
        };
        let rc = unsafe { libc::ioctl(self.fd, fb::MXCFB_WAIT_FOR_UPDATE_COMPLETE, &mut data) };
        if rc < 0 {
            log!(
                "MXCFB_WAIT_FOR_UPDATE_COMPLETE failed: {}",
                io::Error::last_os_error()
            );
        }
    }

    /// Blank page, full-quality refresh. Used on startup, after a reply
    /// fades, and before exiting so the panel is left in a sane state.
    pub fn full_white_refresh(&mut self) {
        self.fill_white();
        self.send_update(Rect::full(), fb::WAVEFORM_GC16, fb::UPDATE_MODE_FULL);
        self.wait_idle();
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.map as *mut libc::c_void, fb::FB_LEN);
            libc::close(self.fd);
        }
    }
}

fn preload_hint(what: &str) -> io::Error {
    io::Error::other(format!(
        "{what} failed — the rm2fb client shim is not loaded.\n\
         Run horcrux under the rm2fb client preload, e.g.:\n\
         \x20 LD_PRELOAD=/opt/lib/librm2fb_client.so.1 ./horcrux"
    ))
}
