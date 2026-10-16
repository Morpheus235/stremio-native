//! **What a stream IS, as the Load must declare it** — and the two decisions that follow from it
//! before a native object is built: how its Dolby Vision is presented, and whether its audio can
//! be fed to the television's decoder at all.
//!
//! LG's buffer-feed pipeline is told the codecs, the frame rate and the sink envelope in the Load
//! payload, BEFORE the first access unit is fed, and it builds its decoders from that declaration
//! rather than from the bitstream. So the facts here must be known at start: the caller probes the
//! stream (its container is the only authority, an addon's metadata is not) and hands the result
//! in with [`crate::session::PlaybackStart`]. The demuxer then refuses a file that disagrees with
//! what was declared (`ff::demux`), because a Load that says one codec over another codec's
//! bitstream is a black screen with nothing in the log.

/// The declared shape of one playback. All-empty is the local-sample feeds' baseline (an H.264
/// video-only declaration).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamDeclaration {
    /// The video codec in FFmpeg's lowercase naming (`h264`, `hevc`, `av1`, `vp9`). Empty means
    /// H.264, the baseline the static payloads declare.
    pub vcodec: String,
    /// The audio codec of the track to feed, FFmpeg's lowercase naming (`aac`, `ac3`, `eac3`,
    /// `dts`, `truehd`, ...). Empty: the stream has no audio track to feed.
    pub acodec: String,
    /// That track's channel count; 0 when unknown.
    pub audio_channels: i64,
    /// The video frame rate; 0.0 when unknown.
    pub fps: f64,
    /// The coded raster `(width, height)`; 0 on an axis nobody knows.
    pub raster: (u16, u16),
    /// What the container says about Dolby Vision.
    pub dovi: Dovi,
    /// The audio track is Dolby Atmos (E-AC3 JOC).
    pub atmos: bool,
}

/// A stream's **Dolby Vision** layering, read together, because the only question worth asking of
/// it is a joint one: **is the base layer, alone, a correct picture?** The buffer-feed pipeline
/// feeds one elementary stream to a decoder that has never heard of an RPU, so the base layer is
/// exactly what reaches the panel unless the Load declares the stream as Dolby Vision.
///
/// All zero is "nothing said", which is also what a non-DV stream produces — see
/// [`Dovi::base_layer_unusable`] for why silence must never convict.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Dovi {
    /// The stream carries Dolby Vision at all (a `dvcC`/`dvvC` configuration record).
    pub present: bool,
    /// The profile: 5 / 7 / 8 …; 0 means unknown.
    pub profile: i64,
    /// The base layer's signal compatibility id: 0 none (P5) / 1 HDR10 / 2 SDR / 4 HLG.
    pub bl_compat: i64,
    /// An enhancement layer is present (P7).
    pub el_present: bool,
}

impl Dovi {
    pub const NONE: Dovi = Dovi { present: false, profile: 0, bl_compat: 0, el_present: false };

    /// PURE: **true when the base layer on its own is not a picture the panel shows correctly** when
    /// the pipeline was never told it is Dolby Vision: an enhancement layer (P7: the other layer
    /// never arrives) or no cross-compatible base layer (P5's IPT-PQ, decoded as ordinary HEVC
    /// with a pink-green cast). A `profile` of 0 never convicts: silence is indistinguishable from
    /// an SDR file.
    pub fn base_layer_unusable(&self) -> bool {
        if !self.present {
            return false;
        }
        self.el_present || self.profile == 5 || (self.profile > 0 && self.bl_compat == 0)
    }

    /// PURE: **how this stream will be presented** — the one predicate behind both the refusal and
    /// the Load payload's `DolbyHdrInfo` node, so the two can never disagree.
    ///
    /// - not `present` → [`DvPresentation::NotDv`];
    /// - `el_present` → refuse, always: one elementary stream cannot carry two layers;
    /// - no node will be sent (capability not a definite `Supported`, non-HEVC video, an unknown
    ///   profile, or `signal` withheld on an unusable base layer) → refuse iff the base layer is
    ///   unusable, else play the base layer;
    /// - otherwise declare it, which is what makes the panel enter Dolby Vision mode (libpf sets
    ///   `dolby-vision=TRUE` on the mere presence of the node — decompiled from this television's
    ///   own `CustomPipeline::parseOptionStringSpi`).
    pub fn presentation(
        &self,
        signal: bool,
        capability: sn_platform::devcaps::dv::DvCapability,
        video_is_hevc: bool,
    ) -> DvPresentation {
        if !self.present {
            return DvPresentation::NotDv;
        }
        if self.el_present {
            return DvPresentation::Refuse("dual-layer");
        }
        let declare = capability == sn_platform::devcaps::dv::DvCapability::Supported
            && video_is_hevc
            && (signal || !self.base_layer_unusable());
        if !declare || self.profile <= 0 {
            return if self.base_layer_unusable() {
                DvPresentation::Refuse("no cross-compatible base layer")
            } else {
                DvPresentation::NotDv
            };
        }
        DvPresentation::Declare(DolbyHdrInfo {
            profile_id: self.profile,
            track_type: "single",
            // Never `"all"`: with `trackType: "dual"` that sets `dv-dual-svp`, the secure video
            // path this app cannot satisfy.
            encryption_type: "clear",
        })
    }

