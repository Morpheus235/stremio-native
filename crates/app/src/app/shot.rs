//! **The simulator's screenshot** (`hostsim` only): `shot` in the `memcmd` trigger marks the next
//! frame; after it is drawn and before the swap its pixels are read back and written as
//! `shot-<n>.png` in the instance's runtime root, numbered so a sequence never overwrites itself.
//! Opaque RGB on purpose: the UI plane's alpha is for the television's video plane, and a viewer
//! re-interpreting it would bloom the whole interface over a white page.

use std::os::raw::{c_int, c_uint, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use sn_base::eventlog::log;

const GL_RGB: c_uint = 0x1907;
const GL_UNSIGNED_BYTE: c_uint = 0x1401;
const GL_PACK_ALIGNMENT: c_uint = 0x0D05;

extern "C" {
    fn glReadPixels(x: c_int, y: c_int, w: c_int, h: c_int, format: c_uint, ty: c_uint, data: *mut c_void);
    fn glPixelStorei(pname: c_uint, param: c_int);
}

static ASKED: AtomicBool = AtomicBool::new(false);
static TAKEN: AtomicU32 = AtomicU32::new(0);

/// Mark the next frame for capture, and make sure there is one (a settled screen stops presenting).
pub fn request() {
    ASKED.store(true, Ordering::Relaxed);
    sn_machine::idle::invalidate();
}

/// Write the frame just drawn if one was asked for. The main thread, after the draw, before the swap.
pub fn take_if_requested() {
    if !ASKED.swap(false, Ordering::Relaxed) {
        return;
    }
    let (x, y, w, h) = sn_base::surface::viewport();
    if w <= 0 || h <= 0 {
        return;
    }
    let row = w as usize * 3;
    let mut px = vec![0u8; row * h as usize];
    // SAFETY: the GL context is current on this thread; `px` holds w*h tightly packed RGB pixels.
    unsafe {
        glPixelStorei(GL_PACK_ALIGNMENT, 1);
        glReadPixels(x, y, w, h, GL_RGB, GL_UNSIGNED_BYTE, px.as_mut_ptr().cast());
    }
    // GL's rows run bottom-up
    let flipped: Vec<u8> = px.chunks_exact(row).rev().flatten().copied().collect();
    let n = TAKEN.fetch_add(1, Ordering::Relaxed) + 1;
    let path = sn_base::paths::in_runtime_dir(&format!("shot-{n}.png"));
    match image::RgbImage::from_raw(w as u32, h as u32, flipped).map(|img| img.save(&path)) {
        Some(Ok(())) => log(&format!("shot: {}", path.display())),
        _ => log(&format!("shot: could not write {}", path.display())),
    }
}
