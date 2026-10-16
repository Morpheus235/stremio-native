//! **The application**: what [`run_application`] builds ([`boot`]), the loop that runs it
//! ([`run`]), and the parts the loop drives.
//!
//! One owner per state. [`App`] holds the SDL window, the dispatcher — which owns navigation, the
//! focus engine, the press machine and the present gate (`sn_ui::dispatch::Dispatcher`, the
//! `InputMachine` inside it) — and the [`bridge::Bridge`], which owns the stores the screens read,
//! the workers that fill them and the playback. Nothing else holds application state; there is no
//! `static` page stack or route word.

use std::os::raw::{c_int, c_void};

use sn_base::eventlog::log;

pub mod boot;
pub mod bridge;
pub mod input;
pub mod playback;
pub mod run;

/// The window asked for: the authored canvas, which a television's panel is.
pub(crate) const SCR_W: c_int = sn_base::surface::LOGICAL_W as c_int;
pub(crate) const SCR_H: c_int = sn_base::surface::LOGICAL_H as c_int;

pub struct App {
    /// The SDL window, for the swap and the foreground re-grab.
    pub win: *mut c_void,
    /// The container tree and its machines (§2.2): navigation, input, the present gate.
    pub pages: sn_ui::dispatch::Dispatcher<sn_screens::registry::Bundle>,
    pub bridge: bridge::Bridge,
    pub running: bool,
    /// Between a WILL/DID background and the next foreground: nothing is presented.
    pub backgrounded: bool,
    /// The SDL event buffer (`SDL_Event` is 56 bytes; the TV's fork writes no more).
    pub ev: [u8; 64],
}

/// Boot, run, shut down. The process exit status.
pub fn run_application() -> c_int {
    crate::crash::install_panic_logger();
    log(&format!("StremioNative {} starting", crate::VERSION));
    // SAFETY: `sn_run` is called once, by `main`, on the SDL main thread.
    let mt = unsafe { sn_base::task::MainThread::assume() };
    // SAFETY: the SDL/GL bring-up runs on the main thread before any frame.
    let mut app = match unsafe { boot::construct(mt) } {
        Ok(app) => app,
        Err(code) => return code,
    };
    // SAFETY: the loop runs on the thread that created the window and the GL context.
    unsafe { run::run(&mut app) };
    boot::shutdown(&mut app);
    0
}
