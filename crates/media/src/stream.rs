//! **What a stream IS, as the Load must declare it** — and the two decisions that follow from it
//! before a native object is built: how its Dolby Vision is presented, and whether its audio can
//! be fed to the television's decoder at all.
//!
//! LG's buffer-feed pipeline is told the codecs, the frame rate and the sink envelope in the Load
//! payload, BEFORE the first access unit is fed, and it builds its decoders from that declaration
//! rather than from the bitstream. So the facts here must be known at start, and the container is
//! the only authority (an addon's metadata is not, and Stremio gives none): unless the caller hands
//! in a declaration of its own with [`crate::session::PlaybackStart`], the player PROBES the stream
//! with the bundled demuxer first (`ff::probe`, run off-frame by `player::probe`) and derives the
//! declaration from what it read ([`derive_declaration`]). The demuxer then refuses a file that
//! disagrees with what was declared (`ff::demux`), because a Load that says one codec over another
//! codec's bitstream is a black screen with nothing in the log.

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
    /// **DTS the television cannot take, re-encoded on the CPU as AC-3** (`ff::transcode`): the
    /// demuxer decodes the DTS core with the bundled `dca` decoder, keeps at most 5.1 and feeds
    /// 640 kb/s AC-3, and the Load declares `"AC3"` — the audio path the Starfish feed is proven
    /// on. Chosen for DTS with no DTS row in the codec table or more channels than its DTS ceiling,
    /// when the bundled libraries carry both the decoder and the encoder (runtime-detected) and
    /// the table lists AC-3.
    TranscodeAc3 { source: String, channels: i64 },
    /// **The television cannot decode this track** — a codec the pipeline has no decoder for
    /// (TrueHD, FLAC, Opus, ...), a listed codec beyond its measured channel ceiling, or DTS where
    /// the software path above is not available. Nothing else re-encodes it: a start that reaches
    /// this decision is refused with an explicit error (`player::FailureKind::AudioUnsupported`)
    /// instead of a Load whose audio clock never starts.
    NeedsSoftwareDecode { codec: String, channels: i64 },
}

/// The software audio paths this build can run, as the feed decision needs them: today one, DTS
/// decoded on the CPU and re-encoded as AC-3. `ff::transcode::available()` answers it at run time
/// (the bundled libavcodec carries the `dca` decoder and the `ac3` encoder); a test passes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoftwareAudio {
    pub dts_to_ac3: bool,
}

impl SoftwareAudio {
    /// No software path: the television's own decoders only.
    pub const NONE: SoftwareAudio = SoftwareAudio { dts_to_ac3: false };

    /// What the bundled FFmpeg on this device can do, as detected at `ff::boot`.
    pub fn detected() -> SoftwareAudio {
        SoftwareAudio { dts_to_ac3: crate::ff::transcode::available() }
    }
}

/// The most channels the DTS -> AC-3 path produces: AC-3 carries at most 5.1, so a wider core is
/// reduced to it (`ff::transcode`).
pub const TRANSCODE_MAX_CHANNELS: i64 = 6;

