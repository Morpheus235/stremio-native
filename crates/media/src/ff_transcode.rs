//! **Decode -> AC-3 on the television's CPU** — the software audio path behind
//! [`crate::stream::AudioFeed::TranscodeAc3`] and, with the `eac3` encoder ([`Target::Eac3`],
//! 768 kb/s, the same frame and delay), [`crate::stream::AudioFeed::TranscodeEac3`].
//!
//! A DTS track the television's codec table will not take (no DTS row, or more channels than its
//! DTS ceiling), and a multichannel track in a codec the pipeline has no name for (TrueHD/MLP,
//! FLAC, LPCM, Opus, Vorbis, ALAC, WMA Pro), are re-encoded into what the Starfish feed is PROVEN
//! on and what an optical or plain HDMI ARC link carries as 5.1: AC-3 (the television decodes
//! AC3/E-AC3 natively, and AC-3 is fed through `ff::demux`'s audio lane verbatim). The bundled
//! decoder for the stream's codec ([`crate::stream::DECODERS`]) decodes it — for DTS the
//! **core** only (`core_only`, extensions ignored), and DTS and TrueHD/MLP are asked by name for a
//! `downmix=5.1(side)` they fold or extract themselves — the samples are converted to float
//! planar ([`to_planar_f32`]: decoders emit u8/s16/s32/float/double, packed or planar; no
//! libswresample), the planes are kept or reduced to at most 5.1 by channel identity, and the
//! float `ac3` encoder produces 640 kb/s frames of 1536 samples, which the demuxer pushes onto the
//! audio lane in place of the source packets while the Load declares `"AC3"`.
//!
//! ponytail: this trades the lossless multichannel PCM an eARC link could carry for the AC-3 an
//! optical or ARC link can — and on webOS 6 app PCM is mixed to stereo even over eARC (issue #3),
//! so AC-3 is Auto's on every route; `prefs::MultichannelAudio`'s PCM rung forces PCM.
//!
//! # Timestamps
//!
//! The decoder turns one packet into one frame (512 samples for the common 48 kHz DTS core, 40 for
//! a TrueHD access unit); the encoder wants exactly 1536. So samples are queued in a planar FIFO and timed by COUNTING them,
//! anchored at the container pts of the first packet that decodes to samples (a TrueHD stream joined
//! mid-way decodes nothing until its next major sync) ([`SampleClock`]): AC-3 frame `n` is stamped
//! `anchor + (n * 1536 - 256) / rate`, where 256 is the encoder's `initial_padding` (the
//! encoder subtracts it from each packet's pts itself, in its 1/rate time base). Counting rather
//! than restamping each frame from the container keeps the AC-3 pts strictly monotonic through the
//! millisecond rounding Matroska timestamps carry; a packet whose pts disagrees with the count by
//! more than [`REANCHOR_NS`] (a gap in the file, a broken mux) re-anchors the clock and drops what
//! the FIFO held from the old timeline. A seek calls [`ToAc3::reset`].
//!
//! # ABI
//!
//! Every FFmpeg struct field this file touches is asserted in `ci/ffabi-assert.c` on both pointer
//! widths, and the encoder is configured through name- and accessor-based paths wherever FFmpeg has
//! one: a library-allocated `AVCodecParameters` (format, bit rate, sample rate, channel layout)
//! handed to `avcodec_parameters_to_context`, `av_opt_set("time_base")`, the decoder's private
//! `core_only`/`downmix` options by name, and the sample format by name (`av_get_sample_fmt`). After
//! `avcodec_open2` the encoder is read back through `avcodec_parameters_from_context` and compared
//! with what was asked for — the runtime self-check `Venc::open` established — and a mismatch
//! refuses the transcoder instead of feeding garbage. Only AVFrame's audio fields
//! (`OFF_FRAME_NB_SAMPLES`, `_FORMAT`, `_SAMPLE_RATE`, `_EXTENDED_DATA`, `_CH_LAYOUT`) are raw
//! offsets, because FFmpeg has no setter for them.

use super::*;

/// The AC-3 bit rate: the format's maximum, so a 5.1 source loses as little as AC-3 allows.
pub const AC3_BITRATE: i64 = 640_000;
/// The E-AC-3 bit rate, Kodi's webOS choice. FFmpeg's `eac3` encoder is the AC-3 core (same
/// 1536-sample frame, same 256-sample delay, 5.1 at most), so everything below serves both.
pub const EAC3_BITRATE: i64 = 768_000;

/// What the transcoder encodes to: AC-3 (`"AC3"`, what optical and plain ARC carry), or E-AC-3
/// (`"AC3 PLUS"`, for HDMI ARC / eARC; Settings → Multichannel audio → Dolby Digital Plus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Ac3,
    Eac3,
}

impl Target {
    fn encoder(self) -> &'static std::ffi::CStr {
        match self { Target::Ac3 => c"ac3", Target::Eac3 => c"eac3" }
    }
    fn codec_id(self) -> c_int {
        match self { Target::Ac3 => AV_CODEC_ID_AC3, Target::Eac3 => AV_CODEC_ID_EAC3 }
    }
    pub fn bitrate(self) -> i64 {
        match self { Target::Ac3 => AC3_BITRATE, Target::Eac3 => EAC3_BITRATE }
    }
    pub fn name(self) -> &'static str {
        match self { Target::Ac3 => "AC-3", Target::Eac3 => "E-AC-3" }
    }
}
/// Samples per AC-3 frame (six 256-sample blocks), what the encoder requires of every input frame.
pub const AC3_FRAME_SAMPLES: usize = 1536;
/// The sample rates AC-3 can carry. The decoder's output rate is checked against them before the
/// encoder is opened (and the feed decision keeps a track probed at another rate on PCM).
pub const AC3_RATES: [i32; 3] = [48_000, 44_100, 32_000];
/// A packet whose container pts is further than this from the sample count's prediction starts a
/// new timeline (see the module doc). Matroska rounds to the millisecond and MPEG-TS jitters by a
/// few: 100 ms is well clear of both and well under any real gap.
pub const REANCHOR_NS: i64 = 100_000_000;
/// Consecutive undecodable packets tolerated before the transcoder gives up: a damaged frame or
/// two is skipped (silence), a stream of them is not audio this decoder can play.
const MAX_BAD_PACKETS: u32 = 32;

