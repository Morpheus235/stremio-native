//! **The Magic Remote's pointer**: hover, click, wheel, and hiding the pointer when it rests.
//!
//! Every pointer event becomes the dispatcher's own input (`InputKind::Pointer` / `Click`, an OK
//! release on the button-up, an Up/Down key per wheel tick), so the screens see it through the
//! same hit map and focus rules as the keys. Two rules are the remote's, not the screens':
//!
//! * **D-pad first.** After a key, the pointer takes focus again only once it has travelled
//!   [`WAKE_TRAVEL`] px: a remote resting in a hand jitters, and that jitter must not steal focus
//!   from the D-pad (PlxNative's rule).
//! * **A resting pointer hides** after [`HIDE_AFTER_MS`], and at once on any remote key, on every screen: the pointer stops
//!   drawing and the app stops getting reasons to repaint, so a still screen goes idle.
//!
//! Hiding uses LG's `SDL_webOSCursorVisibility` where the system SDL has it (webOS 4+); otherwise
//! a transparent SDL cursor is swapped in and the previous one restored on motion (PlxNative's
//! door, for the webOS 3 SDLs without LG's extension). Both are resolved at runtime.

use sn_base::eventlog::log;
use sn_machine::machine::{Edge, InputEvent, InputKind, Key, Source, Tick};

/// Pointer travel, in logical px, that hands focus back to the pointer after a key.
pub const WAKE_TRAVEL: f32 = 120.0;
/// How long a still pointer stays on screen.
pub const HIDE_AFTER_MS: u32 = 3000;
/// The shortest gap between two wheel steps: a remote's wheel fires several events per notch.
pub const WHEEL_GAP_MS: u32 = 250;

/// What the loop knows about the pointer.
#[derive(Debug, Default)]
pub struct Pointer {
    /// The last motion, in loop milliseconds; 0 before any.
    last_motion: u32,
    last_wheel: u32,
    hidden: bool,
    button_down: bool,
    /// A key was pressed since the pointer last took focus.
    dpad: bool,
    travel: f32,
    prev: Option<(f32, f32)>,
    /// A remote key asked for the pointer to go now, not after its rest.
    clear: bool,
}

impl Pointer {
    /// Motion to (x, y). Returns the input to deliver, if the pointer has focus.
    pub fn motion(&mut self, x: f32, y: f32, now: u32) -> Option<InputEvent<u32>> {
        self.last_motion = now.max(1);
        self.clear = false;
        if self.hidden {
            self.hidden = false;
            // SAFETY: called on the loop (main) thread, after SDL_Init.
            unsafe { show() };
        }
        if let Some((px, py)) = self.prev {
            self.travel += (x - px).abs() + (y - py).abs();
        }
        self.prev = Some((x, y));
        if self.dpad && !self.button_down {
            if self.travel < WAKE_TRAVEL {
                return None;
            }
            self.dpad = false;
        }
        let at = Tick { ms: now, dt_us: 0 };
        let kind = if self.button_down { InputKind::Drag { x, y, hit: None } } else { InputKind::Pointer { x, y, hit: None } };
        Some(InputEvent { at, source: Source::Sdl, kind })
    }

    /// The button went down at (x, y): a click.
    pub fn press(&mut self, x: f32, y: f32, now: u32) -> InputEvent<u32> {
        self.button_down = true;
        self.dpad = false;
        self.last_motion = now.max(1);
        InputEvent { at: Tick { ms: now, dt_us: 0 }, source: Source::Sdl, kind: InputKind::Click { x, y, hit: None } }
    }

    /// The button came up: the OK release the click's press waits for.
    pub fn release(&mut self, now: u32) -> InputEvent<u32> {
        self.button_down = false;
        self.last_motion = now.max(1);
        key(Key::Ok, Edge::Up, now)
    }

    /// A wheel tick (`dy` > 0 away from the viewer): one Up or Down step, at most one per
    /// [`WHEEL_GAP_MS`].
    pub fn wheel(&mut self, dy: i32, now: u32) -> Vec<InputEvent<u32>> {
        if dy == 0 || (self.last_wheel != 0 && now.wrapping_sub(self.last_wheel) < WHEEL_GAP_MS) {
            return Vec::new();
        }
        self.last_wheel = now.max(1);
        let k = if dy < 0 { Key::Down } else { Key::Up };
        vec![key(k, Edge::Down, now), key(k, Edge::Up, now)]
    }

