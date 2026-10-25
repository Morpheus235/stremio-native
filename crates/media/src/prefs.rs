//! **The viewer's playback preferences (subtitles, Dolby Vision Profile 7), as the player reads them.**
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

/// The text caption's fill colour (Settings > Audio & subtitles > Text colour): stremio-web's
/// `subtitlesTextColor` as a palette a remote can step through, white first (its default). What
/// each rung is in ASS is [`crate::player::text_style`]'s. The serde names are a file-format contract.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleColor {
    #[default]
    #[serde(rename = "white")]
    White,
    #[serde(rename = "yellow")]
    Yellow,
    #[serde(rename = "cyan")]
    Cyan,
    #[serde(rename = "green")]
    Green,
    #[serde(rename = "grey")]
    Grey,
}

impl SubtitleColor {
    pub const LADDER: [SubtitleColor; 5] =
        [SubtitleColor::White, SubtitleColor::Yellow, SubtitleColor::Cyan, SubtitleColor::Green, SubtitleColor::Grey];
}
ladder!(SubtitleColor, White);

/// The box behind a text caption (`subtitlesBackgroundColor`, transparent by default).
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleBackground {
    #[default]
    #[serde(rename = "none")]
    None,
    #[serde(rename = "black_50")]
    Black50,
    #[serde(rename = "black_75")]
    Black75,
    #[serde(rename = "black")]
    Black,
}

impl SubtitleBackground {
    pub const LADDER: [SubtitleBackground; 4] =
        [SubtitleBackground::None, SubtitleBackground::Black50, SubtitleBackground::Black75, SubtitleBackground::Black];
}
ladder!(SubtitleBackground, None);

/// The text caption's outline colour (`subtitlesOutlineColor`, black by default).
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleOutline {
    #[default]
    #[serde(rename = "black")]
    Black,
    #[serde(rename = "grey")]
    Grey,
    #[serde(rename = "white")]
    White,
}

impl SubtitleOutline {
    pub const LADDER: [SubtitleOutline; 3] = [SubtitleOutline::Black, SubtitleOutline::Grey, SubtitleOutline::White];
}
ladder!(SubtitleOutline, Black);

/// The text caption's opacity (`subtitlesOpacity`, 100 % by default): text, outline and box alike.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleOpacity {
    #[default]
    #[serde(rename = "100")]
    Full,
    #[serde(rename = "75")]
    P75,
    #[serde(rename = "50")]
    P50,
}

impl SubtitleOpacity {
    pub const LADDER: [SubtitleOpacity; 3] = [SubtitleOpacity::Full, SubtitleOpacity::P75, SubtitleOpacity::P50];

    /// The opacity in percent.
    pub fn percent(self) -> u8 {
        match self {
            SubtitleOpacity::Full => 100,
            SubtitleOpacity::P75 => 75,
            SubtitleOpacity::P50 => 50,
        }
    }
}
ladder!(SubtitleOpacity, Full);

/// How a Dolby Vision **Profile 7** (dual-layer) stream is played (`stream::Dovi::presentation_with`).
/// The serde names are a file-format contract.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DvProfile7 {
    /// The base layer alone, as HDR10: RPU and enhancement layer dropped by the demuxer.
    #[default]
    #[serde(rename = "hdr10")]
    Hdr10,
    /// Converted in the feed to single-layer Profile 8.1 (`crate::dovi`) and declared as 8.1; the
    /// player falls back to [`DvProfile7::Hdr10`] by itself when the set refuses it (`player::pump`).
    #[serde(rename = "try_dolby_vision")]
    TryNative,
}

impl DvProfile7 {
    pub const LADDER: [DvProfile7; 2] = [DvProfile7::Hdr10, DvProfile7::TryNative];
}
ladder!(DvProfile7, Hdr10);

static DV_PROFILE7: LiveCell<DvProfile7> = LiveCell::new("dolby vision profile 7 preference");