// AVChannel ids (libavutil/channel_layout.h), asserted in ci/ffabi-assert.c.
pub const CH_FL: i32 = 0;
pub const CH_FR: i32 = 1;
pub const CH_FC: i32 = 2;
pub const CH_LFE: i32 = 3;
pub const CH_BL: i32 = 4;
pub const CH_BR: i32 = 5;
pub const CH_SL: i32 = 9;
pub const CH_SR: i32 = 10;
const CH_BC: i32 = 8;
const AV_CHANNEL_ORDER_NATIVE: c_int = 1;

const fn bit(ch: i32) -> u64 {
    1u64 << ch
}

/// The channel masks the `ac3` encoder accepts (`ff_ac3_ch_layouts`, libavcodec/ac3enc.c, FFmpeg
/// 9.0): mono, stereo, 2.1, 3.0, 2/2 side, quad, 4.0, 5.0 side and back, the five LFE variants of
/// mono/stereo/2.1/3.0/4.0, and 5.1 side and back. Note that 2/2 + LFE is NOT among them.
pub const AC3_MASKS: [u64; 16] = {
    let (fl, fr, fc, lfe, bl, br, sl, sr, bc) =
        (bit(CH_FL), bit(CH_FR), bit(CH_FC), bit(CH_LFE), bit(CH_BL), bit(CH_BR), bit(CH_SL), bit(CH_SR), bit(CH_BC));
    let stereo = fl | fr;
    [
        fc,
        stereo,
        stereo | bc,
        stereo | fc,
        stereo | sl | sr,
        stereo | bl | br,
        stereo | fc | bc,
        stereo | fc | sl | sr,
        stereo | fc | bl | br,
        fc | lfe,
        stereo | lfe,
        stereo | bc | lfe,
        stereo | fc | lfe,
        stereo | fc | bc | lfe,
        stereo | fc | sl | sr | lfe,
        stereo | fc | bl | br | lfe,
    ]
};

/// PURE: **which output channel takes which decoded plane.** `mask` is the AC-3 layout (native
/// order, so output plane `i` is the `i`-th set bit), and `map[i]` the input plane feeding it, or
/// `None` for silence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelPlan {
    pub mask: u64,
    pub map: Vec<Option<usize>>,
}

impl ChannelPlan {
    pub fn channels(&self) -> usize {
        self.map.len()
    }

    /// The plan for a decoded frame whose planes carry the channel ids `input` (`-1` for one the
    /// layout cannot name). A layout AC-3 carries as it is passes through unchanged (reordered to
    /// native order if it came in another). Anything else is REDUCED to the closest AC-3 layout by
    /// identity: the fronts, the centre, LFE, and side surrounds (back ones standing in when there
    /// are no sides); extra channels (a 6.1 back centre, 7.1's second surround pair) are dropped,
    /// and a missing centre beside surrounds and LFE is silence, since 2/2 + LFE is no AC-3 layout.
    /// A layout with no ids at all is taken positionally: one plane is mono, otherwise the first
    /// two are stereo and six or more are read as FFmpeg's 5.1 order.
    pub fn for_input(input: &[i32]) -> ChannelPlan {
        let known = !input.is_empty() && input.iter().all(|&c| (0..64).contains(&c));
        let mask = input.iter().fold(0u64, |m, &c| if (0..64).contains(&c) { m | bit(c) } else { m });
        let distinct = known && mask.count_ones() as usize == input.len();
        let idx = |ch: i32| input.iter().position(|&c| c == ch);
        if distinct && AC3_MASKS.contains(&mask) {
            return Self::native(mask, |ch| idx(ch));
        }
        if !known {
            return match input.len() {
                0 => ChannelPlan { mask: bit(CH_FC), map: vec![None] },
                1 => ChannelPlan { mask: bit(CH_FC), map: vec![Some(0)] },
                n if n >= 6 => Self::native(AC3_MASKS[14], |ch| match ch {
                    CH_FL => Some(0),
                    CH_FR => Some(1),
                    CH_FC => Some(2),
                    CH_LFE => Some(3),
                    CH_SL => Some(4),
                    _ => Some(5),
                }),
                _ => ChannelPlan { mask: bit(CH_FL) | bit(CH_FR), map: vec![Some(0), Some(1)] },
            };
        }
        let (fl, fr, fc, lfe) = (idx(CH_FL), idx(CH_FR), idx(CH_FC), idx(CH_LFE));
        let (sl, sr) = (idx(CH_SL).or(idx(CH_BL)), idx(CH_SR).or(idx(CH_BR)));
        if fl.is_none() || fr.is_none() {
            // No front pair (a centre-only layout plus oddments): mono from the centre, else the
            // first plane.
            return ChannelPlan { mask: bit(CH_FC), map: vec![fc.or(Some(0))] };
        }
        let mut want = bit(CH_FL) | bit(CH_FR);
        if sl.is_some() && sr.is_some() {
            want |= bit(CH_FC) | bit(CH_SL) | bit(CH_SR);
        } else if fc.is_some() {
            want |= bit(CH_FC);
        }
        if lfe.is_some() {
            want |= bit(CH_LFE);
        }
        debug_assert!(AC3_MASKS.contains(&want));
        Self::native(want, |ch| match ch {
            CH_FL => fl,
            CH_FR => fr,
            CH_FC => fc,
            CH_LFE => lfe,
            CH_SL => sl,
            CH_SR => sr,
            other => idx(other),
        })
    }

    fn native(mask: u64, source: impl Fn(i32) -> Option<usize>) -> ChannelPlan {
        let map = (0..64).filter(|&c| mask & bit(c) != 0).map(source).collect();
        ChannelPlan { mask, map }
    }
}

/// The sample type of a decoded frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    U8,
    S16,
    S32,
    F32,
    F64,
}

impl SampleKind {
    pub const fn bytes(self) -> usize {
        match self {
            SampleKind::U8 => 1,
            SampleKind::S16 => 2,
            SampleKind::S32 | SampleKind::F32 => 4,
            SampleKind::F64 => 8,
        }
    }