    /// A remote key: the D-pad has focus until the pointer travels again, and the pointer
    /// clears at once (as webOS's own apps do) instead of after its rest.
    pub fn key(&mut self) {
        self.dpad = true;
        self.travel = 0.0;
        self.clear = !self.hidden && self.last_motion != 0;
    }

    /// Once a frame: hide a pointer that has rested for [`HIDE_AFTER_MS`]. Returns the input
    /// that tells the tree the pointer is gone (its hover ends).
    pub fn tick(&mut self, now: u32) -> Option<InputEvent<u32>> {
        if !self.due_to_hide(now) {
            return None;
        }
        self.hidden = true;
        self.clear = false;
        // SAFETY: called on the loop (main) thread, after SDL_Init.
        unsafe { hide() };
        Some(InputEvent { at: Tick { ms: now, dt_us: 0 }, source: Source::Sdl, kind: InputKind::PointerHidden })
    }

    fn due_to_hide(&self, now: u32) -> bool {
        !self.hidden && !self.button_down && self.last_motion != 0 && (self.clear || now.wrapping_sub(self.last_motion) > HIDE_AFTER_MS)
    }
}

fn key(key: Key, edge: Edge, now: u32) -> InputEvent<u32> {
    InputEvent { at: Tick { ms: now, dt_us: 0 }, source: Source::Sdl, kind: InputKind::Key { key, sym: 0, wcode: 0, edge, at_edge: false } }
}

// ---- hiding the pointer --------------------------------------------------------------------

type VisibilityFn = unsafe extern "C" fn(std::ffi::c_int) -> std::ffi::c_int;
type SetCursorFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type GetCursorFn = unsafe extern "C" fn() -> *mut std::ffi::c_void;
type CreateCursorFn = unsafe extern "C" fn(*const u8, *const u8, std::ffi::c_int, std::ffi::c_int, std::ffi::c_int, std::ffi::c_int) -> *mut std::ffi::c_void;

#[derive(Clone, Copy)]
enum Door {
    Lg(VisibilityFn),
    Sdl { set: SetCursorFn, get: GetCursorFn, def: GetCursorFn, blank: usize },
    None,
}

/// The cursor a transparent swap replaced, for the motion that restores it.
static SHOWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn sym(name: &str) -> Option<*mut std::ffi::c_void> {
    sn_base::dynlib::Handle::self_handle().sym(name).filter(|p| !p.is_null())
}

/// Resolved on first use (after `SDL_Init`: `SDL_CreateCursor` needs the video subsystem).
fn door() -> Door {
    static DOOR: std::sync::OnceLock<Door> = std::sync::OnceLock::new();
    *DOOR.get_or_init(|| {
        use std::mem::transmute;
        // SAFETY: each symbol is transmuted to its public SDL2 signature (LG's fork for the first).
        let door = unsafe {
            if let Some(f) = sym("SDL_webOSCursorVisibility") {
                Door::Lg(transmute::<*mut std::ffi::c_void, VisibilityFn>(f))
            } else if let (Some(set), Some(get), Some(def), Some(create)) =
                (sym("SDL_SetCursor"), sym("SDL_GetCursor"), sym("SDL_GetDefaultCursor"), sym("SDL_CreateCursor"))
            {
                let bits = [0u8; 1];
                let blank = transmute::<*mut std::ffi::c_void, CreateCursorFn>(create)(bits.as_ptr(), bits.as_ptr(), 8, 1, 0, 0);
                if blank.is_null() {
                    Door::None
                } else {
                    Door::Sdl {
                        set: transmute::<*mut std::ffi::c_void, SetCursorFn>(set),
                        get: transmute::<*mut std::ffi::c_void, GetCursorFn>(get),
                        def: transmute::<*mut std::ffi::c_void, GetCursorFn>(def),
                        blank: blank as usize,
                    }
                }
            } else {
                Door::None
            }
        };
        log(match door {
            Door::Lg(_) => "cursor: SDL_webOSCursorVisibility",
            Door::Sdl { .. } => "cursor: transparent SDL cursor",
            Door::None => "cursor: no way to hide the pointer",
        });
        door
    })
}