    /// [`Self::presentation`] from the cached platform answer and the boot-latched diagnostic
    /// signal. Resolved once per playback start; reloads keep the value they were started with.
    pub fn presentation_now(&self, video_is_hevc: bool) -> DvPresentation {
        self.presentation(!dv_withheld(), sn_platform::devcaps::dv::capability(), video_is_hevc)
    }
}

/// The `option.externalStreamingInfo.contents.DolbyHdrInfo` node of the Starfish Load payload.
/// `profileId` must be a JSON integer (`getInt`); the other two are strings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DolbyHdrInfo {
    pub profile_id: i64,
    /// `"single"` or `"dual"`.
    pub track_type: &'static str,
    /// `"clear"`.
    pub encryption_type: &'static str,
}

/// What [`Dovi::presentation`] decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DvPresentation {
    /// Not Dolby Vision, or not identifiably so: play it as ordinary HEVC, declare nothing.
    NotDv,
    /// Play it with this node spliced into the Load payload.
    Declare(DolbyHdrInfo),
    /// The base layer alone is a wrong picture and the node cannot be sent: refuse the start.
    Refuse(&'static str),
}

impl DvPresentation {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::NotDv => "base-layer",
            Self::Declare(_) => "declare",
            Self::Refuse(_) => "refuse",
        }
    }

    /// The node to splice into the Load payload, if any.
    pub fn declared(&self) -> Option<DolbyHdrInfo> {
        match self {
            Self::Declare(d) => Some(*d),
            _ => None,
        }
    }

    /// Why the start is refused, if it is.
    pub fn refusal(&self) -> Option<&'static str> {
        match self {
            Self::Refuse(why) => Some(why),
            _ => None,
        }
    }
}

sn_base::devtrig::latched_flag!(
    /// `/tmp/stremionative-dvnonode` — keep a declared Dolby Vision playback but send **no**
    /// `DolbyHdrInfo` node. Diagnostic only, applied at the payload: the picture is expected to be
    /// WRONG for a Profile 5 stream; this knob judges cadence, not colour.
    pub fn dv_node_suppressed = "dvnonode";
);

sn_base::devtrig::latched_flag!(
    /// `/tmp/stremionative-nodv` — **withhold the Dolby Vision declaration**, for a bisect. A
    /// withheld Profile 5 is refused (its base layer is unusable), so this bisects "declared vs
    /// refused"; [`dv_node_suppressed`] is the finer instrument.
    pub fn dv_withheld = "nodv";
);

/// **How the selected audio track reaches the speakers**, decided at start from the declaration
/// and the television's own codec table (`sn_platform::devcaps`). One decision, so the Load
/// payload, the demuxer's framing and the refusal can never disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioFeed {
    /// The stream has no audio track: a video-only Load, played silently.
    Silent,
    /// The television decodes the track as demuxed (AAC ADTS-framed by the demuxer). `payload`
    /// is LG's name for it in the Load (`"AC3 PLUS"` for E-AC3, never `"EAC3"`: the wrong
    /// string leaves the audio ES unconfigured and, through `audioSync`, stalls the video).
    Native { payload: &'static str, diag: u8 },
    /// DTS: the demuxer extracts the **core** of every packet (`ff::dts_core_packet`, DTS-HD
    /// extensions discarded) and the television's DTS decoder plays it. Only chosen when the
    /// codec table lists DTS with a channel ceiling the track fits.
    DtsCore,
    /// **The television cannot decode this track** — a codec the pipeline has no decoder for
    /// (TrueHD, FLAC, Opus, ...), or DTS/any listed codec beyond its measured channel ceiling.
    /// Nothing upstream re-encodes a stream, so it must be decoded in software and fed as PCM or
    /// AC3. That decoder does not exist yet: a start that reaches this decision is refused with an
    /// explicit error (`player::FailureKind::AudioUnsupported`) instead of a Load whose audio
    /// clock never starts.
    NeedsSoftwareDecode { codec: String, channels: i64 },
}