    fn read(self, b: &[u8]) -> f32 {
        match self {
            SampleKind::U8 => (f32::from(b[0]) - 128.0) / 128.0,
            SampleKind::S16 => f32::from(i16::from_ne_bytes([b[0], b[1]])) / 32_768.0,
            SampleKind::S32 => i32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            SampleKind::F32 => f32::from_ne_bytes([b[0], b[1], b[2], b[3]]),
            SampleKind::F64 => f64::from_ne_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
        }
    }
}

/// The decoders' sample formats by FFmpeg's names (`av_get_sample_fmt` resolves each to the
/// library's number at open): the type, and whether each channel has its own plane.
pub const SAMPLE_FORMATS: [(&std::ffi::CStr, SampleKind, bool); 10] = [
    (c"u8", SampleKind::U8, false),
    (c"s16", SampleKind::S16, false),
    (c"s32", SampleKind::S32, false),
    (c"flt", SampleKind::F32, false),
    (c"dbl", SampleKind::F64, false),
    (c"u8p", SampleKind::U8, true),
    (c"s16p", SampleKind::S16, true),
    (c"s32p", SampleKind::S32, true),
    (c"fltp", SampleKind::F32, true),
    (c"dblp", SampleKind::F64, true),
];

/// PURE: `n` samples of `channels` channels as float planes. `planes` holds one byte plane per
/// channel when `planar`, else one interleaved plane; a short plane reads as silence past its end.
pub fn to_planar_f32(kind: SampleKind, planar: bool, channels: usize, n: usize, planes: &[&[u8]]) -> Vec<Vec<f32>> {
    let w = kind.bytes();
    (0..channels)
        .map(|c| {
            (0..n)
                .map(|i| {
                    let (plane, at) = if planar { (planes.get(c), i * w) } else { (planes.first(), (i * channels + c) * w) };
                    plane.and_then(|p| p.get(at..at + w)).map_or(0.0, |b| kind.read(b))
                })
                .collect()
        })
        .collect()
}

/// PURE: the planar sample queue between the decoder's frames and the encoder's 1536-sample ones.
#[derive(Debug, Default)]
pub struct PlanarFifo {
    planes: Vec<Vec<f32>>,
}

impl PlanarFifo {
    pub fn new(channels: usize) -> PlanarFifo {
        PlanarFifo { planes: vec![Vec::new(); channels] }
    }

    pub fn len(&self) -> usize {
        self.planes.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.planes.iter_mut().for_each(Vec::clear);
    }

    /// Append `n` samples to every output plane: from `input[map[i]]`, or silence.
    pub fn push(&mut self, plan: &ChannelPlan, input: &[&[f32]], n: usize) {
        let end = self.len() + n;
        for (plane, src) in self.planes.iter_mut().zip(&plan.map) {
            if let Some(samples) = src.and_then(|s| input.get(s)) {
                plane.extend_from_slice(&samples[..n.min(samples.len())]);
            }
            // Silence for an unmapped channel (or a short input plane), so every plane always
            // holds the same count.
            plane.resize(end, 0.0);
        }
    }

    /// Pad every plane with silence up to `n` samples.
    pub fn pad_to(&mut self, n: usize) {
        if self.len() < n {
            self.planes.iter_mut().for_each(|p| p.resize(n, 0.0));
        }
    }

    /// Remove the first `n` samples of every plane, handing each plane's slice to `out`.
    pub fn pop(&mut self, n: usize, mut out: impl FnMut(usize, &[f32])) {
        for (i, plane) in self.planes.iter_mut().enumerate() {
            out(i, &plane[..n]);
            plane.drain(..n);
        }
    }
}

/// PURE: the sample-counting clock of the module doc. Sample indices are absolute (they never
/// restart), so encoder pts stay monotonic across a re-anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleClock {
    rate: i64,
    base_ns: i64,
    base_sample: i64,
    anchored: bool,
}

impl SampleClock {
    pub fn new(rate: i32) -> SampleClock {
        SampleClock { rate: i64::from(rate.max(1)), base_ns: 0, base_sample: 0, anchored: false }
    }

    /// Content time of absolute sample `s`.
    pub fn ns_of(&self, s: i64) -> i64 {
        let d = i128::from(s - self.base_sample) * 1_000_000_000 / i128::from(self.rate);
        self.base_ns.saturating_add(d.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }

    /// A packet stamped `pts_ns` is about to add samples starting at absolute sample `end`. Returns
    /// `true` when it starts a new timeline (the first packet, or one past [`REANCHOR_NS`]).
    pub fn on_packet(&mut self, pts_ns: i64, end: i64) -> bool {
        if self.anchored && (pts_ns - self.ns_of(end)).abs() <= REANCHOR_NS {
            return false;
        }
        let first = !self.anchored;
        self.base_ns = pts_ns;
        self.base_sample = end;
        self.anchored = true;
        !first
    }

    /// Forget the anchor (a seek): the next packet anchors afresh.
    pub fn reset(&mut self) {
        self.anchored = false;
    }

    /// A new sample rate from the decoder: keep the time of `end` and count at the new rate.
    pub fn set_rate(&mut self, rate: i32, end: i64) {
        let at = self.ns_of(end);
        self.rate = i64::from(rate.max(1));
        self.base_ns = at;
        self.base_sample = end;
    }
}

/// One AC-3 access unit for the audio lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ac3Frame {
    pub data: Vec<u8>,
    pub pts_ns: i64,
}

/// Why the transcoder cannot go on. Every one ends the audio feed with an explicit failure
/// (`ff::demux` raises `demux_failed`), never a silent lane the clock waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscodeError {
    /// The bundled libavcodec has no decoder for the source or no `ac3` encoder, or FFmpeg is not
    /// bound.
    Unavailable,
    /// An allocation or open step failed (`what`, FFmpeg's return code).
    Open(&'static str, c_int),
    /// The encoder read back differently from how it was configured — the ABI self-check.
    Abi(String),
    /// The source decodes at a rate AC-3 cannot carry.
    SampleRate(i32),
    /// The decoder produced a sample format [`SAMPLE_FORMATS`] does not name.
    SampleFormat(c_int),
    /// Too many consecutive packets failed to decode.
    Decode(c_int),
    /// The encoder refused a frame or a packet.
    Encode(c_int),
}

static AVAILABLE: AtomicBool = AtomicBool::new(false);
static EAC3_AVAILABLE: AtomicBool = AtomicBool::new(false);
static DECODERS: AtomicU32 = AtomicU32::new(0);