unsafe fn hide() {
    if cfg!(test) {
        return;
    }
    match door() {
        Door::Lg(f) => {
            f(0);
        }
        Door::Sdl { set, get, def, blank } => {
            let cur = get() as usize;
            if cur != blank {
                let keep = if cur != 0 { cur } else { def() as usize };
                if keep == 0 {
                    return;
                }
                SHOWN.store(keep, std::sync::atomic::Ordering::Relaxed);
            }
            set(blank as *mut std::ffi::c_void);
        }
        Door::None => {}
    }
}

unsafe fn show() {
    if cfg!(test) {
        return;
    }
    // LG's compositor shows its own pointer on motion; only a transparent swap needs undoing.
    if let Door::Sdl { set, .. } = door() {
        let shown = SHOWN.load(std::sync::atomic::Ordering::Relaxed);
        if shown != 0 {
            set(shown as *mut std::ffi::c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resting_pointer_hides_once_and_motion_brings_it_back() {
        let mut p = Pointer::default();
        assert!(p.tick(10_000).is_none(), "no pointer yet, nothing to hide");
        assert!(p.motion(100.0, 100.0, 1000).is_some());
        assert!(p.tick(3999).is_none());
        let hidden = p.tick(4001).expect("hidden after 3 s still");
        assert!(matches!(hidden.kind, InputKind::PointerHidden));
        assert!(p.tick(9000).is_none(), "hidden once, not every frame");
        assert!(p.motion(101.0, 100.0, 9001).is_some());
        assert!(p.tick(9500).is_none());
    }

    #[test]
    fn after_a_key_the_pointer_takes_focus_only_after_real_travel() {
        let mut p = Pointer::default();
        p.motion(100.0, 100.0, 10);
        p.key();
        assert!(p.motion(130.0, 100.0, 20).is_none(), "a jitter does not steal focus");
        assert!(p.motion(200.0, 100.0, 30).is_none());
        assert!(p.motion(260.0, 100.0, 40).is_some(), "120 px of travel hands focus back");
    }

    #[test]
    fn a_held_button_drags_and_never_hides() {
        let mut p = Pointer::default();
        let click = p.press(10.0, 10.0, 100);
        assert!(matches!(click.kind, InputKind::Click { .. }));
        assert!(matches!(p.motion(40.0, 10.0, 200).unwrap().kind, InputKind::Drag { .. }));
        assert!(p.tick(10_000).is_none(), "a held pointer stays");
        let up = p.release(10_001);
        assert!(matches!(up.kind, InputKind::Key { key: Key::Ok, edge: Edge::Up, .. }));
    }

    /// A remote key clears the pointer at once (webOS's own apps do): the D-pad has the screen,
    /// and the next motion brings the pointer back.
    #[test]
    fn a_remote_key_hides_the_pointer_at_once() {
        let mut p = Pointer::default();
        assert!(p.motion(100.0, 100.0, 1000).is_some());
        p.key();
        let hidden = p.tick(1016).expect("hidden on the next frame, not after 3 s");
        assert!(matches!(hidden.kind, InputKind::PointerHidden));
        assert!(p.tick(1032).is_none(), "once");
        p.key();
        assert!(p.tick(1048).is_none(), "already hidden: nothing to clear");
        assert!(p.motion(300.0, 100.0, 1100).is_some(), "a real move brings it back");
        assert!(p.tick(1116).is_none());
    }

    #[test]
    fn one_wheel_notch_is_one_step() {
        let mut p = Pointer::default();
        let down = p.wheel(-1, 1000);
        assert_eq!(down.len(), 2);
        assert!(matches!(down[0].kind, InputKind::Key { key: Key::Down, edge: Edge::Down, .. }));
        assert!(p.wheel(-1, 1100).is_empty(), "the same notch's burst");
        assert!(matches!(p.wheel(1, 1300)[0].kind, InputKind::Key { key: Key::Up, .. }));
        assert!(p.wheel(0, 2000).is_empty());
    }
}
