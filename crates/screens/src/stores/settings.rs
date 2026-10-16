//! **The settings store** — the viewer's preferences the Settings pages show and change.
//!
//! A preference is a rung on a ladder ([`Pref`]): the subtitle size and the subtitle position.
//! The ladders' rungs are the media layer's (`sn_media::prefs::SubtitleSize::LADDER`,
//! `SubtitlePosition::LADDER`) — this store keeps the index, the application maps it to the
//! media type, publishes it to the player and persists it, and its tests hold the two ladders the
//! same length with the same default. The search addon filter and the recent searches are
//! settings too, held by the search store beside the rows they shape.

/// A preference the Settings pages offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pref {
    SubtitleSize = 0,
    SubtitlePosition = 1,
}

impl Pref {
    pub const ALL: [Pref; 2] = [Pref::SubtitleSize, Pref::SubtitlePosition];

    /// How many rungs the ladder has (`sn_media::prefs`'s `LADDER.len()`).
    pub const fn rungs(self) -> u8 {
        match self {
            Pref::SubtitleSize => 5,
            Pref::SubtitlePosition => 3,
        }
    }

    /// The rung a fresh install starts on (`sn_media::prefs`'s `#[default]`).
    pub const fn default_rung(self) -> u8 {
        match self {
            Pref::SubtitleSize => 1,
            Pref::SubtitlePosition => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsStore {
    rungs: [u8; 2],
    gen: u32,
}

impl Default for SettingsStore {
    fn default() -> Self {
        SettingsStore { rungs: Pref::ALL.map(Pref::default_rung), gen: 0 }
    }
}

impl SettingsStore {
    /// A store seeded from what the settings file held (an out-of-range rung is the default).
    pub fn with(size: u8, position: u8) -> Self {
        let mut s = Self::default();
        s.rungs = [size, position];
        for p in Pref::ALL {
            if s.rungs[p as usize] >= p.rungs() {
                s.rungs[p as usize] = p.default_rung();
            }
        }
        s
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    pub fn get(&self, p: Pref) -> u8 {
        self.rungs[p as usize]
    }

    /// Set `p` to `rung`; `false` (and nothing moves) when it already is, or out of range.
    pub fn set(&mut self, p: Pref, rung: u8) -> bool {
        if rung >= p.rungs() || self.rungs[p as usize] == rung {
            return false;
        }
        self.rungs[p as usize] = rung;
        self.gen = self.gen.wrapping_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rung_out_of_range_is_the_default_and_a_set_moves_the_generation_once() {
        let mut s = SettingsStore::with(9, 2);
        assert_eq!((s.get(Pref::SubtitleSize), s.get(Pref::SubtitlePosition)), (1, 2));
        let g = s.gen();
        assert!(s.set(Pref::SubtitleSize, 4));
        assert!(!s.set(Pref::SubtitleSize, 4), "the same rung is no change");
        assert!(!s.set(Pref::SubtitlePosition, 3), "past the ladder is refused");
        assert_eq!(s.gen(), g.wrapping_add(1));
    }
}