/// Does this build carry the `ac3` encoder? Latched by `ff::boot` from the libraries actually
/// loaded, so an FFmpeg built without it keeps the PCM path or the explicit `AudioUnsupported`
/// refusal. `false` before boot.
pub fn available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}

/// The bundled libavcodec also carries the `eac3` encoder (the E-AC-3 target). `false` before boot.
pub fn eac3_available() -> bool {
    EAC3_AVAILABLE.load(Ordering::Acquire)
}

/// Which of [`crate::stream::DECODERS`] the loaded libavcodec carries, one bit each (0 before
/// boot).
pub fn decoders() -> u32 {
    DECODERS.load(Ordering::Acquire)
}

/// Look the encoder and the decoders up in the bound libavcodec. Only after the ABI gate opened.
/// `true` when the AC-3 path is there for at least one source.
pub(super) fn detect() -> bool {
    let abi = ABI_OK.load(Ordering::Acquire);
    let enc = abi && unsafe { !avcodec_find_encoder_by_name(c"ac3".as_ptr()).is_null() };
    let mut mask = 0u32;
    let mut names = Vec::new();
    for (i, (codec, dec)) in crate::stream::DECODERS.iter().enumerate() {
        let name = CString::new(*dec).expect("no NUL");
        if abi && unsafe { !avcodec_find_decoder_by_name(name.as_ptr()).is_null() } {
            mask |= 1 << i;
            names.push(*codec);
        }
    }
    let eac3 = enc && unsafe { !avcodec_find_encoder_by_name(c"eac3".as_ptr()).is_null() };
    AVAILABLE.store(enc, Ordering::Release);
    EAC3_AVAILABLE.store(eac3, Ordering::Release);
    DECODERS.store(mask, Ordering::Release);
    let ok = enc && mask != 0;
    crate::player::log(&format!(
        "ff: decode -> AC-3 software path {}",
        if ok { format!("available (ac3{} encoder; from {})", if eac3 { " + eac3" } else { "" }, names.join(" ")) } else { "UNAVAILABLE — DTS past the TV's ceiling is refused, other multichannel sources go as PCM".into() }
    ));
    ok
}

#[inline]
unsafe fn frame_i32(f: *const c_void, off: usize) -> i32 {
    *((f as *const u8).add(off) as *const i32)
}

#[inline]
unsafe fn frame_planes(f: *const c_void) -> *const *mut u8 {
    *((f as *const u8).add(OFF_FRAME_EXTENDED_DATA) as *const *const *mut u8)
}

#[inline]
unsafe fn frame_ch_layout(f: *mut c_void) -> *mut AVChannelLayout {
    (f as *mut u8).add(OFF_FRAME_CH_LAYOUT) as *mut AVChannelLayout
}

/// The live encoder half, opened at the first decoded frame (it needs the source's rate and layout).
struct Encoder {
    ctx: *mut AVCodecContext,
    frame: *mut c_void,
    pkt: *mut AVPacket,
    plan: ChannelPlan,
    rate: i32,
    /// The decoded layout the plan was made for, as (nb_channels, ids): a change re-plans.
    input: Vec<i32>,
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            if !self.pkt.is_null() {
                av_packet_free(&mut self.pkt);
            }
            if !self.frame.is_null() {
                av_frame_free(&mut self.frame);
            }
            if !self.ctx.is_null() {
                avcodec_free_context(&mut self.ctx);
            }
        }
    }
}

/// The transcoder (see the module doc). Owned and driven by the demux thread alone.
pub struct ToAc3 {
    dec: *mut AVCodecContext,
    dec_frame: *mut c_void,
    pkt_in: *mut AVPacket,
    enc: Option<Encoder>,
    fifo: PlanarFifo,
    clock: SampleClock,
    /// Absolute index of the first sample in `fifo`.
    head_sample: i64,
    /// The pts of the packet being decoded, until its first sample anchors (or checks) the clock.
    pending_pts: Option<i64>,
    fltp: c_int,
    /// [`SAMPLE_FORMATS`] as this library numbers them.
    formats: [c_int; SAMPLE_FORMATS.len()],
    /// The source codec's name, for the log.
    source: &'static str,
    /// AC-3 or E-AC-3.
    target: Target,
    bad_run: u32,
    bad_total: u64,
    frames_out: u64,
}

// The FFmpeg objects are touched by the one thread that owns the value.
unsafe impl Send for ToAc3 {}

impl Drop for ToAc3 {
    fn drop(&mut self) {
        self.enc = None;
        unsafe {
            if !self.pkt_in.is_null() {
                av_packet_free(&mut self.pkt_in);
            }
            if !self.dec_frame.is_null() {
                av_frame_free(&mut self.dec_frame);
            }
            if !self.dec.is_null() {
                avcodec_free_context(&mut self.dec);
            }
        }
    }
}