/// The live Profile 7 choice, read once per playback start.
pub fn dv_profile7() -> DvProfile7 {
    DV_PROFILE7.get()
}

/// Publish the Profile 7 choice (the app persists it; this does not).
pub fn set_dv_profile7(v: DvProfile7) {
    DV_PROFILE7.set(v)
}

/// **What a track of more than two channels that the app decodes itself becomes** (Settings >
/// Playback > Multichannel audio). The serde names are a file-format contract.
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultichannelAudio {
    /// AC-3 5.1 over every route, eARC included: webOS 6 (the owner-reported C1, Pass Through and
    /// eARC on) mixes app PCM down to stereo inside the TV, while a Dolby bitstream reaches the
    /// soundbar intact — Kodi's webOS port never sends PCM for the same reason. PCM stays a rung.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    /// Always AC-3 5.1 (`stream::AudioFeed::TranscodeAc3`): what optical and plain ARC carry.
    #[serde(rename = "dolby_digital")]
    DolbyDigital,
    /// Always E-AC-3 5.1 at 768 kb/s (`stream::AudioFeed::TranscodeEac3`, declared `"AC3 PLUS"`),
    /// for HDMI ARC / eARC; AC-3 where the build or the table has no E-AC-3. Over optical the TV
    /// converts it back to AC-3.
    #[serde(rename = "dolby_digital_plus")]
    DolbyDigitalPlus,
    /// Always 6-channel PCM (`stream::AudioFeed::SoftPcm`), the behaviour before this setting.
    #[serde(rename = "pcm")]
    Pcm,
    /// Every track of more than two channels the build decodes — the television's own codecs too —
    /// is decoded here and folded to a plain stereo pair (libswresample's Lo/Ro downmix: centre
    /// and surrounds at -3 dB, LFE dropped), for TV speakers, headphones, Bluetooth or a soundbar
    /// without a decoder. Never chosen by `Auto`.
    #[serde(rename = "stereo")]
    Stereo,
    /// Every track of more than two channels — the ones the television decodes itself too — is
    /// decoded here and folded to a Dolby Pro Logic II matrixed stereo pair, for a Pro Logic II
    /// receiver on a stereo link (optical with LG Sound Sync). Never chosen by `Auto`.
    #[serde(rename = "stereo_dplii")]
    StereoDplii,
}

impl MultichannelAudio {
    pub const LADDER: [MultichannelAudio; 6] = [
        MultichannelAudio::Auto,
        MultichannelAudio::DolbyDigital,
        MultichannelAudio::DolbyDigitalPlus,
        MultichannelAudio::Pcm,
        MultichannelAudio::Stereo,
        MultichannelAudio::StereoDplii,
    ];
}
ladder!(MultichannelAudio, Auto);

static MULTICHANNEL_AUDIO: LiveCell<MultichannelAudio> = LiveCell::new("multichannel audio preference");

/// The live multichannel choice, read once per playback start.
pub fn multichannel_audio() -> MultichannelAudio {
    MULTICHANNEL_AUDIO.get()
}

/// Publish the multichannel choice (the app persists it; this does not).
pub fn set_multichannel_audio(v: MultichannelAudio) {
    MULTICHANNEL_AUDIO.set(v)
}

/// How a track of more than two channels decoded here is fed, as the feed decision reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Multichannel {
    /// Re-encoded as AC-3 5.1 (where the build and the codec table allow; else 6-channel PCM).
    #[default]
    Ac3,
    /// Re-encoded as E-AC-3 5.1 (where the build and the codec table allow; else AC-3).
    Eac3,
    /// 6-channel PCM.
    Pcm,
    /// Plain stereo PCM (Lo/Ro downmix) — every >2-channel track, native codecs included.
    Stereo,
    /// Dolby Pro Logic II matrixed stereo PCM — every >2-channel track, native codecs included.
    StereoDplii,
}

