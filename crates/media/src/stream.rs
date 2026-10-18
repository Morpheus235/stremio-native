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
    /// That track's sample rate in Hz; 0 when unknown.
    pub audio_sample_rate: i64,
    /// The video frame rate; 0.0 when unknown.
    pub fps: f64,
    /// The coded raster `(width, height)`; 0 on an axis nobody knows.
    pub raster: (u16, u16),
    /// The video's FFmpeg profile id (`AV_PROFILE_*`); -99 (FFmpeg's unknown) or 0 when not read.
    pub vprofile: i32,
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
    /// - `el_present` (or Profile 7) → [`Self::presentation_with`]'s dual-layer rule: the base layer
    ///   alone, as HDR10, by default;
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
        self.presentation_with(signal, capability, video_is_hevc, crate::prefs::DvProfile7::Hdr10)
    }

    /// [`Self::presentation`] with the viewer's Profile 7 choice. **A dual-layer stream (Profile 7,
    /// or anything with an enhancement layer)** is no longer refused:
    ///
    /// - by default it plays its BASE LAYER only ([`DvPresentation::BaseLayer`]): the demuxer drops
    ///   every RPU (NAL 62) and enhancement-layer (NAL 63) unit (`dovi::P81Feed`) and the Load
    ///   declares plain HEVC. The base layer's own VUI and SEI carry its transfer (PQ for P7's
    ///   compatibility id 6 and for 1, SDR for 2, HLG for 4), which the decoder reads in-band; there
    ///   is no payload key for it to repeat. An enhancement layer over a base layer with no
    ///   compatible signal (id 0) has nothing to fall back to and is refused.
    /// - with [`crate::prefs::DvProfile7::TryNative`], on a set that answers Dolby Vision, it is
    ///   CONVERTED to single-layer Profile 8.1 in the feed ([`DvPresentation::ConvertTo81`]: the
    ///   enhancement layer dropped, each RPU rewritten by [`crate::dovi`]) and declared exactly
    ///   like a native 8.1 stream; `player::pump` still watches it and falls back to the base
    ///   layer if the set shows no frame ([`DvPresentation::is_p7_trial`]).
    pub fn presentation_with(
        &self,
        signal: bool,
        capability: sn_platform::devcaps::dv::DvCapability,
        video_is_hevc: bool,
        p7: crate::prefs::DvProfile7,
    ) -> DvPresentation {
        if !self.present {
            return DvPresentation::NotDv;
        }
        if self.el_present || self.profile == 7 {
            // Only Profile 7 converts: its RPU is what `dovi::rpu_to_p81` rewrites.
            if p7 == crate::prefs::DvProfile7::TryNative
                && self.profile == 7
                && signal
                && video_is_hevc
                && capability == sn_platform::devcaps::dv::DvCapability::Supported
            {
                return DvPresentation::ConvertTo81;
            }
            // P7's base layer is HDR10 by definition (id 6), even where a muxer wrote 0.
            let base_usable = self.profile == 7 || matches!(self.bl_compat, 1 | 2 | 4 | 6);
            return if base_usable && video_is_hevc { DvPresentation::BaseLayer } else { DvPresentation::Refuse("dual-layer") };
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
        self.presentation_with(!dv_withheld(), sn_platform::devcaps::dv::capability(), video_is_hevc, crate::prefs::dv_profile7())
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
    /// A dual-layer (Profile 7) stream converted in the feed to single-layer Profile 8.1: the
    /// demuxer drops the enhancement layer and rewrites every RPU (`crate::dovi::P81Feed`), and the
    /// Load declares it as [`DvPresentation::P81`], exactly like a native 8.1 stream.
    ConvertTo81,
    /// A dual-layer stream played as its base layer alone: RPU and enhancement-layer NAL units are
    /// dropped by the demuxer and nothing Dolby is declared (Profile 7 as HDR10).
    BaseLayer,
    /// The base layer alone is a wrong picture and the node cannot be sent: refuse the start.
    Refuse(&'static str),
}

impl DvPresentation {
    /// The node a native Profile 8.1 stream is declared with, and so a converted Profile 7 one.
    pub const P81: DolbyHdrInfo = DolbyHdrInfo { profile_id: 8, track_type: "single", encryption_type: "clear" };

    pub const fn label(&self) -> &'static str {
        match self {
            Self::NotDv => "base-layer",
            Self::Declare(_) | Self::ConvertTo81 => "declare",
            Self::BaseLayer => "base-layer-only",
            Self::Refuse(_) => "refuse",
        }
    }

    /// The Profile 7 → 8.1 conversion, which the pump falls back from.
    pub fn is_p7_trial(&self) -> bool {
        matches!(self, Self::ConvertTo81)
    }

    /// The demuxer must convert Profile 7 to 8.1 (`crate::dovi::P81Feed`).
    pub fn converts_p7(&self) -> bool {
        matches!(self, Self::ConvertTo81)
    }

    /// The demuxer must drop the RPU and enhancement-layer NAL units.
    pub fn strips_layers(&self) -> bool {
        matches!(self, Self::BaseLayer)
    }

    /// The node to splice into the Load payload, if any.
    pub fn declared(&self) -> Option<DolbyHdrInfo> {
        match self {
            Self::Declare(d) => Some(*d),
            Self::ConvertTo81 => Some(Self::P81),
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
    /// **Decoded on the CPU and re-encoded as AC-3 5.1** (`ff::transcode::ToAc3`): the demuxer
    /// decodes the track with the bundled libavcodec, keeps at most 5.1 and feeds 640 kb/s AC-3,
    /// and the Load declares `"AC3"` — the audio path the Starfish feed is proven on, and what an
    /// optical or plain ARC link carries as 5.1. Chosen for DTS with no DTS row in the codec table
    /// or more channels than its DTS ceiling, and for a codec the pipeline has no name for
    /// (TrueHD, FLAC, LPCM, Opus, ...) with more than two channels unless
    /// [`crate::prefs::Multichannel`] asks for PCM — when the bundled libraries carry that
    /// decoder and the `ac3` encoder (runtime-detected), the table lists AC-3 and the source rate
    /// is one AC-3 carries (48/44.1/32 kHz; another rate is decoded to PCM instead).
    TranscodeAc3 { source: String, channels: i64 },
    /// **Decoded on the CPU and fed as PCM** (`ff::adec`, ported from PlxNative): FLAC, TrueHD/MLP,
    /// LPCM, Opus, Vorbis, MP2, WMA, ALAC — a codec the pipeline has no name for — and MP3 on a set
    /// without an MPEG audio row, decoded by the
    /// bundled libavcodec and resampled to S16LE 48 kHz, `channels` of them (2, or 6 for a source
    /// of 5.1 and up); the Load declares `"PCM"` with that `pcmInfo`. Chosen when `libsnadec.so`
    /// loaded at boot and carries the codec's decoder — for more than two channels only where
    /// [`TranscodeAc3`](Self::TranscodeAc3) is not chosen. `dplii`: any track of more than two
    /// channels (the television's own codecs too) folded to a Dolby Pro Logic II matrixed stereo
    /// pair ([`crate::prefs::Multichannel::StereoDplii`]); `channels` is then 2. With
    /// [`crate::prefs::Multichannel::Stereo`] the same tracks are `channels: 2, dplii: false`.
    SoftPcm { source: String, channels: u32, dplii: bool },
    /// **The television cannot decode this track** — a codec the pipeline has no decoder for and
    /// the build cannot decode either (or TrueHD, FLAC, Opus, ... without `libsnadec.so`), a listed
    /// codec beyond its measured channel ceiling, or DTS where the software path above is not
    /// available. Nothing else re-encodes it: a start that reaches
    /// this decision is refused with an explicit error (`player::FailureKind::AudioUnsupported`)
    /// instead of a Load whose audio clock never starts.
    NeedsSoftwareDecode { codec: String, channels: i64 },
}

/// The codecs the bundled libavcodec may decode for the software paths, by FFmpeg's codec name and
/// its decoder's name; [`SoftwareAudio::decoders`] holds one bit per entry (bit 0 = DTS).
pub const DECODERS: [(&str, &str); 18] = [
    ("dts", "dca"),
    ("truehd", "truehd"),
    ("mlp", "mlp"),
    ("flac", "flac"),
    ("opus", "opus"),
    ("vorbis", "vorbis"),
    ("alac", "alac"),
    ("wmapro", "wmapro"),
    ("pcm_s16le", "pcm_s16le"),
    ("pcm_s16be", "pcm_s16be"),
    ("pcm_s24le", "pcm_s24le"),
    ("pcm_s24be", "pcm_s24be"),
    ("pcm_s32le", "pcm_s32le"),
    ("pcm_f32le", "pcm_f32le"),
    ("pcm_bluray", "pcm_bluray"),
    ("pcm_dvd", "pcm_dvd"),
    ("ac3", "ac3"),
    ("eac3", "eac3"),
];

/// The software audio paths this build can run, as the feed decision needs them: decode ->
/// AC-3 (`ff::transcode::available`: the bundled `ac3` encoder, and per codec its decoder) and
/// decode -> PCM (`ff::adec::available()`), and what the viewer asked a >2-channel track to
/// become (`prefs::multichannel`); a test passes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoftwareAudio {
    /// The bundled libavcodec's `ac3` encoder: the AC-3 path's encoding half.
    pub ac3_encoder: bool,
    /// The software decode -> PCM path (`ff::adec`): `libsnadec.so` loaded.
    pub pcm: bool,
    /// The [`DECODERS`] the bundled libavcodec carries, one bit each.
    pub decoders: u32,
    /// What a track of more than two channels decoded here becomes.
    pub multichannel: crate::prefs::Multichannel,
}

impl SoftwareAudio {
    /// No software path: the television's own decoders only.
    pub const NONE: SoftwareAudio =
        SoftwareAudio { ac3_encoder: false, pcm: false, decoders: 0, multichannel: crate::prefs::Multichannel::Ac3 };

    /// What the bundled FFmpeg on this device can do, as detected at `ff::boot`, and the viewer's
    /// multichannel choice for this start.
    pub fn detected() -> SoftwareAudio {
        SoftwareAudio {
            ac3_encoder: crate::ff::transcode::available(),
            pcm: crate::ff::adec::available(),
            decoders: crate::ff::transcode::decoders(),
            multichannel: crate::prefs::multichannel(),
        }
    }

    /// Does the bundled libavcodec decode `codec` (lowercase FFmpeg name)?
    pub fn decodes(&self, codec: &str) -> bool {
        DECODERS.iter().position(|(c, _)| *c == codec).is_some_and(|i| self.decoders & (1 << i) != 0)
    }

    /// Can `codec` be decoded here and re-encoded as AC-3?
    pub fn to_ac3(&self, codec: &str) -> bool {
        self.ac3_encoder && self.decodes(codec)
    }
}

/// The most channels the AC-3 path produces: AC-3 carries at most 5.1, so a wider source is
/// reduced to it (`ff::transcode`).
pub const TRANSCODE_MAX_CHANNELS: i64 = 6;

impl AudioFeed {
    /// The decision for `codec` with `channels` on a set whose table is `caps`, with the software
    /// paths `sw` this build can run; the sample rate unknown ([`decide_at`](Self::decide_at)).
    pub fn decide(codec: &str, channels: i64, caps: &sn_platform::devcaps::Caps, sw: SoftwareAudio) -> AudioFeed {
        AudioFeed::decide_at(codec, channels, 0, caps, sw)
    }

    /// [`decide`](Self::decide) knowing the track's sample rate (`rate`, Hz; 0 unknown): a rate
    /// AC-3 cannot carry keeps a multichannel track on PCM rather than failing the transcoder's
    /// open (the DTS core is 48 kHz in practice and keeps its old rule: the open finds out).
    pub fn decide_at(codec: &str, channels: i64, rate: i64, caps: &sn_platform::devcaps::Caps, sw: SoftwareAudio) -> AudioFeed {
        use crate::prefs::Multichannel;
        let codec = codec.to_ascii_lowercase();
        if codec.is_empty() {
            return AudioFeed::Silent;
        }
        let soft = crate::ff::adec::is_soft(&codec);
        // Stereo / Pro Logic II: every track of more than two channels the build can decode, the
        // television's own codecs too, folded to a stereo pair here (plain, or DPLII matrixed).
        let fold = matches!(sw.multichannel, Multichannel::Stereo | Multichannel::StereoDplii);
        if fold && channels > 2 && sw.pcm && (soft || sw.decodes(&codec)) {
            return AudioFeed::SoftPcm { source: codec, channels: 2, dplii: sw.multichannel == Multichannel::StereoDplii };
        }
        // LG's buffer-feed vocabulary (libpf `CustomPipeline::parseOptionStringSpi`); the diag
        // code is `SHARED.dg_load_a`'s. Anything else has no name the pipeline accepts.
        let named = match codec.as_str() {
            "ac3" => Some(("AC3", 1)),
            "eac3" => Some(("AC3 PLUS", 2)),
            "aac" => Some(("AAC", 3)),
            "dts" => Some(("DTS", 4)),
            "mp3" => Some(("MP3", 7)),
            "ac4" => Some(("AC4", 8)),
            _ => None,
        };
        // A codec the pipeline has no name for, more than two channels, as AC-3 5.1 unless the
        // viewer (or the route, for Auto) asked for PCM. Stereo stays PCM: any link carries it.
        let ac3_rate = rate == 0 || crate::ff::transcode::AC3_RATES.contains(&(rate as i32));
        let soft_ac3 = soft
            && channels > 2
            && sw.multichannel != Multichannel::Pcm
            && ac3_rate
            && sw.to_ac3(&codec)
            && caps.audio_supports("ac3", channels.min(TRANSCODE_MAX_CHANNELS));
        match named {
            Some(_) if codec == "dts" && caps.audio_supports("dts", channels) => AudioFeed::DtsCore,
            // AC-4 has no table row (and no channel count without a decoder): the firmware rule.
            Some((payload, diag)) if codec == "ac4" && caps.ac4 => AudioFeed::Native { payload, diag },
            Some((payload, diag)) if codec != "dts" && caps.audio_supports(&codec, channels) => {
                AudioFeed::Native { payload, diag }
            }
            // The television will not take this DTS track (no DTS row, or past its ceiling). The
            // transcoder's output is at most 5.1, so that is what the AC-3 row must admit; an
            // unknown channel count is the decoder's to find out, and 5.1 is the most it can be.
            _ if codec == "dts"
                && sw.to_ac3("dts")
                && caps.audio_supports("ac3", channels.clamp(1, TRANSCODE_MAX_CHANNELS)) =>
            {
                AudioFeed::TranscodeAc3 { source: codec, channels }
            }
            _ if soft_ac3 => AudioFeed::TranscodeAc3 { source: codec, channels },
            // A codec the pipeline has no name for, decoded here and fed as PCM. DTS keeps its
            // own paths above (PlxNative never routed DTS through PCM either).
            // MP3 on a set without the MPEG row lands here too.
            _ if codec != "dts" && sw.pcm && soft => {
                AudioFeed::SoftPcm { channels: crate::ff::adec::sink_channels(channels), source: codec, dplii: false }
            }
            _ => AudioFeed::NeedsSoftwareDecode { codec, channels },
        }
    }

    /// What the track becomes on its way to the television, for a track menu: `""` as it is (or
    /// its DTS core), else the format it is fed as.
    pub fn fed_as(&self) -> &'static str {
        match self {
            AudioFeed::TranscodeAc3 { .. } => "Dolby Digital",
            AudioFeed::SoftPcm { dplii: true, .. } => "Pro Logic II",
            AudioFeed::SoftPcm { .. } => "PCM",
            _ => "",
        }
    }

    /// How good a feed this is, for choosing among a file's audio tracks: 0 the television
    /// decodes it as demuxed (or its DTS core), 1 it plays through the software path, 2 it cannot
    /// play at all. `Silent` is not a track.
    pub fn rank(&self) -> u8 {
        match self {
            AudioFeed::Native { .. } | AudioFeed::DtsCore | AudioFeed::Silent => 0,
            AudioFeed::TranscodeAc3 { .. } | AudioFeed::SoftPcm { .. } => 1,
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
            AudioFeed::SoftPcm { .. } => Some(("PCM", 6)),
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
    /// Every subtitle stream, file order — the order the demuxer's `desired_sub_idx` counts in
    /// (`ff_subs::SubTracks`), so the start can pick one before the Load.
    pub subs: Vec<ProbedSub>,
    /// The container's duration, nanoseconds; 0 when it does not say.
    pub duration_ns: i64,
    /// Where the container's timeline starts, nanoseconds; 0 when it does not say. A file starts
    /// at ~0; a live MPEG-TS joined mid-stream starts wherever its encoder's clock stands.
    pub start_ns: i64,
}

/// Does a fresh start need its first keyframe rebased to fed-pts 0? The TV's buffer-feed
/// pipeline starts its clock at 0 on every Load; a stream whose timeline starts far from 0 (a live
/// channel at ~3581 s) is fed "an hour in the future" otherwise and is never presented.
pub fn needs_fresh_rebase(start_ns: i64) -> bool {
    start_ns > FRESH_REBASE_AFTER_NS
}

/// Timelines that start later than this are rebased (files start within a second or two).
pub const FRESH_REBASE_AFTER_NS: i64 = 10_000_000_000;

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
    /// FFmpeg's profile id for the stream (`AVCodecParameters.profile`); -99 when unknown.
    pub profile: i32,
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
    /// E-AC3 JOC (Dolby Atmos): `codecpar.profile`, which only the E-AC-3 decoder sets and the
    /// bundled build does not carry, else the probe's own read of the first frame's `addbsi`
    /// (`ff::eac3_joc`).
    pub atmos: bool,
    /// The container's track name (`title`/`name` tag), for a track menu.
    pub name: String,
    /// The `language` tag, if any.
    pub language: String,
    /// The container flags it forced (`AV_DISPOSITION_FORCED`).
    pub forced: bool,
}

/// One probed subtitle stream: what the start's language pick reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbedSub {
    /// The `language` tag, if any.
    pub language: String,
    /// The container's track name (`title`/`name` tag).
    pub name: String,
    /// `AV_DISPOSITION_DEFAULT`.
    pub default: bool,
    /// `AV_DISPOSITION_FORCED`, or "forced" in its name.
    pub forced: bool,
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
    /// The link answered something that is plainly not a video (`ff::probe`'s sniff of the first
    /// bytes): the addon picked the wrong file in a torrent, or the service answered a page.
    NotVideo(NotVideo),
}

/// **What a stream answered instead of a video**, told apart by its first bytes (or, when they
/// say nothing, its `Content-Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotVideo {
    /// PNG, JPEG, GIF, WebP: an image that sat in the torrent.
    Image,
    /// ZIP or RAR.
    Archive,
    /// JSON or plain text (an `.nfo`, a subtitle, a service's text answer).
    Document,
    /// An HTML page. The one kind worth the waiting retries: a debrid link that is not ready yet
    /// can answer a page for a moment.
    WebPage,
}

