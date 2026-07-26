//! reMarkable 2 panel geometry and i.MX EPDC (mxcfb) ioctl interface.
//!
//! The struct layouts and ioctl numbers below mirror the i.MX
//! `linux/mxcfb.h` uapi header. They were cross-checked against two
//! independent copies of that header:
//!
//!   * FBInk `eink/mxcfb-remarkable.h`, itself taken from the reMarkable
//!     kernel tree (<https://github.com/NiLuJe/FBInk>)
//!   * rmkit `src/rmkit/fb/mxcfb.h` (<https://github.com/rmkit-dev/rmkit>)
//!
//! Both agree with the reMarkable kernel (branch zero-gravitas,
//! `include/uapi/linux/mxcfb.h`).

use core::mem::size_of;

/// Visible geometry of the rM2 panel in portrait orientation.
pub const WIDTH: i32 = 1404;
pub const HEIGHT: i32 = 1872;
/// RGB565: two bytes per pixel, tightly packed lines.
pub const LINE_LENGTH: usize = 2808;
pub const FB_LEN: usize = LINE_LENGTH * (HEIGHT as usize);

// Waveform modes (linux/mxcfb.h, values per the rM <epframebuffer.h>).
pub const WAVEFORM_DU: u32 = 1; // fast bilevel — live pen ink
pub const WAVEFORM_GC16: u32 = 2; // flashing, full-quality 16-level grayscale
#[allow(dead_code)] // documented for completeness; DU/GC16 cover the app's needs
pub const WAVEFORM_GL16: u32 = 3; // fast 16-level grayscale

pub const UPDATE_MODE_PARTIAL: u32 = 0;
pub const UPDATE_MODE_FULL: u32 = 1;

/// "Use ambient temperature" sentinel for `MxcfbUpdateData::temp`.
pub const TEMP_USE_AMBIENT: i32 = 0x1000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MxcfbRect {
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MxcfbAltBufferData {
    pub phys_addr: u32,
    pub width: u32,                   // width of entire buffer
    pub height: u32,                  // height of entire buffer
    pub alt_update_region: MxcfbRect, // region within buffer to update
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MxcfbUpdateData {
    pub update_region: MxcfbRect,
    pub waveform_mode: u32,
    pub update_mode: u32,
    pub update_marker: u32,
    pub temp: i32,
    pub flags: u32,
    pub dither_mode: i32,
    pub quant_bit: i32,
    pub alt_buffer_data: MxcfbAltBufferData,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MxcfbUpdateMarkerData {
    pub update_marker: u32,
    pub collision_test: u32,
}

// --- ioctl request numbers --------------------------------------------------
// Computed exactly like the kernel's _IOW/_IOWR macros (asm-generic/ioctl.h):
// the payload size is encoded in the request number, so mirroring the struct
// layout precisely matters.

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(dir: u32, ty: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (ty << 8) | nr | (size << 16)
}

const fn iow<T>(ty: u32, nr: u32) -> u32 {
    ioc(IOC_WRITE, ty, nr, size_of::<T>() as u32)
}

const fn iowr<T>(ty: u32, nr: u32) -> u32 {
    ioc(IOC_READ | IOC_WRITE, ty, nr, size_of::<T>() as u32)
}

pub const MXCFB_SEND_UPDATE: libc::c_ulong =
    iow::<MxcfbUpdateData>(b'F' as u32, 0x2E) as libc::c_ulong;
pub const MXCFB_WAIT_FOR_UPDATE_COMPLETE: libc::c_ulong =
    iowr::<MxcfbUpdateMarkerData>(b'F' as u32, 0x2F) as libc::c_ulong;

// Layout sanity checks. On the 32-bit ARM ABI these must be 72 and 8 bytes,
// giving the well-known request numbers 0x4048462E and 0xC008462F.
const _: () = assert!(size_of::<MxcfbUpdateData>() == 72);
const _: () = assert!(size_of::<MxcfbUpdateMarkerData>() == 8);
const _: () = assert!(MXCFB_SEND_UPDATE == 0x4048_462E_u64 as libc::c_ulong);
const _: () = assert!(MXCFB_WAIT_FOR_UPDATE_COMPLETE == 0xC008_462F_u64 as libc::c_ulong);
