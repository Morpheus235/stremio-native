//! Keys from SDL's event bytes to the dispatcher's `InputEvent`.
//!
//! **The television's SDL fork does not lay `SDL_KeyboardEvent` out the way the headers do**: LG
//! writes `state` (u32) at +16, the webOS keycode at +20 and the SDL sym at +24. Stock SDL2 keeps
//! `state:u8` at +12 and `keysym.sym` at +20, so reading one layout out of the other yields the
//! window id as a key state — plausible garbage, not a crash. The television build reads the
//! fork's offsets; the desktop simulator (`hostsim`, stock SDL2) reads the headers' and packs
//! them the same way (no webOS keycode: 0).
//!
//! The packed state: low byte 1 pressed / 0 released, bit `0x100` a hardware auto-repeat.

use sn_machine::machine::{Edge, InputEvent, InputKind, Key, Source, Tick};
use sn_ui::consts::{classify, Key as UiKey};

pub fn rd_u32(ev: &[u8], off: usize) -> u32 {
    u32::from_ne_bytes([ev[off], ev[off + 1], ev[off + 2], ev[off + 3]])
}

/// A keyboard event's `(state, wcode, sym)` in the fork's layout.
#[cfg(not(feature = "hostsim"))]
pub fn decode_key(ev: &[u8]) -> (u32, u32, u32) {
    (rd_u32(ev, 16), rd_u32(ev, 20), rd_u32(ev, 24))
}

/// A keyboard event's `(state, wcode, sym)` from stock SDL2's layout: `state:u8` at +12,
/// `repeat:u8` at +13 (packed as the fork's `0x100`), `keysym.sym` at +20.
#[cfg(feature = "hostsim")]
pub fn decode_key(ev: &[u8]) -> (u32, u32, u32) {
    let state = u32::from(ev[12]) | if ev[13] != 0 { 0x100 } else { 0 };
    (state, 0, rd_u32(ev, 20))
}

/// The packed state as an edge.
pub fn edge_of(state: u32) -> Edge {
    if (state & 0xff) != 1 {
        Edge::Up
    } else if state & 0x100 != 0 {
        Edge::Repeat
    } else {
        Edge::Down
    }
}

/// A key for the dispatcher: the raw fields classified once (`sn_ui::consts::classify`); the raw
/// `sym`/`wcode` ride along for the screens that read transport keys themselves (the player).
pub fn key_input(sym: u32, wcode: u32, state: u32, at: Tick) -> InputEvent<u32> {
    let key = match classify(sym, wcode) {
        UiKey::Up => Key::Up,
        UiKey::Down => Key::Down,
        UiKey::Left { .. } => Key::Left,
        UiKey::Right { .. } => Key::Right,
        UiKey::Ok => Key::Ok,
        UiKey::Back => Key::Back,
        _ => Key::Other,
    };
    InputEvent { at, source: Source::Sdl, kind: InputKind::Key { key, sym, wcode, edge: edge_of(state), at_edge: false } }
}

/// The television keyboard's commit as the dispatcher's input: the panel's own state edge first
/// when this firmware has one (a commit is proof the panel is up), then the text. Empty commits
/// are the IME's bookkeeping and carry nothing.
pub fn text_inputs(text: &str, panel: bool, at: Tick) -> Vec<InputEvent<u32>> {
    use sn_machine::machine::TextEdit;
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(2);
    if panel {
        out.push(InputEvent { at, source: Source::Sdl, kind: InputKind::SystemKeyboard(true) });
    }
    out.push(InputEvent { at, source: Source::Sdl, kind: InputKind::Text(TextEdit::Commit(text.into())) });
    out
}

/// The remote's EXIT key: the one key that ends the process (BACK at the root hands the screen to
/// the television's Home and keeps running).
pub fn is_exit(sym: u32, wcode: u32, state: u32) -> bool {
    edge_of(state) == Edge::Down && classify(sym, wcode) == UiKey::Exit
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_ui::consts::{SDLK_DOWN, SDLK_RETURN};

    #[cfg(not(feature = "hostsim"))]
    fn event(state: u32, wcode: u32, sym: u32) -> [u8; 64] {
        let mut ev = [0u8; 64];
        ev[0..4].copy_from_slice(&0x300u32.to_ne_bytes());
        ev[16..20].copy_from_slice(&state.to_ne_bytes());
        ev[20..24].copy_from_slice(&wcode.to_ne_bytes());
        ev[24..28].copy_from_slice(&sym.to_ne_bytes());
        ev
    }

    /// The simulator's stock SDL2 layout: a pressed, repeated ESC reads as the fork's packed
    /// state and BACK's sym.
    #[cfg(feature = "hostsim")]
    #[test]
    fn the_desktop_layout_decodes_like_the_forks() {
        let mut ev = [0u8; 64];
        ev[0..4].copy_from_slice(&0x300u32.to_ne_bytes());
        (ev[12], ev[13]) = (1, 1);
        ev[16..20].copy_from_slice(&41u32.to_ne_bytes()); // the scancode, never the state
        ev[20..24].copy_from_slice(&sn_ui::consts::SDLK_ESCAPE.to_ne_bytes());
        let (state, wcode, sym) = decode_key(&ev);
        assert_eq!((edge_of(state), wcode, sym), (Edge::Repeat, 0, sn_ui::consts::SDLK_ESCAPE));
        ev[13] = 0;
        assert_eq!(edge_of(decode_key(&ev).0), Edge::Down);
    }

    #[test]
    fn the_forks_layout_decodes_and_the_state_is_an_edge() {
        #[cfg(not(feature = "hostsim"))]
        assert_eq!(decode_key(&event(1, 0, SDLK_DOWN)), (1, 0, SDLK_DOWN));
        assert_eq!(edge_of(1), Edge::Down);
        assert_eq!(edge_of(0x101), Edge::Repeat);
        assert_eq!(edge_of(0), Edge::Up);
        let t = Tick { ms: 5, dt_us: 0 };
        match key_input(SDLK_DOWN, 0, 1, t).kind {
            InputKind::Key { key, edge, .. } => assert_eq!((key, edge), (Key::Down, Edge::Down)),
            other => panic!("a key: {other:?}"),
        }
        match key_input(SDLK_RETURN, 0, 0, t).kind {
            InputKind::Key { key, edge, .. } => assert_eq!((key, edge), (Key::Ok, Edge::Up)),
            other => panic!("a key: {other:?}"),
        }
        assert!(!is_exit(SDLK_RETURN, 0, 1));
    }

    #[test]
    fn a_commit_is_text_and_proves_the_panel_up() {
        let t = Tick { ms: 1, dt_us: 0 };
        assert!(text_inputs("", true, t).is_empty(), "an empty commit carries nothing");
        let ev = text_inputs("bat", true, t);
        assert!(matches!(ev[0].kind, InputKind::SystemKeyboard(true)));
        assert!(matches!(&ev[1].kind, InputKind::Text(sn_machine::machine::TextEdit::Commit(s)) if &**s == "bat"));
        assert_eq!(text_inputs("x", false, t).len(), 1, "no panel, no panel edge");
    }
}