impl AudioFeed {
    /// The decision for `codec` with `channels` on a set whose table is `caps`, with the software
    /// paths `sw` this build can run.
    pub fn decide(codec: &str, channels: i64, caps: &sn_platform::devcaps::Caps, sw: SoftwareAudio) -> AudioFeed {
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
            // The television will not take this DTS track (no DTS row, or past its ceiling). The
            // transcoder's output is at most 5.1, so that is what the AC-3 row must admit; an
            // unknown channel count is the decoder's to find out, and 5.1 is the most it can be.
            _ if codec == "dts"
                && sw.dts_to_ac3
                && caps.audio_supports("ac3", channels.clamp(1, TRANSCODE_MAX_CHANNELS)) =>
            {
                AudioFeed::TranscodeAc3 { source: codec, channels }
            }
            _ => AudioFeed::NeedsSoftwareDecode { codec, channels },
        }
    }

    /// How good a feed this is, for choosing among a file's audio tracks: 0 the television
    /// decodes it as demuxed (or its DTS core), 1 it plays through the software path, 2 it cannot
    /// play at all. `Silent` is not a track.
    pub fn rank(&self) -> u8 {
        match self {
            AudioFeed::Native { .. } | AudioFeed::DtsCore | AudioFeed::Silent => 0,
            AudioFeed::TranscodeAc3 { .. } => 1,
            AudioFeed::NeedsSoftwareDecode { .. } => 2,
        }
    }

    /// The Load payload's audio codec name and the diagnostics code, when the television decodes
    /// the track.
    pub fn payload(&self) -> Option<(&'static str, u8)> {
        match self {
            AudioFeed::Native { payload, diag } => Some((payload, *diag)),
            AudioFeed::DtsCore => Some(("DTS", 4)),
            // The pipeline is fed AC-3 and told so; the diag code (5) keeps the read-out honest
            // about where that AC-3 came from.
            AudioFeed::TranscodeAc3 { .. } => Some(("AC3", 5)),
            AudioFeed::Silent | AudioFeed::NeedsSoftwareDecode { .. } => None,
        }
    }
}

/// **What the probe read off the container** (`ff::probe`): every video and audio stream, as
/// plain data, in FILE order — the order `PlaybackStart::audio_track` and the demuxer's
/// `desired_audio_idx` count audio streams in. Filled by the FFmpeg side, consumed by the pure
/// [`derive_declaration`], so the decision is testable without a demuxer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbedStreams {
    /// The video stream libavformat ranks best (`av_find_best_stream`), if any.
    pub video: Option<ProbedVideo>,
    /// Every audio stream, file order.
    pub audio: Vec<ProbedAudio>,
    /// The container's duration, nanoseconds; 0 when it does not say.
    pub duration_ns: i64,
}

/// One probed video stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbedVideo {
    /// FFmpeg's codec name (`avcodec_get_name`): `h264`, `hevc`, `av1`, `vp9`, ...
    pub codec: String,
    pub width: i32,
    pub height: i32,
    /// `AVStream.avg_frame_rate`, else `r_frame_rate`; 0.0 when neither is known.
    pub fps: f64,
    /// The stream-level Dolby Vision configuration record, read as [`Dovi`].
    pub dovi: Dovi,
}

/// One probed audio stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbedAudio {
    /// FFmpeg's codec name: `aac`, `ac3`, `eac3`, `dts`, `truehd`, ...
    pub codec: String,
    /// `ch_layout.nb_channels`; 0 when unknown.
    pub channels: i64,
    pub sample_rate: i64,
    /// The container flags it as the default track (`AV_DISPOSITION_DEFAULT`).
    pub default: bool,
    /// E-AC3 JOC, read from `codecpar.profile`. Only a decoder sets that profile and the bundled
    /// build has no E-AC3 decoder, so a probe reports `false` today; a caller that knows better
    /// passes its own declaration.
    pub atmos: bool,
    /// The container's track name (`title`/`name` tag), for a track menu.
    pub name: String,
    /// The `language` tag, if any.
    pub language: String,
}

/// **Why a probe produced no declaration.** Each is an explicit refusal of the start
/// (`session::StartRefusal::Probe`), never a Load and never a spinner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeFailure {
    /// The bundled FFmpeg is not loaded (or is not the ABI this build was made for), or the probe
    /// thread could not be started.
    Unavailable,
    /// The stream would not open: transport, HTTP status, or a container libavformat rejects.
    Open,
    /// The container opened but its streams could not be read (`avformat_find_stream_info`).
    StreamInfo,
    /// The stream carries no video track.
    NoVideo,
    /// The probe did not finish inside `player::probe::PROBE_BUDGET`.
    Timeout,
}

