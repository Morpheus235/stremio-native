//! **The viewer's subtitle preferences, as the player reads them every frame.**
//!
//! The persisted value is the app's (a profile setting); the copy here is the one the player and
//! the subtitle renderers read without a lock: a process-global [`LiveCell`] the app publishes into
//! whenever the value changes or a profile is seated. Nothing here persists.

use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering::Relaxed;

/// A value that packs into the one byte a [`LiveCell`] holds.
pub trait Packed: Copy {
    /// What the cell holds before anything has published.
    const SEED: u8;
    fn to_u8(self) -> u8;
    fn from_u8(b: u8) -> Self;
}

/// A lock-free, process-global copy of one preference.
pub struct LiveCell<T: Packed> {
    a: AtomicU8,
    /// Names the global in the test-lock panic.
    #[cfg_attr(not(any(test, feature = "test-support")), allow(dead_code))]
    name: &'static str,
    _t: PhantomData<fn() -> T>,
}

impl<T: Packed> LiveCell<T> {
    pub const fn new(name: &'static str) -> Self {
        LiveCell { a: AtomicU8::new(T::SEED), name, _t: PhantomData }
    }

    /// Safe from any thread; a reader takes no lock.
    pub fn get(&self) -> T {
        T::from_u8(self.a.load(Relaxed))
    }

    /// Publish `v`. A cell is a process global read without a lock, so in tests a write without
    /// `testlock::serial()` would land in the middle of another test's read; the assertion turns
    /// that into a deterministic panic in the culprit.
    pub fn set(&self, v: T) {
        #[cfg(any(test, feature = "test-support"))]
        sn_base::testlock::assert_held(self.name);
        self.a.store(v.to_u8(), Relaxed);
    }
}

impl Packed for bool {
    const SEED: u8 = 1;
    fn to_u8(self) -> u8 {
        self as u8
    }
    fn from_u8(b: u8) -> Self {
        b != 0
    }
}

macro_rules! ladder {
    ($ty:ident, $default:ident) => {
        impl $ty {
            /// An in-memory index back to a rung — out of range is the default, never a
            /// neighbouring rung: the ladder can grow or shrink.
            pub fn from_index(i: u8) -> $ty {
                Self::LADDER.get(i as usize).copied().unwrap_or($ty::$default)
            }

            pub fn index(self) -> u8 {
                Self::LADDER.iter().position(|&r| r == self).unwrap_or(0) as u8
            }
        }

        impl Packed for $ty {
            const SEED: u8 = $ty::$default as u8;
            fn to_u8(self) -> u8 {
                self.index()
            }
            fn from_u8(b: u8) -> Self {
                $ty::from_index(b)
            }
        }
    };
}

/// The client-rendered caption's brightness, lightest first: over an HDR picture the panel maps
/// graphics white far above where it sits in SDR, and the only thing that makes a caption
/// comfortable there is less light. What each rung looks like is `ui::theme`'s. The serde names are
/// a file-format contract.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleTone {
    #[default]
    #[serde(rename = "white")]
    White,
    #[serde(rename = "grey_85")]
    Silver,
    #[serde(rename = "grey_70")]
    LightGrey,
    #[serde(rename = "grey_55")]
    Grey,
    #[serde(rename = "grey_40")]
    DarkGrey,
    #[serde(rename = "grey_28")]
    Charcoal,
}

impl SubtitleTone {
    pub const LADDER: [SubtitleTone; 6] = [
        SubtitleTone::White,
        SubtitleTone::Silver,
        SubtitleTone::LightGrey,
        SubtitleTone::Grey,
        SubtitleTone::DarkGrey,
        SubtitleTone::Charcoal,
    ];
}
ladder!(SubtitleTone, White);

/// The client-rendered caption's text size, smallest first.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleSize {
    #[serde(rename = "small")]
    Small,
    #[default]
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "large")]
    Large,
    #[serde(rename = "extra_large")]
    ExtraLarge,
    #[serde(rename = "huge")]
    Huge,
}

impl SubtitleSize {
    pub const LADDER: [SubtitleSize; 5] = [
        SubtitleSize::Small,
        SubtitleSize::Medium,
        SubtitleSize::Large,
        SubtitleSize::ExtraLarge,
        SubtitleSize::Huge,
    ];
}
ladder!(SubtitleSize, Medium);

/// The client-rendered caption's vertical placement, lowest first. Only the plain-text caption
/// moves with it; image captions and authored ASS keep their own placement.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitlePosition {
    #[default]
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "middle")]
    Middle,
    #[serde(rename = "high")]
    High,
}

impl SubtitlePosition {
    pub const LADDER: [SubtitlePosition; 3] = [SubtitlePosition::Low, SubtitlePosition::Middle, SubtitlePosition::High];
}
ladder!(SubtitlePosition, Low);

static SUBTITLE_SIZE: LiveCell<SubtitleSize> = LiveCell::new("subtitle size preference");
static SUBTITLE_POSITION: LiveCell<SubtitlePosition> = LiveCell::new("subtitle position preference");
pub(crate) static SUBTITLE_TONE: LiveCell<SubtitleTone> = LiveCell::new("subtitle tone preference");

/// The live caption size.
pub fn subtitle_size() -> SubtitleSize {
    SUBTITLE_SIZE.get()
}

/// Publish the caption size (the app persists it; this does not).
pub fn set_subtitle_size(size: SubtitleSize) {
    SUBTITLE_SIZE.set(size)
}

/// The live caption placement.
pub fn subtitle_position() -> SubtitlePosition {
    SUBTITLE_POSITION.get()
}

/// Publish the caption placement (the app persists it; this does not).
pub fn set_subtitle_position(position: SubtitlePosition) {
    SUBTITLE_POSITION.set(position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rung_round_trips_through_its_byte_and_out_of_range_is_the_default() {
        for s in SubtitleSize::LADDER {
            assert_eq!(SubtitleSize::from_u8(s.to_u8()), s);
        }
        for p in SubtitlePosition::LADDER {
            assert_eq!(SubtitlePosition::from_u8(p.to_u8()), p);
        }
        for t in SubtitleTone::LADDER {
            assert_eq!(SubtitleTone::from_u8(t.to_u8()), t);
        }
        assert_eq!(SubtitleSize::from_index(99), SubtitleSize::Medium);
        assert_eq!(SubtitlePosition::from_index(99), SubtitlePosition::Low);
        assert_eq!(SubtitleTone::from_index(99), SubtitleTone::White);
        assert_eq!(SubtitleSize::from_u8(<SubtitleSize as Packed>::SEED), SubtitleSize::Medium);
    }
}