impl ToAc3 {
    /// Open the decoder for `source` (FFmpeg's codec name, one of [`crate::stream::DECODERS`]),
    /// from the demuxed stream's parameters when there is a stream (`codecpar`), else bare (raw
    /// DTS frames, as the host tests feed), encoding to `target`. The encoder opens at the first
    /// decoded frame.
    ///
    /// # Safety
    /// `codecpar` is null or a live `AVCodecParameters` of a `source` stream.
    pub unsafe fn open(codecpar: *const AVCodecParameters, source: &str, target: Target) -> Result<ToAc3, TranscodeError> {
        if !available() || (target == Target::Eac3 && !eac3_available()) {
            return Err(TranscodeError::Unavailable);
        }
        let Some(&(source, dec_name)) = crate::stream::DECODERS.iter().find(|(c, _)| c.eq_ignore_ascii_case(source)) else {
            return Err(TranscodeError::Unavailable);
        };
        let fltp = av_get_sample_fmt(c"fltp".as_ptr());
        if fltp < 0 {
            return Err(TranscodeError::Open("av_get_sample_fmt(fltp)", fltp));
        }
        let formats = SAMPLE_FORMATS.map(|(name, _, _)| av_get_sample_fmt(name.as_ptr()));
        let dec_c = CString::new(dec_name).expect("no NUL");
        let codec = avcodec_find_decoder_by_name(dec_c.as_ptr());
        if codec.is_null() {
            return Err(TranscodeError::Unavailable);
        }
        let t = ToAc3 {
            dec: avcodec_alloc_context3(codec),
            dec_frame: av_frame_alloc(),
            pkt_in: av_packet_alloc(),
            enc: None,
            fifo: PlanarFifo::default(),
            clock: SampleClock::new(48_000),
            head_sample: 0,
            pending_pts: None,
            fltp,
            formats,
            source,
            target,
            bad_run: 0,
            bad_total: 0,
            frames_out: 0,
        };
        if t.dec.is_null() || t.dec_frame.is_null() || t.pkt_in.is_null() {
            return Err(TranscodeError::Open("alloc", -12));
        }
        if !codecpar.is_null() {
            let r = avcodec_parameters_to_context(t.dec, codecpar);
            if r < 0 {
                return Err(TranscodeError::Open("avcodec_parameters_to_context", r));
            }
        }
        // The decoder's PRIVATE options, by name. DTS: decode the core only (DTS-HD's extension
        // substream is ignored — the result is the same lossy core the TV's own DTS path plays).
        // DTS and TrueHD/MLP: ask for 5.1 — dca folds a 6.1/7.1 core down with the stream's own
        // coefficients where it carries them, mlpdec decodes the 5.1 presentation substream
        // instead of the full 7.1. `ChannelPlan` still reduces whatever comes out. NB "5.1(side)",
        // not "5.1": FFmpeg names 5.1(back) "5.1", and both decoders only accept the side layout
        // (a mismatch is just a logged "Invalid downmix layout" — the request is silently dropped).
        if source == "dts" {
            let r = av_opt_set_int(t.dec as *mut c_void, c"core_only".as_ptr(), 1, AV_OPT_SEARCH_CHILDREN);
            if r < 0 {
                return Err(TranscodeError::Open("dca core_only", r));
            }
        }
        if matches!(source, "dts" | "truehd" | "mlp") {
            let r = av_opt_set(t.dec as *mut c_void, c"downmix".as_ptr(), c"5.1(side)".as_ptr(), AV_OPT_SEARCH_CHILDREN);
            if r < 0 {
                return Err(TranscodeError::Open("downmix", r));
            }
        }
        let r = avcodec_open2(t.dec, codec, std::ptr::null_mut());
        if r < 0 {
            return Err(TranscodeError::Open("avcodec_open2(decoder)", r));
        }
        crate::player::log(&format!("ff: {source} -> AC-3 transcoder open ({dec_name} decoder, at most 5.1, ac3 640 kb/s)"));
        Ok(t)
    }

    /// The encoder's channel count once it is open (the first decoded frame), else 0.
    pub fn channels(&self) -> usize {
        self.enc.as_ref().map_or(0, |e| e.plan.channels())
    }

    /// The encoder's sample rate once it is open, else 0.
    pub fn sample_rate(&self) -> i32 {
        self.enc.as_ref().map_or(0, |e| e.rate)
    }

    /// Feed one demuxed packet stamped `pts_ns`; returns the AC-3 frames that completed.
    pub fn push(&mut self, packet: &[u8], pts_ns: i64) -> Result<Vec<Ac3Frame>, TranscodeError> {
        let mut out = Vec::new();
        if packet.is_empty() {
            return Ok(out);
        }
        // The clock is checked against this pts at the packet's first decoded sample: a packet
        // that decodes to nothing (TrueHD before its first major sync) must not anchor it.
        self.pending_pts = Some(pts_ns);
        unsafe {
            let r = av_new_packet(self.pkt_in, packet.len() as c_int);
            if r < 0 {
                return Err(TranscodeError::Open("av_new_packet", r));
            }
            std::ptr::copy_nonoverlapping(packet.as_ptr(), (*self.pkt_in).data, packet.len());
            (*self.pkt_in).pts = AV_NOPTS_VALUE;
            (*self.pkt_in).dts = AV_NOPTS_VALUE;
            let r = avcodec_send_packet(self.dec, self.pkt_in);
            av_packet_unref(self.pkt_in);
            if r < 0 && r != AVERROR_EAGAIN {
                self.bad_packet(r)?;
                return Ok(out);
            }
            loop {
                let r = avcodec_receive_frame(self.dec, self.dec_frame);
                if r == AVERROR_EAGAIN || r == AVERROR_EOF {
                    break;
                }
                if r < 0 {
                    self.bad_packet(r)?;
                    break;
                }
                let took = self.take_frame(&mut out);
                av_frame_unref(self.dec_frame);
                took?;
                self.bad_run = 0;
            }
        }
        Ok(out)
    }

    fn bad_packet(&mut self, r: c_int) -> Result<(), TranscodeError> {
        self.bad_run += 1;
        self.bad_total += 1;
        if self.bad_total <= 3 {
            crate::player::log(&format!("ff: transcode: {} packet did not decode (r={r}); skipped", self.source));
        }
        if self.bad_run >= MAX_BAD_PACKETS {
            return Err(TranscodeError::Decode(r));
        }
        Ok(())
    }