/// The facts of the television's sound route that matter here, from the settings service's
/// `sound` category (`soundOutput`, `eArcSupport`). Values as the owner's C1 (webOS 6) answers:
/// `external_optical` / `off`; `external_arc` is the HDMI ARC/eARC output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundRoute {
    pub output: String,
    pub earc: bool,
}

/// PURE: the route out of a `getSystemSettings` reply. The service answers `returnValue: false`
/// for a key it does not know and STILL returns the ones it does, so `settings` is read whatever
/// `returnValue` says; no `soundOutput` there is no answer.
pub fn parse_sound_route(reply: &str) -> Option<SoundRoute> {
    let v: serde_json::Value = serde_json::from_str(reply).ok()?;
    let s = v.get("settings")?;
    let output = s.get("soundOutput")?.as_str()?.to_string();
    let earc = s.get("eArcSupport").and_then(|e| e.as_str()) == Some("on");
    Some(SoundRoute { output, earc })
}

/// PURE: **the multichannel decision** from the preference. `Auto` is AC-3 on every route: the
/// path proven on the television, what optical and plain ARC carry, and over eARC the one that
/// stays 5.1 (app PCM is mixed to stereo there on webOS 6).
///
/// ponytail: Auto does not try E-AC-3 on eARC — it is 5.1 either way (FFmpeg's encoder has no
/// 7.1), so the gain is 128 kb/s; Dolby Digital Plus is its own rung.
pub fn multichannel_out(pref: MultichannelAudio) -> Multichannel {
    match pref {
        MultichannelAudio::Auto | MultichannelAudio::DolbyDigital => Multichannel::Ac3,
        MultichannelAudio::DolbyDigitalPlus => Multichannel::Eac3,
        MultichannelAudio::Pcm => Multichannel::Pcm,
        MultichannelAudio::Stereo => Multichannel::Stereo,
        MultichannelAudio::StereoDplii => Multichannel::StereoDplii,
    }
}

/// The decision for the start being made.
pub fn multichannel() -> Multichannel {
    multichannel_out(multichannel_audio())
}

/// Log the television's sound route (an LS2 round trip: off the frame thread — the probe worker
/// calls it before each probe) beside the multichannel choice: no decision reads it any more, but
/// a report of "stereo on my soundbar" needs the route next to what was sent.
pub fn refresh_sound_route() {
    let route = sn_platform::tv::sound_settings().as_deref().and_then(parse_sound_route);
    sn_base::eventlog::log(&format!(
        "audio route: {} -> multichannel {:?}",
        route.as_ref().map_or("unknown".to_string(), |r| format!("output={} earc={}", r.output, r.earc)),
        multichannel()
    ));
}

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

static SUBTITLE_COLOR: LiveCell<SubtitleColor> = LiveCell::new("subtitle colour preference");
static SUBTITLE_BACKGROUND: LiveCell<SubtitleBackground> = LiveCell::new("subtitle background preference");
static SUBTITLE_OUTLINE: LiveCell<SubtitleOutline> = LiveCell::new("subtitle outline preference");
static SUBTITLE_OPACITY: LiveCell<SubtitleOpacity> = LiveCell::new("subtitle opacity preference");
/// Seeded `true` (bool's seed): an authored ASS script keeps its own look until the app says otherwise.
static ASS_STYLING: LiveCell<bool> = LiveCell::new("ass styling preference");

/// The live caption colour, box, outline and opacity (the app persists them; these do not).
pub fn subtitle_color() -> SubtitleColor {
    SUBTITLE_COLOR.get()
}
pub fn set_subtitle_color(v: SubtitleColor) {
    SUBTITLE_COLOR.set(v)
}
pub fn subtitle_background() -> SubtitleBackground {
    SUBTITLE_BACKGROUND.get()
}
pub fn set_subtitle_background(v: SubtitleBackground) {
    SUBTITLE_BACKGROUND.set(v)
}
pub fn subtitle_outline() -> SubtitleOutline {
    SUBTITLE_OUTLINE.get()
}
pub fn set_subtitle_outline(v: SubtitleOutline) {
    SUBTITLE_OUTLINE.set(v)
}
pub fn subtitle_opacity() -> SubtitleOpacity {
    SUBTITLE_OPACITY.get()
}
pub fn set_subtitle_opacity(v: SubtitleOpacity) {
    SUBTITLE_OPACITY.set(v)
}