/// PURE: **the declaration a probed stream is Loaded with**, and the 0-based audio stream to feed
/// (`None`: no audio track at all).
///
/// The audio track is the caller's `audio_track` when the file has it. Otherwise it is chosen,
/// in this order: the best FEED (`feed_of(track).rank()`: a codec the television decodes, then one
/// the software path plays, then one nothing can play), then the track the container flags as
/// default, then file order. "Default first" alone would pick a TrueHD track over the AC-3 one
/// beside it and refuse a file that plays; "first decodable" alone would ignore the muxer's
/// choice of language among equally playable tracks.
pub fn derive_declaration(
    probed: &ProbedStreams,
    audio_track: Option<i32>,
    feed_of: impl Fn(&ProbedAudio) -> AudioFeed,
) -> Result<(StreamDeclaration, Option<i32>), ProbeFailure> {
    let video = probed.video.as_ref().ok_or(ProbeFailure::NoVideo)?;
    let axis = |v: i32| u16::try_from(v.max(0)).unwrap_or(u16::MAX);
    let explicit = audio_track
        .and_then(|t| usize::try_from(t).ok())
        .filter(|&t| t < probed.audio.len());
    let chosen = explicit.or_else(|| {
        probed
            .audio
            .iter()
            .enumerate()
            .min_by_key(|(i, a)| (feed_of(a).rank(), !a.default, *i))
            .map(|(i, _)| i)
    });
    let mut decl = StreamDeclaration {
        vcodec: video.codec.to_ascii_lowercase(),
        fps: if video.fps.is_finite() && video.fps > 0.0 { video.fps } else { 0.0 },
        raster: (axis(video.width), axis(video.height)),
        dovi: video.dovi,
        ..StreamDeclaration::default()
    };
    if let Some(a) = chosen.map(|i| &probed.audio[i]) {
        decl.acodec = a.codec.to_ascii_lowercase();
        decl.audio_channels = a.channels.max(0);
        decl.atmos = a.atmos;
    }
    Ok((decl, chosen.map(|i| i as i32)))
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
        assert_eq!(AudioFeed::decide("dts", 6, &six, SoftwareAudio::NONE), AudioFeed::DtsCore);
        assert_eq!(AudioFeed::decide("DTS", 2, &six, SoftwareAudio::NONE), AudioFeed::DtsCore, "codec names are case-insensitive");
        assert_eq!(
            AudioFeed::decide("dts", 8, &six, SoftwareAudio::NONE),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 8 },
            "a 7.1 track past a 5.1 ceiling is not the decoder's"
        );
        assert_eq!(
            AudioFeed::decide("dts", 0, &six, SoftwareAudio::NONE),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 0 },
            "an unknown channel count never fits a measured ceiling"
        );
        assert_eq!(
            AudioFeed::decide("dts", 6, &caps_with_dts(None), SoftwareAudio::NONE),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 6 },
            "DTS listed without a measured ceiling is not admitted"
        );
        assert_eq!(
            AudioFeed::decide("dts", 6, &Caps::assumed(), SoftwareAudio::NONE),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 6 },
            "a table that lists no DTS decoder"
        );
        assert_eq!(AudioFeed::DtsCore.payload(), Some(("DTS", 4)));
    }

    #[test]
    fn the_pipelines_own_codecs_are_native_and_everything_else_needs_a_software_decoder() {
        let c = Caps::assumed();
        assert_eq!(AudioFeed::decide("eac3", 6, &c, SoftwareAudio::NONE).payload(), Some(("AC3 PLUS", 2)));
        assert_eq!(AudioFeed::decide("ac3", 6, &c, SoftwareAudio::NONE).payload(), Some(("AC3", 1)));
        assert_eq!(AudioFeed::decide("aac", 2, &c, SoftwareAudio::NONE).payload(), Some(("AAC", 3)));
        for codec in ["truehd", "flac", "opus", "mp3", "pcm_s16le"] {
            assert!(
                matches!(AudioFeed::decide(codec, 2, &c, SoftwareAudio::NONE), AudioFeed::NeedsSoftwareDecode { .. }),
                "{codec}"
            );
            assert_eq!(AudioFeed::decide(codec, 2, &c, SoftwareAudio::NONE).payload(), None, "{codec} is never declared");
        }
        assert_eq!(AudioFeed::decide("", 0, &c, SoftwareAudio::NONE), AudioFeed::Silent);
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

    const TRANSCODER: SoftwareAudio = SoftwareAudio { dts_to_ac3: true };

    /// The feed decision matrix for DTS with the software path in the build: within the
    /// television's ceiling it is still fed as its core (the TV decodes it); past the ceiling, or
    /// with no DTS row at all, it is transcoded to AC-3; without the software path it is refused.
    #[test]
    fn dts_past_the_ceiling_is_transcoded_to_ac3_when_the_build_can_and_refused_when_it_cannot() {
        let six = caps_with_dts(Some(6));
        assert_eq!(AudioFeed::decide("dts", 6, &six, TRANSCODER), AudioFeed::DtsCore, "within the ceiling: the TV decodes the core");
        assert_eq!(
            AudioFeed::decide("dts", 8, &six, TRANSCODER),
            AudioFeed::TranscodeAc3 { source: "dts".into(), channels: 8 },
            "7.1 past a 5.1 ceiling: decoded on the CPU and fed as (at most 5.1) AC-3"
        );
        assert_eq!(
            AudioFeed::decide("dts", 8, &six, SoftwareAudio::NONE),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 8 },
            "the same track without the dca decoder / ac3 encoder in the build is refused"
        );
        assert_eq!(
            AudioFeed::decide("DTS", 6, &Caps::assumed(), TRANSCODER),
            AudioFeed::TranscodeAc3 { source: "dts".into(), channels: 6 },
            "a table with no DTS row at all"
        );
        assert_eq!(
            AudioFeed::decide("dts", 0, &caps_with_dts(None), TRANSCODER),
            AudioFeed::TranscodeAc3 { source: "dts".into(), channels: 0 },
            "an unknown channel count is the decoder's to find; the output is at most 5.1"
        );
        let mut no_ac3 = Caps::assumed();
        no_ac3.audio = "aac".into();
        assert_eq!(
            AudioFeed::decide("dts", 6, &no_ac3, TRANSCODER),
            AudioFeed::NeedsSoftwareDecode { codec: "dts".into(), channels: 6 },
            "the transcoder's AC-3 still needs an AC-3 row"
        );
        let mut ac3_stereo = Caps::assumed();
        ac3_stereo.audio_channels.insert("ac3".into(), 2);
        assert!(
            matches!(AudioFeed::decide("dts", 6, &ac3_stereo, TRANSCODER), AudioFeed::NeedsSoftwareDecode { .. }),
            "a 5.1 transcode past a measured AC-3 ceiling of 2 is not admitted"
        );
        for codec in ["truehd", "flac", "opus"] {
            assert!(
                matches!(AudioFeed::decide(codec, 2, &Caps::assumed(), TRANSCODER), AudioFeed::NeedsSoftwareDecode { .. }),
                "{codec}: the software path is DTS only"
            );
        }
        let t = AudioFeed::TranscodeAc3 { source: "dts".into(), channels: 8 };
        assert_eq!(t.payload(), Some(("AC3", 5)), "the Load declares AC3, the read-out says it came from DTS");
        assert_eq!((AudioFeed::DtsCore.rank(), t.rank(), AudioFeed::NeedsSoftwareDecode { codec: "x".into(), channels: 2 }.rank()), (0, 1, 2));
    }

    fn probed(video: Option<&str>, audio: &[(&str, i64, bool)]) -> ProbedStreams {
        ProbedStreams {
            video: video.map(|c| ProbedVideo {
                codec: c.into(),
                width: 3840,
                height: 2160,
                fps: 24000.0 / 1001.0,
                dovi: Dovi { present: true, profile: 8, bl_compat: 1, el_present: false },
            }),
            audio: audio
                .iter()
                .map(|&(codec, channels, default)| ProbedAudio {
                    codec: codec.into(),
                    channels,
                    sample_rate: 48_000,
                    default,
                    ..ProbedAudio::default()
                })
                .collect(),
            duration_ns: 0,
        }
    }

    fn feed(sw: SoftwareAudio) -> impl Fn(&ProbedAudio) -> AudioFeed {
        let caps = caps_with_dts(Some(6));
        move |a: &ProbedAudio| AudioFeed::decide(&a.codec, a.channels, &caps, sw)
    }

    /// The declaration is the probed video stream's codec, raster, frame rate and Dolby Vision
    /// record, plus the chosen audio track's codec and channel count.
    #[test]
    fn the_declaration_is_derived_from_the_probed_streams() {
        let p = probed(Some("HEVC"), &[("eac3", 6, true)]);
        let (d, track) = derive_declaration(&p, None, feed(SoftwareAudio::NONE)).unwrap();
        assert_eq!(track, Some(0));
        assert_eq!(
            d,
            StreamDeclaration {
                vcodec: "hevc".into(),
                acodec: "eac3".into(),
                audio_channels: 6,
                fps: 24000.0 / 1001.0,
                raster: (3840, 2160),
                dovi: Dovi { present: true, profile: 8, bl_compat: 1, el_present: false },
                atmos: false,
            }
        );
        let (silent, none) = derive_declaration(&probed(Some("h264"), &[]), None, feed(SoftwareAudio::NONE)).unwrap();
        assert_eq!((silent.acodec.as_str(), none), ("", None), "no audio stream: a video-only declaration");
        assert_eq!(
            derive_declaration(&probed(None, &[("aac", 2, true)]), None, feed(SoftwareAudio::NONE)),
            Err(ProbeFailure::NoVideo),
            "an audio-only stream is an explicit refusal, not a Load"
        );
        let mut odd = probed(Some("h264"), &[]);
        odd.video.as_mut().unwrap().fps = f64::NAN;
        odd.video.as_mut().unwrap().width = 70_000;
        let (d, _) = derive_declaration(&odd, None, feed(SoftwareAudio::NONE)).unwrap();
        assert_eq!((d.fps, d.raster.0), (0.0, u16::MAX), "an unreadable rate is unknown, a huge raster saturates");
    }

    /// The audio track: the caller's when the file has it, else the best feed, then the
    /// container's default, then file order.
    #[test]
    fn the_audio_track_prefers_a_playable_feed_then_the_default_then_file_order() {
        let p = probed(Some("hevc"), &[("truehd", 8, true), ("dts", 8, false), ("ac3", 6, false), ("ac3", 2, true)]);
        let pick = |p: &ProbedStreams, t: Option<i32>, sw| derive_declaration(p, t, feed(sw)).unwrap();
        assert_eq!(pick(&p, None, SoftwareAudio::NONE).1, Some(3), "a playable default beats a playable non-default");
        let (d, t) = pick(&p, Some(1), TRANSCODER);
        assert_eq!((t, d.acodec.as_str(), d.audio_channels), (Some(1), "dts", 8), "the caller's pick is kept, even if it is not the best");
        assert_eq!(pick(&p, Some(9), SoftwareAudio::NONE).1, Some(3), "a pick past the last track falls back to the automatic choice");
        let dts_only = probed(Some("hevc"), &[("truehd", 8, true), ("dts", 8, false)]);
        assert_eq!(pick(&dts_only, None, TRANSCODER).1, Some(1), "the transcodable DTS beats the unplayable default");
        assert_eq!(pick(&dts_only, None, SoftwareAudio::NONE).1, Some(0), "nothing plays: the default, which the session then refuses");
    }
}