    /// Move the decoded frame into the FIFO (opening or re-planning the encoder as needed) and
    /// encode every full AC-3 frame now queued.
    unsafe fn take_frame(&mut self, out: &mut Vec<Ac3Frame>) -> Result<(), TranscodeError> {
        let f = self.dec_frame;
        let n = frame_i32(f, OFF_FRAME_NB_SAMPLES).max(0) as usize;
        let format = frame_i32(f, OFF_FRAME_FORMAT);
        let rate = frame_i32(f, OFF_FRAME_SAMPLE_RATE);
        let Some(fi) = self.formats.iter().position(|&x| x >= 0 && x == format) else {
            return Err(TranscodeError::SampleFormat(format));
        };
        let (_, kind, planar) = SAMPLE_FORMATS[fi];
        if !AC3_RATES.contains(&rate) {
            return Err(TranscodeError::SampleRate(rate));
        }
        if let Some(pts_ns) = self.pending_pts.take() {
            let end = self.head_sample + self.fifo.len() as i64;
            if self.clock.on_packet(pts_ns, end) {
                crate::player::log(&format!(
                    "ff: transcode: {} timeline moved to {}ms; {} queued samples dropped",
                    self.source,
                    pts_ns / 1_000_000,
                    self.fifo.len()
                ));
                self.head_sample = end;
                self.fifo.clear();
            }
        }
        let layout = frame_ch_layout(f);
        let nb = (*layout).nb_channels.max(0) as usize;
        let ids: Vec<i32> = (0..nb).map(|i| av_channel_layout_channel_from_index(layout, i as c_uint)).collect();
        let replan = self.enc.as_ref().is_none_or(|e| e.input != ids || e.rate != rate);
        if replan {
            // A new layout or rate mid-stream: finish what the old encoder holds (padded), then
            // start a new one. On the first frame this is just the open.
            if self.enc.is_some() {
                crate::player::log(&format!("ff: transcode: {} layout/rate changed; reopening the AC-3 encoder", self.source));
                self.flush_into(out)?;
            }
            let end = self.head_sample + self.fifo.len() as i64;
            self.clock.set_rate(rate, end);
            let plan = ChannelPlan::for_input(&ids);
            let enc = self.open_encoder(plan, rate, ids)?;
            self.fifo = PlanarFifo::new(enc.plan.channels());
            self.enc = Some(enc);
        }
        let planes = frame_planes(f);
        let w = kind.bytes();
        let (count, len) = if planar { (nb, n * w) } else { (1, n * nb * w) };
        let bytes: Vec<&[u8]> = (0..count)
            .map(|i| {
                let p = *planes.add(i) as *const u8;
                if p.is_null() || len == 0 { &[][..] } else { std::slice::from_raw_parts(p, len) }
            })
            .collect();
        let samples = to_planar_f32(kind, planar, nb, n, &bytes);
        let input: Vec<&[f32]> = samples.iter().map(Vec::as_slice).collect();
        let plan = &self.enc.as_ref().expect("opened above").plan;
        self.fifo.push(plan, &input, n);
        while self.fifo.len() >= AC3_FRAME_SAMPLES {
            self.encode_head(out)?;
        }
        Ok(())
    }

    unsafe fn open_encoder(&self, plan: ChannelPlan, rate: i32, input: Vec<i32>) -> Result<Encoder, TranscodeError> {
        let codec = avcodec_find_encoder_by_name(self.target.encoder().as_ptr());
        if codec.is_null() {
            return Err(TranscodeError::Unavailable);
        }
        let bitrate = self.target.bitrate();
        let e = Encoder {
            ctx: avcodec_alloc_context3(codec),
            frame: av_frame_alloc(),
            pkt: av_packet_alloc(),
            plan,
            rate,
            input,
        };
        if e.ctx.is_null() || e.frame.is_null() || e.pkt.is_null() {
            return Err(TranscodeError::Open("alloc ac3", -12));
        }
        let layout = AVChannelLayout {
            order: AV_CHANNEL_ORDER_NATIVE,
            nb_channels: e.plan.channels() as c_int,
            u_mask: e.plan.mask,
            opaque: std::ptr::null_mut(),
        };
        // Configure through a LIBRARY-ALLOCATED AVCodecParameters (its true size is the library's
        // business; our model is a prefix) and the accessor that copies it into the context.
        let mut par = avcodec_parameters_alloc();
        if par.is_null() {
            return Err(TranscodeError::Open("avcodec_parameters_alloc", -12));
        }
        (*par).codec_type = AVMEDIA_TYPE_AUDIO;
        (*par).codec_id = self.target.codec_id();
        (*par).format = self.fltp;
        (*par).bit_rate = bitrate;
        (*par).sample_rate = rate;
        let mut r = av_channel_layout_copy(&mut (*par).ch_layout, &layout);
        if r >= 0 {
            r = avcodec_parameters_to_context(e.ctx, par);
        }
        avcodec_parameters_free(&mut par);
        if r < 0 {
            return Err(TranscodeError::Open("avcodec_parameters_to_context(ac3)", r));
        }
        let tb = CString::new(format!("1/{rate}")).expect("no NUL");
        let r = av_opt_set(e.ctx as *mut c_void, c"time_base".as_ptr(), tb.as_ptr(), 0);
        if r < 0 {
            return Err(TranscodeError::Open("ac3 time_base", r));
        }
        let r = avcodec_open2(e.ctx, codec, std::ptr::null_mut());
        if r < 0 {
            return Err(TranscodeError::Open("avcodec_open2(ac3)", r));
        }
        // The runtime ABI self-check: read the open encoder back through the accessor and compare
        // with what was asked. A mismatch means a field this file wrote landed somewhere else.
        let mut back = avcodec_parameters_alloc();
        if back.is_null() {
            return Err(TranscodeError::Open("avcodec_parameters_alloc", -12));
        }
        let r = avcodec_parameters_from_context(back, e.ctx);
        let got = (
            (*back).format,
            (*back).sample_rate,
            (*back).ch_layout.nb_channels,
            (*back).ch_layout.u_mask,
            (*back).bit_rate,
            (*back).frame_size,
            (*back).initial_padding,
        );
        avcodec_parameters_free(&mut back);
        let want = (self.fltp, rate, e.plan.channels() as c_int, e.plan.mask, bitrate, AC3_FRAME_SAMPLES as c_int);
        if r < 0 || (got.0, got.1, got.2, got.3, got.4, got.5) != want {
            return Err(TranscodeError::Abi(format!(
                "{} encoder reads back (fmt {}, {} Hz, {} ch, mask {:#x}, {} b/s, frame {}) for {:?}",
                self.target.name(), got.0, got.1, got.2, got.3, got.4, got.5, want
            )));
        }
        // The input frame: its audio fields have no setter, so these are the asserted offsets.
        poke_i32(e.frame, OFF_FRAME_NB_SAMPLES, AC3_FRAME_SAMPLES as i32);
        poke_i32(e.frame, OFF_FRAME_FORMAT, self.fltp);
        poke_i32(e.frame, OFF_FRAME_SAMPLE_RATE, rate);
        let r = av_channel_layout_copy(frame_ch_layout(e.frame), &layout);
        if r < 0 {
            return Err(TranscodeError::Open("frame ch_layout", r));
        }
        let r = av_frame_get_buffer(e.frame, 0);
        if r < 0 {
            return Err(TranscodeError::Open("av_frame_get_buffer", r));
        }
        let mut desc = [0 as c_char; 64];
        av_channel_layout_describe(&layout, desc.as_mut_ptr(), desc.len());
        crate::player::log(&format!(
            "ff: transcode: {} encoder up: {} Hz, {} ({} ch, from {} decoded), {} kb/s, padding {}",
            self.target.name(),
            rate,
            std::ffi::CStr::from_ptr(desc.as_ptr()).to_string_lossy(),
            e.plan.channels(),
            e.input.len(),
            bitrate / 1000,
            got.6
        ));
        Ok(e)
    }

