//! Pen and touch input via evdev.
//!
//! The pen is the "Wacom I2C Digitizer" (located by scanning
//! /sys/class/input/event*/device/name, never hardcoded) and is grabbed
//! exclusively. The touchscreen ("pt_mt") is only watched, un-grabbed, for
//! gestures: five-finger tap quits, single-finger double-tap dismisses the
//! reply early.
//!
//! NOTE: on this 32-bit platform `struct input_event` is 16 bytes
//! (timeval = 2 x i32, then u16 type, u16 code, i32 value) — not 24.

use crate::log;
use std::ffi::CString;
use std::io;
use std::time::{Duration, Instant};

// evdev constants (linux/input-event-codes.h)
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const SYN_REPORT: u16 = 0;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_MT_SLOT: u16 = 0x2F;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const BTN_TOOL_PEN: u16 = 0x140;
const BTN_TOOL_RUBBER: u16 = 0x141;
const BTN_TOUCH: u16 = 0x14A;

/// EVIOCGRAB = _IOW('E', 0x90, int)
const EVIOCGRAB: libc::c_ulong = 0x4004_4590;

const INPUT_EVENT_LEN: usize = 16; // 32-bit struct input_event

// Pen digitizer ranges (rM2).
const PEN_MAX_X: i32 = 20966; // long axis  -> screen Y
const PEN_MAX_Y: i32 = 15725; // short axis -> screen X

/// Five contacts within this window count as the quit gesture.
const QUIT_WINDOW: Duration = Duration::from_millis(1000);
const QUIT_FINGERS: usize = 5;

