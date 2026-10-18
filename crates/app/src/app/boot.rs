//! **Boot**: the SDL window and its GLES2 context, the renderer, the transport, the locale, the
//! container tree on Home, the artwork source and the media layer — in the order each one's
//! precondition demands — and the matching shutdown.
//!
//! Order that matters, each for a stated reason:
//! - `SDL_SetHint`s before `SDL_Init`, which reads them;
//! - the GL attributes before the window: ES 2.0, RGBA8 (the alpha is how the video plane shows
//!   through), and NO depth or stencil — nothing here uses either, and on a tiler a granted depth
//!   buffer is a per-frame write-back nobody reads;
//! - `surface::probe` right after the context, before anything sizes against the drawable;
//! - the window bind and the Wayland grab before the first frame (the grab clears the opaque
//!   region the video plane needs);
//! - `net::set_user_agent` before `net::global_init`, so the first request already carries it;
//! - `ff::boot` after `global_init`, whose `curl_global_init` must be the main thread's.

use std::os::raw::{c_char, c_int, c_uint, c_void};

use sn_base::eventlog::log;
use sn_base::task::MainThread;
use sn_screens::registry::AppArg;

use super::bridge::Bridge;
use super::playback::Playback;
use super::{App, SCR_H, SCR_W};
use crate::art::{Art, Workers};

const SDL_INIT_VIDEO: u32 = 0x20;
/// `SDL_WINDOW_OPENGL | SDL_WINDOW_FULLSCREEN`: the television owns the whole panel.
const SDL_WINDOW_FLAGS: u32 = 0x2 | 0x1;
const GL_RENDERER: c_uint = 0x1F01;
const GL_VERSION: c_uint = 0x1F02;
// SDL_GLattr
const A_RED: c_int = 0;
const A_GREEN: c_int = 1;
const A_BLUE: c_int = 2;
const A_ALPHA: c_int = 3;
const A_BUFFER_SIZE: c_int = 4;
const A_DEPTH: c_int = 6;
const A_STENCIL: c_int = 7;
const A_CTX_MAJOR: c_int = 17;
const A_CTX_MINOR: c_int = 18;
const A_CTX_PROFILE_MASK: c_int = 21;
const CTX_PROFILE_ES: c_int = 0x0004;

extern "C" {
    fn SDL_SetMainReady();
    fn SDL_SetHint(name: *const c_char, value: *const c_char) -> c_int;
    fn SDL_Init(flags: u32) -> c_int;
    fn SDL_Quit();
    fn SDL_GL_SetAttribute(attr: c_int, value: c_int) -> c_int;
    fn SDL_CreateWindow(title: *const c_char, x: c_int, y: c_int, w: c_int, h: c_int, flags: u32) -> *mut c_void;
    fn SDL_GL_CreateContext(win: *mut c_void) -> *mut c_void;
    fn SDL_GL_SetSwapInterval(interval: c_int) -> c_int;
    fn glGetString(name: c_uint) -> *const c_char;
}