    /// Encode the FIFO's first 1536 samples (the FIFO must hold them).
    unsafe fn encode_head(&mut self, out: &mut Vec<Ac3Frame>) -> Result<(), TranscodeError> {
        let enc = self.enc.as_mut().expect("encoding needs the encoder");
        let r = av_frame_make_writable(enc.frame);
        if r < 0 {
            return Err(TranscodeError::Encode(r));
        }
        let planes = frame_planes(enc.frame);
        self.fifo.pop(AC3_FRAME_SAMPLES, |i, samples| {
            let dst = *planes.add(i) as *mut f32;
            std::ptr::copy_nonoverlapping(samples.as_ptr(), dst, samples.len());
        });
        poke_i64(enc.frame, OFF_FRAME_PTS, self.head_sample);
        self.head_sample += AC3_FRAME_SAMPLES as i64;
        let r = avcodec_send_frame(enc.ctx, enc.frame);
        if r < 0 {
            return Err(TranscodeError::Encode(r));
        }
        self.drain(out)
    }

    unsafe fn drain(&mut self, out: &mut Vec<Ac3Frame>) -> Result<(), TranscodeError> {
        let Some(enc) = self.enc.as_mut() else { return Ok(()) };
        loop {
            let r = avcodec_receive_packet(enc.ctx, enc.pkt);
            if r == AVERROR_EAGAIN || r == AVERROR_EOF {
                return Ok(());
            }
            if r < 0 {
                return Err(TranscodeError::Encode(r));
            }
            let p = enc.pkt;
            let data = std::slice::from_raw_parts((*p).data, (*p).size.max(0) as usize).to_vec();
            // The packet's pts is in the encoder's 1/rate time base, already less the encoder
            // delay: an absolute sample index on this clock.
            let pts_ns = if (*p).pts == AV_NOPTS_VALUE { self.clock.ns_of(self.head_sample) } else { self.clock.ns_of((*p).pts) };
            av_packet_unref(p);
            self.frames_out += 1;
            out.push(Ac3Frame { data, pts_ns });
        }
    }

    /// Encode what the FIFO holds (padded to a full frame with silence) and drain the encoder.
    unsafe fn flush_into(&mut self, out: &mut Vec<Ac3Frame>) -> Result<(), TranscodeError> {
        if self.enc.is_none() {
            return Ok(());
        }
        if !self.fifo.is_empty() {
            self.fifo.pad_to(AC3_FRAME_SAMPLES);
            self.encode_head(out)?;
        }
        let enc = self.enc.as_mut().expect("checked");
        let r = avcodec_send_frame(enc.ctx, std::ptr::null());
        if r < 0 && r != AVERROR_EOF {
            return Err(TranscodeError::Encode(r));
        }
        self.drain(out)?;
        self.enc = None;
        Ok(())
    }

    /// The end of the stream: the last partial frame, padded, and whatever the encoder still holds.
    pub fn finish(&mut self) -> Result<Vec<Ac3Frame>, TranscodeError> {
        let mut out = Vec::new();
        unsafe { self.flush_into(&mut out)? };
        crate::player::log(&format!(
            "ff: transcode: finished — {} {} frames, {} {} packets skipped",
            self.frames_out, self.target.name(), self.bad_total, self.source
        ));
        Ok(out)
    }