/// Single-finger tap/double-tap timing (dismiss the reply early).
const TAP_MAX: Duration = Duration::from_millis(300);
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug)]
pub struct PenSample {
    pub x: i32,
    pub y: i32,
    pub pressure: i32,
    pub touching: bool,
    pub eraser: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum InputEvent {
    Pen(PenSample),
    /// Five-finger tap on the touchscreen.
    Quit,
    /// Two quick single-finger taps on the touchscreen.
    DoubleTap,
}

#[derive(Default, Clone, Copy)]
struct PenState {
    raw_x: i32,
    raw_y: i32,
    pressure: i32,
    touching: bool,
    eraser: bool,
    changed: bool,
}

pub struct Input {
    pen_fd: libc::c_int,
    touch_fd: libc::c_int, // -1 when the touchscreen was not found
    pen: PenState,
    // touch tracking for the quit gesture
    slots: [i32; 8], // tracking id per slot, -1 = up
    cur_slot: usize,
    burst_start: Option<Instant>,
    quit_latched: bool,
    // single-finger tap tracking for the double-tap gesture
    tap_down: Option<Instant>,
    last_tap: Option<Instant>,
}

impl Input {
    pub fn open() -> io::Result<Input> {
        let pen_path = find_device("Wacom I2C Digitizer").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "pen digitizer 'Wacom I2C Digitizer' not found under /sys/class/input",
            )
        })?;
        let pen_fd = open_nonblock(&pen_path)?;
        // Grab the pen exclusively; if the grab fails we still read events
        // (xochitl is stopped by the launcher, so contention is unlikely).
        if unsafe { libc::ioctl(pen_fd, EVIOCGRAB, 1) } < 0 {
            log!(
                "warning: EVIOCGRAB on pen failed: {}",
                io::Error::last_os_error()
            );
        }
        log!("pen: {pen_path}");

        let touch_fd = match find_device("pt_mt") {
            Some(p) => match open_nonblock(&p) {
                Ok(fd) => {
                    log!("touch: {p}");
                    fd
                }
                Err(e) => {
                    log!("warning: cannot open {p}: {e} (quit gesture disabled)");
                    -1
                }
            },
            None => {
                log!("warning: touchscreen 'pt_mt' not found (quit gesture disabled)");
                -1
            }
        };

        Ok(Input {
            pen_fd,
            touch_fd,
            pen: PenState::default(),
            slots: [-1; 8],
            cur_slot: 0,
            burst_start: None,
            quit_latched: false,
            tap_down: None,
            last_tap: None,
        })
    }

    /// Drain both devices, returning at most one event batch.
    pub fn poll(&mut self) -> Vec<InputEvent> {
        let mut out = Vec::new();
        self.drain_pen(&mut out);
        self.drain_touch(&mut out);
        out
    }

    /// Block until pen/touch input is ready or `timeout` elapses, whichever
    /// comes first. Returns true when a device signalled readiness (events
    /// are then available for `poll()` to drain). The fds stay `O_NONBLOCK`,
    /// so this is what lets the main loop sleep deeply while idle instead of
    /// busy-polling at 500 Hz — the process truly blocks in the kernel until
    /// the pen moves or the deadline is due.
    pub fn wait(&self, timeout: Duration) -> bool {
        let mut pfds: [libc::pollfd; 2] = [
            libc::pollfd {
                fd: self.pen_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.touch_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let n = if self.touch_fd >= 0 { 2 } else { 1 };
        let ms = timeout.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
        let rc = unsafe { libc::poll(pfds.as_mut_ptr(), n as libc::nfds_t, ms) };
        rc > 0
    }

    fn drain_pen(&mut self, out: &mut Vec<InputEvent>) {
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe {
                libc::read(
                    self.pen_fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n <= 0 {
                break; // EAGAIN (empty) or error — either way, stop
            }
            for chunk in buf[..n as usize].chunks_exact(INPUT_EVENT_LEN) {
                let (ty, code, value) = parse_input_event(chunk);
                match ty {
                    EV_ABS => match code {
                        ABS_X => {
                            self.pen.raw_x = value;
                            self.pen.changed = true;
                        }
                        ABS_Y => {
                            self.pen.raw_y = value;
                            self.pen.changed = true;
                        }
                        0x18 => {
                            // ABS_PRESSURE
                            self.pen.pressure = value;
                            self.pen.changed = true;
                        }
                        _ => {} // ABS_DISTANCE, ABS_TILT_* — unused
                    },
                    EV_KEY => match code {
                        BTN_TOUCH => {
                            self.pen.touching = value != 0;
                            self.pen.changed = true;
                        }
                        BTN_TOOL_RUBBER if value != 0 => {
                            self.pen.eraser = true;
                            self.pen.changed = true;
                        }
                        BTN_TOOL_PEN if value != 0 => {
                            self.pen.eraser = false;
                            self.pen.changed = true;
                        }
                        _ => {} // BTN_STYLUS, BTN_STYLUS2 — unused
                    },
                    EV_SYN if code == SYN_REPORT && self.pen.changed => {
                        self.pen.changed = false;
                        out.push(InputEvent::Pen(self.transform()));
                    }
                    _ => {}
                }
            }
        }
    }

    /// Map digitizer coordinates to the 1404x1872 portrait screen.
    fn transform(&self) -> PenSample {
        let p = &self.pen;
        PenSample {
            x: p.raw_y * crate::fb::WIDTH / PEN_MAX_Y,
            y: crate::fb::HEIGHT - (p.raw_x * crate::fb::HEIGHT / PEN_MAX_X),
            pressure: p.pressure,
            touching: p.touching,
            eraser: p.eraser,
        }
    }

    fn drain_touch(&mut self, out: &mut Vec<InputEvent>) {
        if self.touch_fd < 0 || self.quit_latched {
            return;
        }
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe {
                libc::read(
                    self.touch_fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n <= 0 {
                break;
            }
            for chunk in buf[..n as usize].chunks_exact(INPUT_EVENT_LEN) {
                let (ty, code, value) = parse_input_event(chunk);
                if ty != EV_ABS {
                    continue;
                }
                match code {
                    ABS_MT_SLOT => {
                        self.cur_slot = (value as usize).min(self.slots.len() - 1);
                    }
                    ABS_MT_TRACKING_ID => {
                        let was = self.active_contacts();
                        self.slots[self.cur_slot] = value; // -1 lifts the contact
                        let now = self.active_contacts();
                        if now > was {
                            self.note_contact_down(now, out);
                        } else if now < was {
                            self.note_contact_up(was, now, out);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn active_contacts(&self) -> usize {
        self.slots.iter().filter(|&&id| id >= 0).count()
    }

    fn note_contact_down(&mut self, active: usize, out: &mut Vec<InputEvent>) {
        if active == 1 {
            self.burst_start = Some(Instant::now());
            self.tap_down = Some(Instant::now());
        } else {
            // multi-finger touch is never a tap
            self.tap_down = None;
        }
        if active >= QUIT_FINGERS {
            let fresh = self
                .burst_start
                .map(|t| t.elapsed() <= QUIT_WINDOW)
                .unwrap_or(true);
            if fresh && !self.quit_latched {
                self.quit_latched = true;
                log!("quit gesture: {active} simultaneous contacts");
                out.push(InputEvent::Quit);
            }
        }
    }

    fn note_contact_up(&mut self, was: usize, now: usize, out: &mut Vec<InputEvent>) {
        // Only a lone finger lifting off (1 -> 0) can complete a tap.
        if was != 1 || now != 0 {
            return;
        }
        let Some(down) = self.tap_down.take() else {
            return;
        };
        if down.elapsed() > TAP_MAX {
            self.last_tap = None;
            return;
        }
        if self
            .last_tap
            .map(|t| t.elapsed() <= DOUBLE_TAP_WINDOW)
            .unwrap_or(false)
        {
            self.last_tap = None;
            log!("double-tap gesture");
            out.push(InputEvent::DoubleTap);
        } else {
            self.last_tap = Some(Instant::now());
        }
    }

    /// Release the pen grab (idempotent).
    pub fn ungrab(&mut self) {
        if self.pen_fd >= 0 {
            unsafe { libc::ioctl(self.pen_fd, EVIOCGRAB, 0) };
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.ungrab();
        unsafe {
            if self.pen_fd >= 0 {
                libc::close(self.pen_fd);
            }
            if self.touch_fd >= 0 {
                libc::close(self.touch_fd);
            }
        }
    }
}

/// (type, code, value) out of a 16-byte 32-bit `struct input_event`.
fn parse_input_event(chunk: &[u8]) -> (u16, u16, i32) {
    let ty = u16::from_le_bytes([chunk[8], chunk[9]]);
    let code = u16::from_le_bytes([chunk[10], chunk[11]]);
    let value = i32::from_le_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]);
    (ty, code, value)
}

/// Find /dev/input/eventN whose device name contains `needle`.
fn find_device(needle: &str) -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/input").ok()?;
    let mut names: Vec<_> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("event"))
        .collect();
    names.sort();
    for name in names {
        let name_file = format!("/sys/class/input/{name}/device/name");
        if let Ok(dev_name) = std::fs::read_to_string(&name_file) {
            if dev_name.trim().contains(needle) {
                return Some(format!("/dev/input/{name}"));
            }
        }
    }
    None
}

fn open_nonblock(path: &str) -> io::Result<libc::c_int> {
    let c_path = CString::new(path).unwrap();
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}