/// Build the application. `Err` is the process exit status: there is no window to show anything in.
///
/// # Safety
/// The SDL main thread, once.
pub unsafe fn construct(mt: MainThread) -> Result<App, c_int> {
    SDL_SetMainReady();
    // A foreground native app is never blanked by webOS; the hint states the intent and costs nothing.
    SDL_SetHint(c"SDL_VIDEO_ALLOW_SCREENSAVER".as_ptr(), c"0".as_ptr());
    if SDL_Init(SDL_INIT_VIDEO) != 0 {
        log("SDL_Init failed");
        return Err(1);
    }
    SDL_GL_SetAttribute(A_CTX_PROFILE_MASK, CTX_PROFILE_ES);
    SDL_GL_SetAttribute(A_CTX_MAJOR, 2);
    SDL_GL_SetAttribute(A_CTX_MINOR, 0);
    SDL_GL_SetAttribute(A_RED, 8);
    SDL_GL_SetAttribute(A_GREEN, 8);
    SDL_GL_SetAttribute(A_BLUE, 8);
    SDL_GL_SetAttribute(A_ALPHA, 8);
    SDL_GL_SetAttribute(A_BUFFER_SIZE, 32);
    SDL_GL_SetAttribute(A_DEPTH, 0);
    SDL_GL_SetAttribute(A_STENCIL, 0);
    let win = SDL_CreateWindow(c"stremionative".as_ptr(), 0, 0, SCR_W, SCR_H, SDL_WINDOW_FLAGS);
    if win.is_null() {
        log("CreateWindow failed");
        return Err(1);
    }
    if SDL_GL_CreateContext(win).is_null() {
        log("GL ctx failed");
        return Err(1);
    }
    sn_base::surface::probe(win);
    // vsync: the frame rate locks to the panel refresh.
    SDL_GL_SetSwapInterval(1);
    {
        let r = glGetString(GL_RENDERER);
        let v = glGetString(GL_VERSION);
        if !r.is_null() && !v.is_null() {
            log(&format!(
                "GL: {} / {}",
                std::ffi::CStr::from_ptr(r).to_string_lossy(),
                std::ffi::CStr::from_ptr(v).to_string_lossy()
            ));
        }
    }
    sn_gfx::egl::probe();
    sn_platform::tv::window::bind_window(win);
    // the television keyboard reads the window's input-focus flag (`textinput`'s trap 2)
    crate::textinput::bind(win);
    sn_platform::tv::window::grab(win);
    sn_platform::tv::window::arm_opaque_region();
    sn_gfx::gfx::init_gl();
    sn_gfx::text::init_text();
    sn_gfx::gfx::init_image();
    sn_gfx::gfx::init_blur();

    // Which television this is, and what it decodes: read once, off the critical path where the
    // platform allows it (the capability probe answers asynchronously).
    sn_platform::tv::probe_device();
    sn_platform::tv::start_capability_probe();

    if sn_net::net::set_user_agent(crate::user_agent()).is_err() {
        log("net: the User-Agent was refused; requests go without one");
    }
    let _ = sn_net::net::global_init();
    // the interface language the viewer chose (Settings > Interface), else the TV's
    sn_platform::i18n::initialize(super::prefs::language(&super::prefs::path()), false);

    let art = Art::new(Box::new(Workers)).install();
    let mut pages = sn_ui::dispatch::Dispatcher::with_transition(Box::new(sn_ui::containers::transition::PageDip::new()));
    pages.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::Root(AppArg::Home));

    sn_media::player::acb_init(&mt);
    sn_media::ff::boot();
    let playback = Playback::new(mt);
    sn_screens::settings::set_version(crate::VERSION);
    // The viewer's settings, read once before the loop (a frame never waits on storage).
    let settings_path = super::prefs::path();
    let loaded = super::prefs::load(&settings_path);
    // The default addons (Cinemeta) until the stored session says otherwise: their manifests are
    // the first requests, beside the session's read and the account's addons once it is known.
    let mut bridge = Bridge::new(
        sn_screens::stores::catalog::CatalogStore::signed_out(),
        playback,
        Box::new(sn_gfx::text::TtfMeasure),
        Some(art),
    )
    .with_settings(loaded, settings_path);
    bridge.boot();
    log("boot: Home requested");
    // A debug build armed with `/tmp/stremionative-playurl` opens the player on that stream.
    #[cfg(feature = "devtriggers")]
    match sn_media::player::playurl::playurl().map(|p| p.and_then(|p| p.to_start().map_err(|e| format!("{e:?}")))) {
        Some(Ok(start)) => {
            bridge.playback.start_dev(start);
            let arg = sn_screens::registry::PlayerArg { title: "playurl".into(), item: None, addon: String::new(), resumed_ms: 0 };
            pages.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::Push(AppArg::Player(arg)));
            log("boot: playurl trigger — player opened");
        }
        Some(Err(e)) => log(&format!("boot: playurl trigger unreadable ({e}) — ignored")),
        None => {}
    }
    let glass = sn_ui::frame::glass::GlassPlan::new();
    Ok(App { win, pages, bridge, glass, running: true, backgrounded: false, ev: [0; 64], pointer: Default::default(), diagnostics: Default::default() })
}

/// Leave cleanly: end a playback (the Starfish pipeline must be unloaded, not abandoned), free the
/// render cache, close SDL.
pub fn shutdown(app: &mut App) {
    app.bridge.playback.stop();
    // the stop's Unload runs off-frame: never exit inside it, nor with its objects undestroyed
    app.bridge.playback.adapter.finish_release();
    sn_ui::tex::shutdown(&mut super::bridge::GfxUploader);
    log("shutdown");
    // SAFETY: the main thread, after the last frame.
    unsafe { SDL_Quit() };
}