    /// A seek: drop queued samples and the encoder (it reopens at the next frame with fresh
    /// padding), flush the decoder, and anchor the clock at the next packet.
    pub fn reset(&mut self) {
        unsafe { avcodec_flush_buffers(self.dec) };
        self.enc = None;
        self.fifo.clear();
        self.clock.reset();
        self.pending_pts = None;
        self.bad_run = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(input: &[i32]) -> ChannelPlan {
        ChannelPlan::for_input(input)
    }

    /// A layout AC-3 carries passes through in native order; 5.1 is the common case.
    #[test]
    fn a_layout_ac3_carries_passes_through_in_native_order() {
        let p = plan(&[CH_FL, CH_FR, CH_FC, CH_LFE, CH_SL, CH_SR]);
        assert_eq!(p.mask, AC3_MASKS[14]);
        assert_eq!(p.map, (0..6).map(Some).collect::<Vec<_>>());
        let back = plan(&[CH_FL, CH_FR, CH_FC, CH_LFE, CH_BL, CH_BR]);
        assert_eq!((back.mask, back.channels()), (AC3_MASKS[15], 6), "5.1(back) is an AC-3 layout too");
        let coded = plan(&[CH_FC, CH_FL, CH_FR, CH_SL, CH_SR, CH_LFE]);
        assert_eq!(coded.mask, AC3_MASKS[14]);
        assert_eq!(coded.map, vec![Some(1), Some(2), Some(0), Some(5), Some(3), Some(4)], "reordered to FL FR FC LFE SL SR");
        assert_eq!(plan(&[CH_FL, CH_FR]).map, vec![Some(0), Some(1)]);
        assert_eq!(plan(&[CH_FC]).mask, bit(CH_FC));
    }

    /// Anything wider is reduced to at most 5.1 by identity, and 2/2 + LFE (no AC-3 layout) gets a
    /// silent centre rather than losing its LFE.
    #[test]
    fn a_wider_or_foreign_layout_is_reduced_to_an_ac3_one() {
        let six_one = plan(&[CH_FL, CH_FR, CH_FC, CH_LFE, CH_SL, CH_SR, 8]);
        assert_eq!((six_one.mask, six_one.map.clone()), (AC3_MASKS[14], (0..6).map(Some).collect()), "6.1 drops the back centre");
        let seven_one = plan(&[CH_FL, CH_FR, CH_FC, CH_LFE, CH_BL, CH_BR, CH_SL, CH_SR]);
        assert_eq!(seven_one.mask, AC3_MASKS[14]);
        assert_eq!(seven_one.map, vec![Some(0), Some(1), Some(2), Some(3), Some(6), Some(7)], "7.1 keeps the side pair");
        let quad_lfe = plan(&[CH_FL, CH_FR, CH_LFE, CH_SL, CH_SR]);
        assert_eq!(quad_lfe.mask, AC3_MASKS[14]);
        assert_eq!(quad_lfe.map, vec![Some(0), Some(1), None, Some(2), Some(3), Some(4)], "2/2+LFE: silent centre");
        let unnamed = plan(&[-1, -1, -1, -1, -1, -1]);
        assert_eq!((unnamed.mask, unnamed.channels()), (AC3_MASKS[14], 6), "an unnamed 6-plane layout is read as FFmpeg's 5.1");
        assert_eq!(plan(&[-1, -1, -1]).map, vec![Some(0), Some(1)], "an unnamed odd layout keeps a stereo pair");
        for input in [&[CH_FL, CH_FR, CH_LFE][..], &[CH_FL, CH_FR, CH_FC, CH_SL, CH_SR, 8], &[CH_FC, CH_LFE, 8]] {
            let p = plan(input);
            assert!(AC3_MASKS.contains(&p.mask), "{input:?} -> {:#x}", p.mask);
            assert_eq!(p.channels(), p.mask.count_ones() as usize);
            assert!(p.channels() <= 6);
        }
    }

    /// The FIFO keeps every plane the same length, fills silence for unmapped channels and hands
    /// out exactly what was queued, in order.
    #[test]
    fn the_planar_fifo_keeps_planes_aligned_and_in_order() {
        let p = ChannelPlan { mask: AC3_MASKS[14], map: vec![Some(1), Some(0), None, Some(2), Some(2), Some(2)] };
        let mut f = PlanarFifo::new(6);
        let a: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let b: Vec<f32> = (0..1000).map(|i| -(i as f32)).collect();
        let c = vec![0.5f32; 1000];
        f.push(&p, &[&a, &b, &c], 1000);
        f.push(&p, &[&a, &b, &c], 1000);
        assert_eq!(f.len(), 2000);
        let mut seen = vec![Vec::new(); 6];
        f.pop(AC3_FRAME_SAMPLES, |i, s| seen[i] = s.to_vec());
        assert_eq!(f.len(), 2000 - AC3_FRAME_SAMPLES);
        assert_eq!(seen[0][999], -999.0, "plane 0 takes input 1");
        assert_eq!(seen[1][1000], 0.0, "and continues with the second push");
        assert_eq!(seen[1][1535], 535.0);
        assert!(seen[2].iter().all(|&s| s == 0.0), "an unmapped channel is silence");
        f.pad_to(AC3_FRAME_SAMPLES);
        assert_eq!(f.len(), AC3_FRAME_SAMPLES);
        f.clear();
        assert!(f.is_empty());
    }

    /// Every decoder sample format becomes the same float planes: packed is de-interleaved, the
    /// integer widths are scaled to [-1, 1), and a short plane reads as silence.
    #[test]
    fn decoded_samples_of_any_format_become_float_planes() {
        let s16: Vec<u8> = [16_384i16, -32_768, 0, 8_192].iter().flat_map(|v| v.to_ne_bytes()).collect();
        assert_eq!(to_planar_f32(SampleKind::S16, false, 2, 2, &[&s16]), vec![vec![0.5, 0.0], vec![-1.0, 0.25]], "packed L R L R");
        let a: Vec<u8> = [1_073_741_824i32, -536_870_912].iter().flat_map(|v| v.to_ne_bytes()).collect();
        let b: Vec<u8> = [0.75f32, -0.5].iter().flat_map(|v| v.to_ne_bytes()).collect();
        assert_eq!(to_planar_f32(SampleKind::S32, true, 2, 2, &[&a, &a]), vec![vec![0.5, -0.25]; 2], "s32 planar");
        assert_eq!(to_planar_f32(SampleKind::F32, true, 1, 2, &[&b]), vec![vec![0.75, -0.5]]);
        let d: Vec<u8> = [0.25f64, 1.0, -0.125].iter().flat_map(|v| v.to_ne_bytes()).collect();
        assert_eq!(to_planar_f32(SampleKind::F64, false, 3, 1, &[&d]), vec![vec![0.25], vec![1.0], vec![-0.125]]);
        assert_eq!(to_planar_f32(SampleKind::U8, false, 1, 3, &[&[128, 0, 192]]), vec![vec![0.0, -1.0, 0.5]]);
        assert_eq!(to_planar_f32(SampleKind::F32, true, 2, 2, &[&b]), vec![vec![0.75, -0.5], vec![0.0, 0.0]], "a missing plane is silence");
        assert!(SAMPLE_FORMATS.iter().all(|(_, k, _)| k.bytes() > 0));
    }

    /// Counting samples gives monotonic, rounding-free times; a container pts within the bound is
    /// trusted to the count, one outside it starts a new timeline.
    #[test]
    fn the_sample_clock_counts_and_reanchors_only_past_the_bound() {
        let mut c = SampleClock::new(48_000);
        assert!(!c.on_packet(10_000_000_000, 0), "the first packet anchors, it does not re-anchor");
        assert_eq!(c.ns_of(0), 10_000_000_000);
        assert_eq!(c.ns_of(48_000), 11_000_000_000);
        assert_eq!(c.ns_of(-256), 10_000_000_000 - 5_333_333, "the encoder delay reads as just before the anchor");
        // Matroska's millisecond rounding: 512 samples = 10.666 ms, stamped 11 ms.
        assert!(!c.on_packet(10_011_000_000, 512));
        assert_eq!(c.ns_of(512), 10_010_666_666, "the count wins over the rounded stamp");
        assert!(c.on_packet(20_000_000_000, 1024), "a 10 s jump starts a new timeline");
        assert_eq!(c.ns_of(1024), 20_000_000_000);
        assert!(c.ns_of(1024 + 1536) > c.ns_of(1024));
        c.reset();
        assert!(!c.on_packet(5_000_000_000, 4096), "after a seek the next packet simply anchors");
        assert_eq!(c.ns_of(4096), 5_000_000_000);
        c.set_rate(44_100, 4096);
        assert_eq!(c.ns_of(4096 + 44_100), 6_000_000_000);
    }
}
