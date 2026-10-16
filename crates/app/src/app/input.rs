//! Keys from SDL's event bytes to the dispatcher's `InputEvent`.
//!
//! **The television's SDL fork does not lay `SDL_KeyboardEvent` out the way the headers do**: LG
//! writes `state` (u32) at +16, the webOS keycode at +20 and the SDL sym at +24. Stock SDL2 keeps
//! `state:u8` at +12 and `keysym.sym` at +20, so reading one layout out of the other yields the
//! window id as a key state — plausible garbage, not a crash. This build runs on the television
//! only, so the fork's offsets are the ones read.
//!
//! The packed state: low byte 1 pressed / 0 released, bit `0x100` a hardware auto-repeat.

use sn_machine::machine::{Edge, InputEvent, InputKind, Key, Source, Tick};
use sn_ui::consts::{classify, Key as UiKey};

pub fn rd_u32(ev: &[u8], off: usize) -> u32 {
    u32::from_ne_bytes([ev[off], ev[off + 1], ev[off + 2], ev[off + 3]])
}

/// A keyboard event's `(state, wcode, sym)` in the fork's layout.
pub fn decode_key(ev: &[u8]) -> (u32, u32, u32) {
    (rd_u32(ev, 16), rd_u32(ev, 20), rd_u32(ev, 24))
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

/// The remote's EXIT key: the one key that ends the process (BACK at the root hands the screen to
/// the television's Home and keeps running).
pub fn is_exit(sym: u32, wcode: u32, state: u32) -> bool {
    edge_of(state) == Edge::Down && classify(sym, wcode) == UiKey::Exit
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_ui::consts::{SDLK_DOWN, SDLK_RETURN};

    fn event(state: u32, wcode: u32, sym: u32) -> [u8; 64] {
        let mut ev = [0u8; 64];
        ev[0..4].copy_from_slice(&0x300u32.to_ne_bytes());
        ev[16..20].copy_from_slice(&state.to_ne_bytes());
        ev[20..24].copy_from_slice(&wcode.to_ne_bytes());
        ev[24..28].copy_from_slice(&sym.to_ne_bytes());
        ev
    }

    #[test]
    fn the_forks_layout_decodes_and_the_state_is_an_edge() {
        let ev = event(1, 0, SDLK_DOWN);
        assert_eq!(decode_key(&ev), (1, 0, SDLK_DOWN));
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
}
