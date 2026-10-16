//! webOS/SDL platform glue: the Wayland surface the video plane shows through, and the luna bus's
//! glib main context.
//!
//! [`sys_grab_wayland`] borrows SDL's `wl_surface` and clears its opaque region so the Starfish
//! video plane below shows through. **The TV's SDL fork writes a LARGER `SDL_SysWMinfo` than the
//! headers declare**, so `SDL_GetWindowWMInfo` is handed a generous over-allocated buffer (a bare
//! struct there is a stack smash). `wl_surface` opcode 4 is `set_opaque_region`.
//!
//! Ported from PlxNative's `system.rs` without its two developer experiments (the opaque-region
//! claim and the compositor frame-callback probe): the port's `arm_opaque_region` /
//! `opaque_route` / `frame_probe_*` entries are no-ops here, and the surface stays non-opaque for
//! the whole run, which is the shipping behaviour those experiments left untouched.

use std::os::raw::{c_int, c_void};

use sn_base::eventlog::log;

extern "C" {
    /// Fills `SDL_version`: three `Uint8`, major/minor/patch.
    fn SDL_GetVersion(ver: *mut u8);
}

// Only a television has these: the Wayland marshaller and glib's main context. The host suite
// links neither, and reaches none of the calls (the surface pointer is null there).
#[cfg(not(test))]
extern "C" {
    fn SDL_GetWindowWMInfo(window: *mut c_void, info: *mut c_void) -> c_int;
    fn wl_proxy_marshal(proxy: *mut c_void, opcode: u32, ...);
    fn g_main_context_pending(ctx: *mut c_void) -> c_int;
    fn g_main_context_iteration(ctx: *mut c_void, may_block: c_int) -> c_int;
}

/// `SDL_SYSWM_WAYLAND` in SDL2's `SDL_SYSWM_TYPE`.
const SDL_SYSWM_WAYLAND: i32 = 6;
/// `wl_surface.set_opaque_region`.
const WL_SURFACE_SET_OPAQUE_REGION: u32 = 4;

static mut G_WL_SURFACE: *mut c_void = std::ptr::null_mut();
static mut G_WL_DISPLAY: *mut c_void = std::ptr::null_mut();

/// A port entry with nothing to do on this build.
pub(crate) fn nothing() {}

/// `set_opaque_region(NULL)`: the whole surface may be blended, so the video plane under it shows.
/// The commit is left to `SDL_GL_SwapWindow` — a bare commit here would present a null-buffer
/// surface and disrupt the slaved video plane.
pub(crate) fn clear_opaque_region() {
    // SAFETY: the frame thread is the only reader and writer of the borrowed handles.
    let surface = unsafe { G_WL_SURFACE };
    if surface.is_null() {
        return;
    }
    #[cfg(not(test))]
    // SAFETY: `surface` is SDL's live wl_surface (revoked on background before SDL may drop it);
    // opcode 4 takes one nullable `wl_region`.
    unsafe {
        wl_proxy_marshal(surface, WL_SURFACE_SET_OPAQUE_REGION, std::ptr::null_mut::<c_void>());
    }
    #[cfg(test)]
    let _ = WL_SURFACE_SET_OPAQUE_REGION;
}

/// No opaque-region experiment on this build: the surface stays non-opaque.
pub(crate) fn opaque_region_init() {}

/// See [`opaque_region_init`].
pub(crate) fn opaque_route(_video_plane_bound: bool) {}

/// Null the Wayland surface and display pointers on app background. SDL owns these client-side
/// proxies; our references must not outlive an active window, so a background revokes the borrow
/// before any later frame can marshal through it. Foreground re-acquires them. Answers whether a
/// surface was held.
pub(crate) fn sys_release_wayland() -> bool {
    // SAFETY: frame thread only (see `clear_opaque_region`).
    unsafe {
        let had = !G_WL_SURFACE.is_null();
        G_WL_SURFACE = std::ptr::null_mut();
        G_WL_DISPLAY = std::ptr::null_mut();
        if had {
            log("wm: released borrowed Wayland handles");
        }
        had
    }
}

/// Service the glib main context luna-service2 replies arrive on, at most eight dispatches a frame.
pub(crate) fn ls2_pump() {
    #[cfg(not(test))]
    // SAFETY: the default main context, on the thread that owns it.
    unsafe {
        let mut guard = 8;
        while guard > 0 && g_main_context_pending(std::ptr::null_mut()) != 0 {
            g_main_context_iteration(std::ptr::null_mut(), 0);
            guard -= 1;
        }
    }
}