/// **Style ASS subtitles**: `true` renders an ASS/SSA track with its own styling; `false` renders
/// its dialogue as plain text in the viewer's caption style (stremio's `assSubtitlesStyling`). Read
/// when a file's subtitle tracks are registered and when a sidecar is decoded, so a change applies
/// from the next playback.
pub fn ass_styling() -> bool {
    ASS_STYLING.get()
}
pub fn set_ass_styling(v: bool) {
    ASS_STYLING.set(v)
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
        assert_eq!(DvProfile7::from_u8(<DvProfile7 as Packed>::SEED), DvProfile7::Hdr10, "HDR10 until the app publishes");
        assert_eq!(DvProfile7::from_u8(DvProfile7::TryNative.to_u8()), DvProfile7::TryNative);
        assert_eq!(SubtitleSize::from_index(99), SubtitleSize::Medium);
        assert_eq!(SubtitlePosition::from_index(99), SubtitlePosition::Low);
        assert_eq!(SubtitleTone::from_index(99), SubtitleTone::White);
        assert_eq!(SubtitleSize::from_u8(<SubtitleSize as Packed>::SEED), SubtitleSize::Medium);
        for c in SubtitleColor::LADDER {
            assert_eq!(SubtitleColor::from_u8(c.to_u8()), c);
        }
        for b in SubtitleBackground::LADDER {
            assert_eq!(SubtitleBackground::from_u8(b.to_u8()), b);
        }
        assert_eq!(SubtitleOutline::from_index(99), SubtitleOutline::Black);
        assert_eq!(SubtitleOpacity::from_u8(<SubtitleOpacity as Packed>::SEED), SubtitleOpacity::Full);
        assert!(<bool as Packed>::from_u8(<bool as Packed>::SEED), "ASS keeps its styling until the app publishes");
        for m in MultichannelAudio::LADDER {
            assert_eq!(MultichannelAudio::from_u8(m.to_u8()), m);
        }
        assert_eq!(MultichannelAudio::from_u8(<MultichannelAudio as Packed>::SEED), MultichannelAudio::Auto);
    }

    /// Every rung's decision. Automatic is AC-3 whatever the route — issue #3: it picked PCM on
    /// eARC, and the C1 (webOS 6) played that as 2.0. Pro Logic II and PCM are never Auto's.
    #[test]
    fn the_multichannel_choice_follows_the_rung_and_automatic_is_ac3_even_on_earc() {
        use MultichannelAudio::*;
        let cases = [
            (Auto, Multichannel::Ac3),
            (DolbyDigital, Multichannel::Ac3),
            (DolbyDigitalPlus, Multichannel::Eac3),
            (Pcm, Multichannel::Pcm),
            (Stereo, Multichannel::Stereo),
            (StereoDplii, Multichannel::StereoDplii),
        ];
        assert_eq!(cases.len(), MultichannelAudio::LADDER.len());
        for (pref, want) in cases {
            assert_eq!(multichannel_out(pref), want, "{pref:?}");
        }
        let optical = parse_sound_route(r#"{"returnValue":false,"settings":{"soundOutput":"external_optical","eArcSupport":"off","soundOutputDigital":"pcm"}}"#).unwrap();
        assert_eq!(optical, SoundRoute { output: "external_optical".into(), earc: false }, "returnValue false still carries the known keys");
        assert_eq!(parse_sound_route(r#"{"returnValue":false,"errorText":"x"}"#), None);
        assert_eq!(parse_sound_route("not json"), None);
    }
}