/// PURE: **the declaration a probed stream is Loaded with**, and the 0-based audio stream to feed
/// (`None`: no audio track at all).
///
/// The audio track is the caller's `audio_track` when the file has it. Otherwise it is chosen,
/// in this order: the best FEED (`feed_of(track).rank()`: a codec the television decodes, then one
/// the software path plays, then one nothing can play), then the track the container flags as
/// default, then an Atmos (E-AC-3 JOC) track, then file order. "Default first" alone would pick a TrueHD track over the AC-3 one
/// beside it and refuse a file that plays; "first decodable" alone would ignore the muxer's
/// choice of language among equally playable tracks.
///
/// With an audio language order (`prefs.audio`, Settings > Playback), a track's place in it comes
/// right after "can it play at all": a German track the software path plays beats an English one
/// the television decodes, but never one nothing can play (that would refuse a file that plays).
/// The pick is made here, before the Load, so the declaration (codec, channels, Atmos) is the
/// chosen track's and no reload follows.
pub fn derive_declaration(
    probed: &ProbedStreams,
    audio_track: Option<i32>,
    prefs: &sn_base::tracks::TrackPrefs,
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
            .min_by_key(|(i, a)| {
                let rank = feed_of(a).rank();
                (rank == 2, prefs.audio_rank(&a.language), rank, !a.default, !a.atmos, *i)
            })
            .map(|(i, _)| i)
    });
    let mut decl = StreamDeclaration {
        vcodec: video.codec.to_ascii_lowercase(),
        fps: if video.fps.is_finite() && video.fps > 0.0 { video.fps } else { 0.0 },
        raster: (axis(video.width), axis(video.height)),
        vprofile: video.profile,
        dovi: video.dovi,
        ..StreamDeclaration::default()
    };
    if let Some(a) = chosen.map(|i| &probed.audio[i]) {
        decl.acodec = a.codec.to_ascii_lowercase();
        decl.audio_channels = a.channels.max(0);
        decl.audio_sample_rate = a.sample_rate.max(0);
        decl.atmos = a.atmos;
    }
    Ok((decl, chosen.map(|i| i as i32)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_platform::devcaps::dv::DvCapability;
    use sn_platform::devcaps::Caps;
    use sn_base::tracks::TrackPrefs;

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
        assert_eq!(p7.presentation(true, DvCapability::Supported, true), DvPresentation::BaseLayer, "P7 plays as HDR10 by default");
        assert_eq!(Dovi::NONE.presentation(true, DvCapability::Supported, true), DvPresentation::NotDv);
        assert!(p5.presentation(true, DvCapability::Supported, false).refusal().is_some(), "never declared over non-HEVC");
    }

    /// Profile 7 (and any enhancement layer): the base layer alone by default, whatever the set
    /// answers, for every compatible base layer (6 HDR10 on Blu-ray, 1 HDR10, 2 SDR, 4 HLG); the
    /// Try setting converts it to Profile 8.1 only where Dolby Vision is affirmatively supported.
    #[test]
    fn profile_7_plays_its_base_layer_unless_the_viewer_converts_it_to_8_1() {
        use crate::prefs::DvProfile7::{Hdr10, TryNative};
        let dual = |profile, bl_compat| Dovi { present: true, profile, bl_compat, el_present: true };
        for compat in [6, 1, 2, 4] {
            let p = dual(7, compat).presentation_with(true, DvCapability::Supported, true, Hdr10);
            assert_eq!(p, DvPresentation::BaseLayer, "compat {compat}");
            assert!(p.strips_layers() && p.declared().is_none() && !p.is_p7_trial());
            assert_eq!(dual(7, compat).presentation_with(true, DvCapability::Unknown, true, TryNative), DvPresentation::BaseLayer);
        }
        assert_eq!(dual(7, 0).presentation(true, DvCapability::Supported, true), DvPresentation::BaseLayer, "P7's base layer is HDR10 by definition");
        assert_eq!(dual(4, 2).presentation(true, DvCapability::Unsupported, true), DvPresentation::BaseLayer, "an SDR base layer");
        assert_eq!(dual(9, 0).presentation(true, DvCapability::Supported, true).refusal(), Some("dual-layer"), "no compatible base layer");
        let tried = dual(7, 6).presentation_with(true, DvCapability::Supported, true, TryNative);
        assert_eq!(tried, DvPresentation::ConvertTo81, "Try Dolby Vision converts Profile 7 to 8.1");
        assert!(tried.is_p7_trial() && tried.converts_p7() && !tried.strips_layers());
        assert_eq!(tried.declared(), Some(DolbyHdrInfo { profile_id: 8, track_type: "single", encryption_type: "clear" }), "declared as native 8.1");
        assert_eq!(tried.label(), "declare");
        assert_eq!(dual(4, 2).presentation_with(true, DvCapability::Supported, true, TryNative), DvPresentation::BaseLayer, "only Profile 7 converts");
        assert_eq!(dual(7, 6).presentation_with(false, DvCapability::Supported, true, TryNative), DvPresentation::BaseLayer, "/tmp/stremionative-nodv withholds it");
        // the setting changes nothing for single-layer profiles
        let p8 = Dovi { present: true, profile: 8, bl_compat: 1, el_present: false };
        let p5 = Dovi { present: true, profile: 5, bl_compat: 0, el_present: false };
        for d in [p8, p5] {
            for cap in [DvCapability::Supported, DvCapability::Unknown] {
                assert_eq!(d.presentation_with(true, cap, true, TryNative), d.presentation(true, cap, true));
            }
        }
        assert_eq!(p8.presentation(true, DvCapability::Supported, true).declared().map(|d| d.profile_id), Some(8));
        let native = p8.presentation(true, DvCapability::Supported, true);
        assert_eq!(native.declared(), Some(DvPresentation::P81), "a converted P7 is declared exactly as this");
        assert!(!native.converts_p7() && !native.strips_layers(), "a Profile 8 stream is fed untouched");
    }

    /// The audio language order ranks right after "can it play at all": a German track beats the
    /// default English one, but a German track nothing can play never refuses a file that plays.
    #[test]
    fn the_language_order_ranks_after_playability() {
        let de = TrackPrefs { audio: vec!["DE".into(), "EN".into()], ..TrackPrefs::default() };
        let mut p = probed(Some("hevc"), &[("ac3", 6, true), ("eac3", 6, false)]);
        (p.audio[0].language, p.audio[1].language) = ("eng".into(), "ger".into());
        let (d, t) = derive_declaration(&p, None, &de, feed(SoftwareAudio::NONE)).unwrap();
        assert_eq!((t, d.acodec.as_str()), (Some(1), "eac3"), "declared for the German track");
        p.audio[1].codec = "truehd".into();
        assert_eq!(derive_declaration(&p, None, &de, feed(SoftwareAudio::NONE)).unwrap().1, Some(0), "unplayable German loses");
    }

    /// An E-AC-3 JOC (Atmos) track beats a TrueHD track the software path would decode to PCM
    /// (losing its Atmos), and among equally playable tracks Atmos wins after the muxer's default.
    #[test]
    fn an_atmos_track_is_preferred_over_soft_decoded_truehd() {
        let video = ProbedVideo { codec: "hevc".into(), width: 3840, height: 2160, fps: 24.0, dovi: Dovi::NONE, profile: -99 };
        let track = |codec: &str, atmos: bool, default: bool| ProbedAudio { codec: codec.into(), channels: 8, sample_rate: 48000, atmos, default, ..ProbedAudio::default() };
        let mut caps = Caps::assumed();
        caps.audio = "aac,ac3,eac3".into();
        let sw = SoftwareAudio { pcm: true, ..SoftwareAudio::NONE };
        let feed = |a: &ProbedAudio| AudioFeed::decide(&a.codec, a.channels, &caps, sw);
        let probed = ProbedStreams { video: Some(video.clone()), audio: vec![track("truehd", true, true), track("eac3", true, false)], ..ProbedStreams::default() };
        let (decl, t) = derive_declaration(&probed, None, &TrackPrefs::default(), feed).unwrap();
        assert_eq!((t, decl.acodec.as_str(), decl.atmos), (Some(1), "eac3", true));
        let probed = ProbedStreams { video: Some(video), audio: vec![track("ac3", false, false), track("eac3", true, false)], ..ProbedStreams::default() };
        assert_eq!(derive_declaration(&probed, None, &TrackPrefs::default(), feed).unwrap().1, Some(1), "Atmos before file order");
    }

    /// The AC-3 encoder and only the DTS decoder (bit 0).
    const TRANSCODER: SoftwareAudio = SoftwareAudio { ac3_encoder: true, decoders: 1, ..SoftwareAudio::NONE };

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
                "{codec}: stereo is never transcoded, and this build has no PCM path"
            );
        }
        let t = AudioFeed::TranscodeAc3 { source: "dts".into(), channels: 8 };
        assert_eq!(t.payload(), Some(("AC3", 5)), "the Load declares AC3, the read-out says it was transcoded");
        assert_eq!((AudioFeed::DtsCore.rank(), t.rank(), AudioFeed::NeedsSoftwareDecode { codec: "x".into(), channels: 2 }.rank()), (0, 1, 2));
    }

    /// A live channel's timeline starts wherever its encoder's clock stands (~3581 s in the beta's
    /// device log), and nothing was presented until it was rebased; a file's ~0 start never is.
    #[test]
    fn only_a_timeline_far_from_zero_is_rebased() {
        assert!(needs_fresh_rebase(3_581_400_000_000));
        assert!(!needs_fresh_rebase(0));
        assert!(!needs_fresh_rebase(1_400_000_000), "a file starting 1.4 s in is fed as it is");
        assert!(!needs_fresh_rebase(FRESH_REBASE_AFTER_NS));
    }

    fn probed(video: Option<&str>, audio: &[(&str, i64, bool)]) -> ProbedStreams {
        ProbedStreams {
            video: video.map(|c| ProbedVideo {
                codec: c.into(),
                width: 3840,
                height: 2160,
                fps: 24000.0 / 1001.0,
                dovi: Dovi { present: true, profile: 8, bl_compat: 1, el_present: false },
                profile: -99,
            }),
            start_ns: 0,
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
            ..ProbedStreams::default()
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
        let (d, track) = derive_declaration(&p, None, &TrackPrefs::default(), feed(SoftwareAudio::NONE)).unwrap();
        assert_eq!(track, Some(0));
        assert_eq!(
            d,
            StreamDeclaration {
                vcodec: "hevc".into(),
                acodec: "eac3".into(),
                audio_channels: 6,
                audio_sample_rate: 48_000,
                fps: 24000.0 / 1001.0,
                raster: (3840, 2160),
                vprofile: -99,
                dovi: Dovi { present: true, profile: 8, bl_compat: 1, el_present: false },
                atmos: false,
            }
        );
        let (silent, none) = derive_declaration(&probed(Some("h264"), &[]), None, &TrackPrefs::default(), feed(SoftwareAudio::NONE)).unwrap();
        assert_eq!((silent.acodec.as_str(), none), ("", None), "no audio stream: a video-only declaration");
        assert_eq!(
            derive_declaration(&probed(None, &[("aac", 2, true)]), None, &TrackPrefs::default(), feed(SoftwareAudio::NONE)),
            Err(ProbeFailure::NoVideo),
            "an audio-only stream is an explicit refusal, not a Load"
        );
        let mut odd = probed(Some("h264"), &[]);
        odd.video.as_mut().unwrap().fps = f64::NAN;
        odd.video.as_mut().unwrap().width = 70_000;
        let (d, _) = derive_declaration(&odd, None, &TrackPrefs::default(), feed(SoftwareAudio::NONE)).unwrap();
        assert_eq!((d.fps, d.raster.0), (0.0, u16::MAX), "an unreadable rate is unknown, a huge raster saturates");
    }

    /// The audio track: the caller's when the file has it, else the best feed, then the
    /// container's default, then file order.
    #[test]
    fn the_audio_track_prefers_a_playable_feed_then_the_default_then_file_order() {
        let p = probed(Some("hevc"), &[("truehd", 8, true), ("dts", 8, false), ("ac3", 6, false), ("ac3", 2, true)]);
        let pick = |p: &ProbedStreams, t: Option<i32>, sw| derive_declaration(p, t, &TrackPrefs::default(), feed(sw)).unwrap();
        assert_eq!(pick(&p, None, SoftwareAudio::NONE).1, Some(3), "a playable default beats a playable non-default");
        let (d, t) = pick(&p, Some(1), TRANSCODER);
        assert_eq!((t, d.acodec.as_str(), d.audio_channels), (Some(1), "dts", 8), "the caller's pick is kept, even if it is not the best");
        assert_eq!(pick(&p, Some(9), SoftwareAudio::NONE).1, Some(3), "a pick past the last track falls back to the automatic choice");
        let dts_only = probed(Some("hevc"), &[("truehd", 8, true), ("dts", 8, false)]);
        assert_eq!(pick(&dts_only, None, TRANSCODER).1, Some(1), "the transcodable DTS beats the unplayable default");
        assert_eq!(pick(&dts_only, None, SoftwareAudio::NONE).1, Some(0), "nothing plays: the default, which the session then refuses");
    }

    const PCM: SoftwareAudio = SoftwareAudio { ac3_encoder: true, pcm: true, decoders: 1, multichannel: crate::prefs::Multichannel::Ac3 };

    /// The C1 beta's log: `probed h264 ... audio=flac 2ch` then `start refused — AudioUnsupported
    /// { codec: "flac", channels: 2 }`. With the software decoder in the build, FLAC and the
    /// **MP3 and AC-4 go to the television's own decoders**, measured on an LG C1 (webOS 6.4):
    /// `"MP3"` played from Matroska, MP4, MPEG-TS and AVI, `"AC4"` from an ATSC 3.0 capture. MP3 is
    /// native where the table lists an `MPEG` audio row (read as `mp3`), AC-4 where `caps.ac4`
    /// says libpf has its caps; otherwise both keep their old answers (MP3 decoded to PCM, AC-4
    /// refused). MP2 declared as `"MP3"` presented nothing there, so it stays on the PCM path.
    #[test]
    fn mp3_and_ac4_are_native_where_the_set_takes_them() {
        let mut c1 = Caps::assumed();
        c1.audio = "aac,ac3,eac3,mp3".into();
        c1.audio_channels.insert("mp3".into(), 2);
        c1.ac4 = true;
        assert_eq!(AudioFeed::decide("mp3", 2, &c1, PCM), AudioFeed::Native { payload: "MP3", diag: 7 });
        assert_eq!(AudioFeed::decide("ac4", 0, &c1, PCM), AudioFeed::Native { payload: "AC4", diag: 8 });
        assert_eq!(AudioFeed::decide("mp2", 2, &c1, PCM), AudioFeed::SoftPcm { source: "mp2".into(), channels: 2, dplii: false });
        let old = Caps::assumed();
        assert_eq!(AudioFeed::decide("mp3", 2, &old, PCM), AudioFeed::SoftPcm { source: "mp3".into(), channels: 2, dplii: false });
        assert!(matches!(AudioFeed::decide("ac4", 0, &old, PCM), AudioFeed::NeedsSoftwareDecode { .. }));
    }

    /// Blu-ray remux codecs are decoded and declared as PCM (stereo, or 5.1 from six channels up);
    /// without it they stay refused; the TV's own codecs never take the software path.
    #[test]
    fn flac_and_remux_audio_become_pcm_when_the_build_can_decode_them() {
        let caps = Caps::assumed();
        assert_eq!(AudioFeed::decide("flac", 2, &caps, PCM), AudioFeed::SoftPcm { source: "flac".into(), channels: 2, dplii: false });
        assert_eq!(AudioFeed::decide("flac", 2, &caps, SoftwareAudio::NONE), AudioFeed::NeedsSoftwareDecode { codec: "flac".into(), channels: 2 });
        for (codec, ch, want) in [("truehd", 8, 6), ("pcm_bluray", 8, 6), ("pcm_s24le", 6, 6), ("pcm_s16be", 2, 2), ("opus", 2, 2), ("mlp", 6, 6), ("vorbis", 1, 2), ("mp2", 2, 2), ("alac", 4, 2), ("flac", 0, 2)] {
            assert_eq!(AudioFeed::decide(codec, ch, &caps, PCM), AudioFeed::SoftPcm { source: codec.into(), channels: want, dplii: false }, "{codec} {ch}ch");
        }
        assert_eq!(AudioFeed::decide("FLAC", 2, &caps, PCM).payload(), Some(("PCM", 6)), "the Load declares PCM");
        assert_eq!(AudioFeed::decide("flac", 2, &caps, PCM).rank(), 1, "below a native track");
        for (codec, native) in [("aac", "AAC"), ("ac3", "AC3"), ("eac3", "AC3 PLUS")] {
            assert_eq!(AudioFeed::decide(codec, 6, &caps, PCM).payload().map(|p| p.0), Some(native), "{codec} stays native");
        }
        assert_eq!(AudioFeed::decide("dts", 6, &caps, PCM), AudioFeed::TranscodeAc3 { source: "dts".into(), channels: 6 }, "DTS keeps its own path");
        assert!(matches!(AudioFeed::decide("cook", 2, &caps, PCM), AudioFeed::NeedsSoftwareDecode { .. }), "a codec the build has no decoder for");
        assert_eq!(AudioFeed::decide("wmav2", 2, &caps, PCM), AudioFeed::SoftPcm { source: "wmav2".into(), channels: 2, dplii: false });
    }

    /// A Blu-ray remux: TrueHD 7.1 default, then the AC-3 compatibility track. The TV decodes the
    /// AC-3, so it is fed natively; the TrueHD is decoded only when it is all there is.
    #[test]
    fn a_natively_playable_track_beats_a_soft_decoded_one() {
        let remux = probed(Some("hevc"), &[("truehd", 8, true), ("ac3", 6, false), ("pcm_bluray", 8, false)]);
        let pick = |p: &ProbedStreams| derive_declaration(p, None, &TrackPrefs::default(), feed(PCM)).unwrap();
        let (d, t) = pick(&remux);
        assert_eq!((t, d.acodec.as_str()), (Some(1), "ac3"));
        let lossless = probed(Some("hevc"), &[("pcm_bluray", 8, false), ("truehd", 8, true)]);
        assert_eq!(pick(&lossless).1, Some(1), "both soft: the default");
        let (d, t) = derive_declaration(&remux, Some(0), &TrackPrefs::default(), feed(PCM)).unwrap();
        assert_eq!((t, d.acodec.as_str(), d.audio_channels), (Some(0), "truehd", 8), "the viewer may still pick the TrueHD");
    }

    /// **The optical-link fix.** A multichannel track the pipeline has no name for goes as AC-3
    /// 5.1 where the build can (its decoder and the `ac3` encoder) and the table admits AC-3 at
    /// that width; stereo stays PCM; the PCM rung, a missing decoder or encoder, an AC-3 row too
    /// narrow, and a rate AC-3 cannot carry keep it on 6-channel PCM.
    #[test]
    fn multichannel_soft_decoded_audio_goes_as_ac3_unless_pcm_is_asked_for() {
        use crate::prefs::Multichannel;
        let mut caps_with_ac3 = Caps::assumed();
        caps_with_ac3.audio = "aac,ac3,eac3".into();
        let sw = SoftwareAudio { ac3_encoder: true, pcm: true, decoders: u32::MAX, multichannel: Multichannel::Ac3 };
        let ac3 = |c: &str, ch| AudioFeed::TranscodeAc3 { source: c.into(), channels: ch };
        let pcm = |c: &str, ch| AudioFeed::SoftPcm { source: c.into(), channels: ch, dplii: false };
        assert_eq!(AudioFeed::decide("truehd", 6, &caps_with_ac3, sw), ac3("truehd", 6));
        for (codec, ch) in [("truehd", 8), ("mlp", 6), ("flac", 6), ("pcm_bluray", 8), ("pcm_s24le", 6), ("opus", 6), ("vorbis", 6), ("alac", 6), ("wmapro", 6), ("flac", 3)] {
            assert_eq!(AudioFeed::decide(codec, ch, &caps_with_ac3, sw), ac3(codec, ch), "{codec} {ch}ch");
        }
        assert_eq!(AudioFeed::decide("flac", 2, &caps_with_ac3, sw), pcm("flac", 2), "stereo stays PCM");
        assert_eq!(AudioFeed::decide("truehd", 0, &caps_with_ac3, sw), pcm("truehd", 2), "an unknown width is not multichannel");
        assert_eq!(AudioFeed::decide("truehd", 6, &caps_with_ac3, SoftwareAudio { multichannel: Multichannel::Pcm, ..sw }), pcm("truehd", 6), "the PCM rung / an eARC route");
        assert_eq!(AudioFeed::decide("truehd", 6, &caps_with_ac3, SoftwareAudio { ac3_encoder: false, ..sw }), pcm("truehd", 6), "no ac3 encoder: the old PCM path");
        assert_eq!(AudioFeed::decide("truehd", 6, &caps_with_ac3, SoftwareAudio { decoders: 1, ..sw }), pcm("truehd", 6), "no truehd decoder for the AC-3 path");
        assert_eq!(AudioFeed::decide_at("truehd", 6, 96_000, &caps_with_ac3, sw), pcm("truehd", 6), "96 kHz: AC-3 cannot carry it");
        assert_eq!(AudioFeed::decide_at("truehd", 6, 48_000, &caps_with_ac3, sw), ac3("truehd", 6));
        let mut narrow = caps_with_ac3.clone();
        narrow.audio_channels.insert("ac3".into(), 2);
        assert_eq!(AudioFeed::decide("truehd", 6, &narrow, sw), pcm("truehd", 6), "an AC-3 ceiling of 2 does not admit 5.1");
        assert_eq!(AudioFeed::decide("truehd", 6, &caps_with_ac3, SoftwareAudio { pcm: false, ..sw }), ac3("truehd", 6), "the AC-3 path needs no libsnadec");
        assert_eq!(AudioFeed::decide("dts", 8, &caps_with_ac3, sw), ac3("dts", 8), "DTS keeps its path");
        assert_eq!(AudioFeed::decide("eac3", 6, &caps_with_ac3, sw).payload(), Some(("AC3 PLUS", 2)), "native stays native");
        assert_eq!((ac3("truehd", 6).fed_as(), pcm("flac", 2).fed_as(), AudioFeed::DtsCore.fed_as()), ("Dolby Digital", "PCM", ""));
    }

    /// **Stereo (Pro Logic II).** Every track of more than two channels the build can decode — the
    /// television's own codecs too — is folded to matrixed stereo PCM; stereo is untouched, and a
    /// codec the build cannot decode keeps its native path.
    #[test]
    fn the_pro_logic_ii_rung_folds_every_multichannel_track_the_build_decodes() {
        use crate::prefs::Multichannel;
        let mut caps = Caps::assumed();
        caps.audio = "aac,ac3,eac3,dts".into();
        let sw = SoftwareAudio { ac3_encoder: true, pcm: true, decoders: u32::MAX, multichannel: Multichannel::StereoDplii };
        let dplii = |c: &str| AudioFeed::SoftPcm { source: c.into(), channels: 2, dplii: true };
        for codec in ["ac3", "eac3", "dts", "truehd", "flac", "pcm_bluray"] {
            assert_eq!(AudioFeed::decide(codec, 6, &caps, sw), dplii(codec), "{codec} 5.1");
        }
        assert_eq!(AudioFeed::decide("eac3", 8, &caps, sw), dplii("eac3"));
        assert_eq!(AudioFeed::decide("aac", 2, &caps, sw), AudioFeed::Native { payload: "AAC", diag: 3 }, "stereo native is untouched");
        assert_eq!(AudioFeed::decide("flac", 2, &caps, sw), AudioFeed::SoftPcm { source: "flac".into(), channels: 2, dplii: false });
        let no_ac3_decoder = SoftwareAudio { decoders: 1, ..sw };
        assert_eq!(AudioFeed::decide("ac3", 6, &caps, no_ac3_decoder), AudioFeed::Native { payload: "AC3", diag: 1 }, "no ac3 decoder in the build: native");
        assert_eq!(AudioFeed::decide("dts", 6, &caps, no_ac3_decoder), dplii("dts"));
        assert_eq!(AudioFeed::decide("ac3", 6, &caps, SoftwareAudio { pcm: false, ..sw }).payload(), Some(("AC3", 1)), "no libsnadec: native");
        assert_eq!(dplii("ac3").payload(), Some(("PCM", 6)));
        assert_eq!(dplii("ac3").fed_as(), "Pro Logic II");
    }

    /// **Stereo.** Every track of more than two channels the build can decode — the television's
    /// own codecs too — is decoded here and downmixed to plain stereo PCM (no matrix); stereo is
    /// untouched, and a codec the build cannot decode keeps its native path (the bundled build has
    /// no ac3/eac3 decoder, so on the television AC-3 and E-AC-3 stay native under this rung).
    #[test]
    fn the_stereo_rung_downmixes_every_multichannel_track_the_build_decodes() {
        use crate::prefs::Multichannel;
        let mut caps = Caps::assumed();
        caps.audio = "aac,ac3,eac3,dts".into();
        let sw = SoftwareAudio { ac3_encoder: true, pcm: true, decoders: u32::MAX, multichannel: Multichannel::Stereo };
        let stereo = |c: &str| AudioFeed::SoftPcm { source: c.into(), channels: 2, dplii: false };
        for codec in ["ac3", "eac3", "dts", "truehd", "flac", "pcm_bluray"] {
            assert_eq!(AudioFeed::decide(codec, 6, &caps, sw), stereo(codec), "{codec} 5.1");
        }
        assert_eq!(AudioFeed::decide("aac", 2, &caps, sw), AudioFeed::Native { payload: "AAC", diag: 3 }, "stereo native is untouched");
        let bundled = SoftwareAudio { decoders: crate::stream::DECODERS.iter().position(|(c, _)| *c == "ac3").map_or(0, |i| !(1u32 << i) & !(1u32 << (i + 1))), ..sw };
        assert!(bundled.decodes("dts") && !bundled.decodes("ac3") && !bundled.decodes("eac3"));
        assert_eq!(AudioFeed::decide("ac3", 6, &caps, bundled), AudioFeed::Native { payload: "AC3", diag: 1 }, "no ac3 decoder: native");
        assert_eq!(AudioFeed::decide("eac3", 6, &caps, bundled).payload(), Some(("AC3 PLUS", 2)), "no eac3 decoder: native");
        assert_eq!(AudioFeed::decide("truehd", 8, &caps, bundled), stereo("truehd"));
    }
}