/// Borrow SDL's Wayland handles for `winp` and clear the surface's opaque region.
pub(crate) fn sys_grab_wayland(winp: *mut c_void) {
    let mut wmbuf = [0u8; 512];
    // ASK THE LIBRARY which `SDL_SysWMinfo` layout to declare, never hardcode it: from 2.0.6 on SDL
    // refuses a caller version below 2.0.6 WITHOUT filling the union, which leaves the surface
    // null, the UI plane opaque, and the video decoding invisibly beneath it. webOS 4.x ships
    // 2.0.4, 5.x/6.x 2.0.10, 7.x+ 2.0.14; the union's first two members (display, surface) never
    // moved, so the runtime version is safe in every direction.
    // SAFETY: SDL writes three bytes into the buffer's head.
    unsafe { SDL_GetVersion(wmbuf.as_mut_ptr()) };
    #[cfg(not(test))]
    // SAFETY: an over-allocated, writable buffer for SDL's struct (see the module doc).
    let ok = unsafe { SDL_GetWindowWMInfo(winp, wmbuf.as_mut_ptr().cast()) };
    #[cfg(test)]
    let ok = {
        let _ = winp;
        0
    };
    // SAFETY: frame thread only.
    unsafe { update_wayland_info(ok, &wmbuf) };
    let subsystem = i32::from_ne_bytes([wmbuf[4], wmbuf[5], wmbuf[6], wmbuf[7]]);
    // SAFETY: frame thread only.
    let (surf, disp) = unsafe { (G_WL_SURFACE, G_WL_DISPLAY) };
    log(&format!(
        "wm sdl={}.{}.{} subsys={subsystem} wl_surface={surf:p} wl_display={disp:p}",
        wmbuf[0], wmbuf[1], wmbuf[2]
    ));
    if surf.is_null() {
        log("wm: NO wl_surface — the UI plane cannot be made transparent, so video would decode invisibly beneath it");
    }
    clear_opaque_region();
}

/// Publish SDL's borrowed handles from a filled `SDL_SysWMinfo`. Apart from the native query so a
/// failed or foreign answer is host-testable.
///
/// # Safety
/// Frame thread only: it writes the handle statics.
unsafe fn update_wayland_info(ok: c_int, info: &[u8; 512]) {
    sys_release_wayland();
    let subsystem = i32::from_ne_bytes([info[4], info[5], info[6], info[7]]);
    if ok == 0 || subsystem != SDL_SYSWM_WAYLAND {
        return;
    }
    // The buffer has no pointer alignment guarantee: read unaligned.
    let pointers = info.as_ptr().add(8) as *const *mut c_void;
    let display = pointers.read_unaligned();
    let surface = pointers.add(1).read_unaligned();
    if !display.is_null() && !surface.is_null() {
        G_WL_DISPLAY = display;
        G_WL_SURFACE = surface;
    }
}

#[cfg(test)]
mod wayland_tests {
    use super::*;

    fn handles() -> (*mut c_void, *mut c_void) {
        unsafe { (G_WL_DISPLAY, G_WL_SURFACE) }
    }

    #[test]
    fn a_failed_query_cannot_reuse_a_previous_surface() {
        let _g = sn_base::testlock::serial();
        let (mut display, mut surface) = (0u8, 0u8);
        unsafe {
            G_WL_DISPLAY = std::ptr::addr_of_mut!(display).cast();
            G_WL_SURFACE = std::ptr::addr_of_mut!(surface).cast();
            update_wayland_info(0, &[0; 512]);
        }
        let (d, s) = handles();
        assert!(d.is_null() && s.is_null(), "a failed SDL query revokes both borrowed handles");
    }

    #[test]
    fn background_release_and_foreground_refresh_replace_the_borrow() {
        let _g = sn_base::testlock::serial();
        let (mut display, mut surface, mut replacement) = (0u8, 0u8, 0u8);
        let display_ptr = std::ptr::addr_of_mut!(display).cast::<c_void>();
        let surface_ptr = std::ptr::addr_of_mut!(surface).cast::<c_void>();
        let mut info = [0u8; 512];
        info[4..8].copy_from_slice(&SDL_SYSWM_WAYLAND.to_ne_bytes());
        unsafe {
            let pointers = info.as_mut_ptr().add(8) as *mut *mut c_void;
            pointers.write_unaligned(display_ptr);
            pointers.add(1).write_unaligned(surface_ptr);
            update_wayland_info(1, &info);
        }
        assert_eq!(handles(), (display_ptr, surface_ptr));
        assert!(sys_release_wayland(), "a held surface answers true");
        assert!(!sys_release_wayland(), "a second background has nothing left to release");
        let (d, s) = handles();
        assert!(d.is_null() && s.is_null());
        clear_opaque_region(); // no handle: nothing can be marshalled after the revocation
        let replacement_ptr = std::ptr::addr_of_mut!(replacement).cast::<c_void>();
        unsafe {
            let pointers = info.as_mut_ptr().add(8) as *mut *mut c_void;
            pointers.add(1).write_unaligned(replacement_ptr);
            update_wayland_info(1, &info);
        }
        assert_eq!(handles(), (display_ptr, replacement_ptr), "the foreground refresh takes the new surface");
        sys_release_wayland();
    }

    #[test]
    fn a_foreign_or_incomplete_answer_supplies_no_handles() {
        let _g = sn_base::testlock::serial();
        for subsystem in [0i32, 4, SDL_SYSWM_WAYLAND] {
            let (mut display, mut surface) = (0u8, 0u8);
            let mut info = [0u8; 512];
            info[4..8].copy_from_slice(&subsystem.to_ne_bytes());
            unsafe {
                let pointers = info.as_mut_ptr().add(8) as *mut *mut c_void;
                pointers.write_unaligned(std::ptr::addr_of_mut!(display).cast());
                if subsystem != SDL_SYSWM_WAYLAND {
                    pointers.add(1).write_unaligned(std::ptr::addr_of_mut!(surface).cast());
                }
                update_wayland_info(1, &info);
            }
            let (d, s) = handles();
            sys_release_wayland();
            assert!(d.is_null() && s.is_null(), "subsystem {subsystem}");
        }
    }
}