impl AudioFeed {
    /// The decision for `codec` with `channels` on a set whose table is `caps`.
    pub fn decide(codec: &str, channels: i64, caps: &sn_platform::devcaps::Caps) -> AudioFeed {
        let codec = codec.to_ascii_lowercase();
        if codec.is_empty() {
            return AudioFeed::Silent;
        }
        // LG's buffer-feed vocabulary (libpf `CustomPipeline::parseOptionStringSpi`); the diag
        // code is `SHARED.dg_load_a`'s. Anything else has no name the pipeline accepts.
        let named = match codec.as_str() {
            "ac3" => Some(("AC3", 1)),
            "eac3" => Some(("AC3 PLUS", 2)),
            "aac" => Some(("AAC", 3)),
            "dts" => Some(("DTS", 4)),
            _ => None,
        };
        match named {
            Some(_) if codec == "dts" && caps.audio_supports("dts", channels) => AudioFeed::DtsCore,
            Some((payload, diag)) if codec != "dts" && caps.audio_supports(&codec, channels) => {
                AudioFeed::Native { payload, diag }
            }
            _ => AudioFeed::NeedsSoftwareDecode { codec, channels },
        }
    }

    /// The Load payload's audio codec name and the diagnostics code, when the television decodes
    /// the track.
    pub fn payload(&self) -> Option<(&'static str, u8)> {
        match self {
            AudioFeed::Native { payload, diag } => Some((payload, *diag)),
            AudioFeed::DtsCore => Some(("DTS", 4)),
            AudioFeed::Silent | AudioFeed::NeedsSoftwareDecode { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_platform::devcaps::dv::DvCapability;
    use sn_platform::devcaps::Caps;

    fn caps_with_dts(limit: Option<u32>) -> Caps {
        let mut c = Caps::assumed();
        c.audio = "aac,ac3,eac3,dts".to_string();
        if let Some(l) = limit {
            c.audio_channels.insert("dts".to_string(), l);
        }
        c
    }

    #[test]
    fn dts_is_fed_as_its_core_only_when_the_table_lists_it_with_a_ceiling_the_track_fits() {
        let six = caps_with_dts(Some(6));
        assert_eq!(AudioFeed::decide("dts", 6, &six), AudioFeed::DtsCore);
        assert_eq!(AudioFeed::decide("DTS", 2, &six), AudioFeed::DtsCore, "codec names are case-insensitive");
        assert_eq!(
            AudioFeed::decide("dts", 8, &six),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 8 },
            "a 7.1 track past a 5.1 ceiling is not the decoder's"
        );
        assert_eq!(
            AudioFeed::decide("dts", 0, &six),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 0 },
            "an unknown channel count never fits a measured ceiling"
        );
        assert_eq!(
            AudioFeed::decide("dts", 6, &caps_with_dts(None)),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 6 },
            "DTS listed without a measured ceiling is not admitted"
        );
        assert_eq!(
            AudioFeed::decide("dts", 6, &Caps::assumed()),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 6 },
            "a table that lists no DTS decoder"
        );
        assert_eq!(AudioFeed::DtsCore.payload(), Some(("DTS", 4)));
    }

    #[test]
    fn the_pipelines_own_codecs_are_native_and_everything_else_needs_a_software_decoder() {
        let c = Caps::assumed();
        assert_eq!(AudioFeed::decide("eac3", 6, &c).payload(), Some(("AC3 PLUS", 2)));
        assert_eq!(AudioFeed::decide("ac3", 6, &c).payload(), Some(("AC3", 1)));
        assert_eq!(AudioFeed::decide("aac", 2, &c).payload(), Some(("AAC", 3)));
        for codec in ["truehd", "flac", "opus", "mp3", "pcm_s16le"] {
            assert!(
                matches!(AudioFeed::decide(codec, 2, &c), AudioFeed::NeedsSoftwareDecode { .. }),
                "{codec}"
            );
            assert_eq!(AudioFeed::decide(codec, 2, &c).payload(), None, "{codec} is never declared");
        }
        assert_eq!(AudioFeed::decide("", 0, &c), AudioFeed::Silent);
    }

    #[test]
    fn dolby_vision_is_declared_only_where_it_is_affirmatively_supported() {
        let p5 = Dovi { present: true, profile: 5, bl_compat: 0, el_present: false };
        let p8 = Dovi { present: true, profile: 8, bl_compat: 1, el_present: false };
        let p7 = Dovi { present: true, profile: 7, bl_compat: 6, el_present: true };
        assert!(p5.presentation(true, DvCapability::Supported, true).declared().is_some());
        assert_eq!(p5.presentation(true, DvCapability::Unknown, true).refusal(), Some("no cross-compatible base layer"));
        assert_eq!(p8.presentation(true, DvCapability::Unsupported, true), DvPresentation::NotDv, "P8's base layer is HDR10");
        assert_eq!(p7.presentation(true, DvCapability::Supported, true).refusal(), Some("dual-layer"));
        assert_eq!(Dovi::NONE.presentation(true, DvCapability::Supported, true), DvPresentation::NotDv);
        assert!(p5.presentation(true, DvCapability::Supported, false).refusal().is_some(), "never declared over non-HEVC");
    }
}
