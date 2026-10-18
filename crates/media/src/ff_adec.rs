//! **Software audio decode -> PCM** — the path behind [`crate::stream::AudioFeed::SoftPcm`].
//!
//! Ported from PlxNative's `media/src/adec.rs`. The codecs the television's buffer feed has no
//! decoder for (FLAC, TrueHD/MLP, the LPCM variants of a Blu-ray remux, Opus, Vorbis, MP2, WMA,
//! ALAC, and MP3 where the set's table has no MPEG audio row) are decoded by the bundled libavcodec through `libsnadec.so` (`native/adec.c`), which
//! also resamples and remixes with libswresample to S16LE 48 kHz — stereo, or "6-channel" 5.1 for
//! a source of six channels or more — and the Load declares `"PCM"` with a `pcmInfo` node of
//! exactly that shape. The shim owns every FFmpeg struct it touches, so nothing here models an
//! offset: the demuxer hands over the `AVCodecParameters` and `AVPacket` pointers it already holds.
//!
//! # Timestamps
//!
//! Decoded PCM is batched into access units of at least [`PCM_BATCH_MS`] (TrueHD decodes 0.83 ms
//! frames; one Starfish feed per frame would be ~1200 calls a second) and each batch is stamped by
//! COUNTING its 48 kHz samples on the transcoder's [`SampleClock`], anchored at the decoder's first
//! frame pts: strictly monotonic through Matroska's millisecond rounding, re-anchored only past
//! `REANCHOR_NS` (a gap in the file). A seek calls [`PcmFeed::reset`].

use super::transcode::SampleClock;
use super::*;

sn_base::dynlib! {
    native_adec: ["libsnadec.so", "libsnadec.dylib"] {
        fn sn_adec_abi_version() -> u32;
        fn sn_adec_last_error() -> i32;
        fn sn_adec_lavc_version() -> u32;
        fn sn_adec_has_decoder(name: *const c_char) -> i32;
        fn sn_adec_open(par: *const AVCodecParameters, tb_num: i32, tb_den: i32, out_ch: i32) -> *mut c_void;
        fn sn_adec_out_channels(d: *const c_void) -> i32;
        fn sn_adec_flush(d: *mut c_void);
        fn sn_adec_close(d: *mut c_void);
        fn sn_adec_decode(d: *mut c_void, pkt: *const AVPacket, out: *mut *const u8, len: *mut usize, pts: *mut i64) -> i32;
    }
}

/// `native/adec.c`'s `SN_ADEC_ABI`: 2 added [`DPLII`] to `sn_adec_open`'s `out_ch`.
const ABI: u32 = 2;

/// The codecs this path decodes, in FFmpeg's names (`avcodec_get_name`) — exactly the decoders
/// `ci/build-ffmpeg.sh` enables for it. `ff_adec_tests` proves each one is in the bundled build.
pub const SOFT_CODECS: &[&str] = &[
    "flac", "truehd", "mlp", "opus", "vorbis", "mp3", "mp2", "alac", "wmav1", "wmav2", "wmapro", "pcm_s16le", "pcm_s16be", "pcm_s24le",
    "pcm_s24be", "pcm_s32le", "pcm_f32le", "pcm_bluray", "pcm_dvd",
];

/// PURE: does this path decode `codec` (FFmpeg's name, any case)?
pub fn is_soft(codec: &str) -> bool {
    SOFT_CODECS.iter().any(|c| c.eq_ignore_ascii_case(codec))
}

/// Or-ed into the channel count handed to [`PcmFeed::open`] (`sn_adec_open`'s `out_ch`): fold to a
/// Dolby Pro Logic II matrixed stereo pair (libswresample's `matrix_encoding=dplii`, its default
/// centre and surround levels, LFE dropped) for a Pro Logic II receiver on a stereo link.
pub const DPLII: u32 = 0x100;

/// Every PCM feed is 48 kHz: the shim resamples.
pub const PCM_RATE: u32 = 48_000;
/// The smallest PCM access unit fed, in ms (decoded frames are batched up to it).
pub const PCM_BATCH_MS: u32 = 20;

/// PURE: the channels the sink is told for a source of `src` channels: 6 for 5.1 and up, else
/// stereo (0 = unknown = stereo). Must agree with `out_ch` in `native/adec.c`.
pub fn sink_channels(src: i64) -> u32 {
    if src >= 6 { 6 } else { 2 }
}

/// PURE: the `pcmInfo` object of a Load payload (shape from webosbrew's ss4s SMP driver, as
/// PlxNative fed it).
pub fn pcm_info_json(channels: u32, rate: u32) -> String {
    let mode = match channels {
        1 => "mono",
        6 => "6-channel",
        _ => "stereo",
    };
    // sampleRate is in kHz, as a number: 48, 44.1.
    let khz = rate as f64 / 1000.0;
    format!(r#"{{"channelMode":"{mode}","format":"S16LE","sampleRate":{khz},"bitsPerSample":16,"layout":"interleaved"}}"#)
}

pub(super) static AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Did `libsnadec.so` load with the ABI this build expects? Latched by [`detect`] at `ff::boot`;
/// `false` keeps the explicit `AudioUnsupported` refusal for these codecs.
pub fn available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}

/// Load the shim from the app directory (after the bundled FFmpeg, which it NEEDs by SONAME).
pub(super) fn detect() -> bool {
    let ok = ABI_OK.load(Ordering::Acquire) && load_from(&sn_base::paths::app_dir());
    AVAILABLE.store(ok, Ordering::Release);
    crate::player::log(&format!(
        "ff: software audio decode -> PCM {}",
        if ok { "available (libsnadec)" } else { "UNAVAILABLE — FLAC/TrueHD/LPCM/Opus/... are refused" }
    ));
    ok
}

pub(super) fn load_from(dir: &std::path::Path) -> bool {
    // The television's own media libraries can already hold the SYSTEM libavcodec in the global
    // scope, and the shim's avcodec_* imports would bind to it (another major, renumbered codec
    // ids — PlxNative saw find_decoder(OPUS) come back NULL). DEEPBIND makes the shim resolve
    // against its own DT_NEEDED -sn libraries first; the dynlib load below reuses this handle.
    #[cfg(target_os = "linux")]
    if let Some(p) = dir.join("libsnadec.so").to_str().and_then(|p| CString::new(p).ok()) {
        unsafe { libc::dlopen(p.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL | libc::RTLD_DEEPBIND) };
    }
    match native_adec::load(Some(dir)) {
        sn_base::dynlib::Loaded::Ok(_) => {
            let abi = unsafe { sn_adec_abi_version() };
            if abi != ABI {
                crate::player::log(&format!("ff: libsnadec ABI {abi}, want {ABI}"));
            }
            abi == ABI
        }
        sn_base::dynlib::Loaded::NoLibrary => false,
        sn_base::dynlib::Loaded::Incomplete(_, n) => {
            crate::player::log(&format!("ff: libsnadec missing {n} symbols"));
            false
        }
    }
}

/// Does the loaded libavcodec decode `codec` (FFmpeg's name)? Only once [`available`].
pub fn has_decoder(codec: &str) -> bool {
    available() && CString::new(codec).is_ok_and(|c| unsafe { sn_adec_has_decoder(c.as_ptr()) } != 0)
}

/// One PCM access unit for the audio lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmAu {
    pub data: Vec<u8>,
    pub pts_ns: i64,
}

/// PURE: decoded S16 interleaved chunks in, sample-counted access units of at least
/// [`PCM_BATCH_MS`] out (see the module doc).
#[derive(Debug)]
pub struct PcmBatcher {
    frame_bytes: usize,
    clock: SampleClock,
    batch: Vec<u8>,
    /// Absolute index of the first sample in `batch`.
    batch_start: i64,
}

impl PcmBatcher {
    pub fn new(channels: u32) -> PcmBatcher {
        PcmBatcher { frame_bytes: channels.max(1) as usize * 2, clock: SampleClock::new(PCM_RATE as i32), batch: Vec::new(), batch_start: 0 }
    }

    fn min_bytes(&self) -> usize {
        PCM_RATE as usize * PCM_BATCH_MS as usize / 1000 * self.frame_bytes
    }

    /// Queue one decode's output, whose first sample the decoder stamps `pts_ns`.
    pub fn push(&mut self, pcm: &[u8], pts_ns: i64) -> Vec<PcmAu> {
        let mut out = Vec::new();
        if pcm.is_empty() {
            return out;
        }
        let end = self.batch_start + (self.batch.len() / self.frame_bytes) as i64;
        let old_pts = self.clock.ns_of(self.batch_start);
        if self.clock.on_packet(pts_ns, end) && !self.batch.is_empty() {
            // A new timeline: what was queued plays at the old one's time.
            out.push(PcmAu { data: std::mem::take(&mut self.batch), pts_ns: old_pts });
        }
        if self.batch.is_empty() {
            self.batch_start = end;
        }
        self.batch.extend_from_slice(&pcm[..pcm.len() - pcm.len() % self.frame_bytes]);
        if self.batch.len() >= self.min_bytes() {
            out.extend(self.take());
        }
        out
    }

    /// What is queued, however short (the end of the stream).
    pub fn take(&mut self) -> Option<PcmAu> {
        if self.batch.is_empty() {
            return None;
        }
        let pts_ns = self.clock.ns_of(self.batch_start);
        self.batch_start += (self.batch.len() / self.frame_bytes) as i64;
        Some(PcmAu { data: std::mem::take(&mut self.batch), pts_ns })
    }

    /// A seek: drop what is queued; the next decode anchors afresh.
    pub fn reset(&mut self) {
        self.batch_start += (self.batch.len() / self.frame_bytes) as i64;
        self.batch.clear();
        self.clock.reset();
    }
}

/// The decoder plus its batcher, owned by the demux thread.
pub struct PcmFeed {
    d: *mut c_void,
    tb: AVRational,
    batcher: PcmBatcher,
    bad: u32,
    pub channels: u32,
}

// The FFmpeg objects are touched by the one thread that owns the value.
unsafe impl Send for PcmFeed {}

impl Drop for PcmFeed {
    fn drop(&mut self) {
        unsafe { sn_adec_close(self.d) }
    }
}

/// Consecutive undecodable packets tolerated before the feed gives up.
const MAX_BAD_PACKETS: u32 = 32;

impl PcmFeed {
    /// Open a decoder for the stream `par` with time base `tb`, producing `channels` (2 or 6, or
    /// `2 | DPLII`).
    ///
    /// # Safety
    /// `par` is a live `AVCodecParameters` of the bundled FFmpeg.
    pub unsafe fn open(par: *const AVCodecParameters, tb: AVRational, channels: u32) -> Result<PcmFeed, String> {
        if !available() {
            return Err("libsnadec unavailable".into());
        }
        let d = sn_adec_open(par, tb.num, tb.den, channels as i32);
        if d.is_null() {
            return Err(format!("open failed (stage {:#x}, lavc {:#x})", sn_adec_last_error(), sn_adec_lavc_version()));
        }
        let channels = sn_adec_out_channels(d) as u32;
        Ok(PcmFeed { d, tb, batcher: PcmBatcher::new(channels), bad: 0, channels })
    }

    /// Decode one packet (null: drain at the end); `fallback_ns` is its container pts in ns, for a
    /// decode the decoder could not stamp.
    ///
    /// # Safety
    /// `pkt` is null or a live `AVPacket` of the bundled FFmpeg.
    pub unsafe fn push(&mut self, pkt: *const AVPacket, fallback_ns: i64) -> Result<Vec<PcmAu>, String> {
        let (mut out, mut len, mut pts) = (std::ptr::null(), 0usize, AV_NOPTS_VALUE);
        let r = sn_adec_decode(self.d, pkt, &mut out, &mut len, &mut pts);
        if r < 0 {
            self.bad += 1;
            if self.bad <= 3 {
                crate::player::log(&format!("ff: soft audio: packet did not decode (r={r}); skipped"));
            }
            return if self.bad >= MAX_BAD_PACKETS { Err(format!("{} packets in a row did not decode (r={r})", self.bad)) } else { Ok(Vec::new()) };
        }
        self.bad = 0;
        let pcm = if out.is_null() || len == 0 { &[][..] } else { std::slice::from_raw_parts(out, len) };
        let at = if pts == AV_NOPTS_VALUE { fallback_ns } else { av_rescale_q(pts, self.tb, NS_TB) };
        Ok(self.batcher.push(pcm, at))
    }

    /// The end of the stream: drain the decoder and hand over the last short batch.
    pub fn finish(&mut self) -> Vec<PcmAu> {
        let mut out = unsafe { self.push(std::ptr::null(), 0) }.unwrap_or_default();
        out.extend(self.batcher.take());
        out
    }

    /// A seek: flush the decoder and resampler, drop the queued batch, re-anchor.
    pub fn reset(&mut self) {
        unsafe { sn_adec_flush(self.d) };
        self.batcher.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_info_is_the_ss4s_shape() {
        assert_eq!(
            pcm_info_json(6, 48_000),
            r#"{"channelMode":"6-channel","format":"S16LE","sampleRate":48,"bitsPerSample":16,"layout":"interleaved"}"#
        );
        assert!(pcm_info_json(2, 44_100).contains(r#""sampleRate":44.1"#));
    }

    #[test]
    fn the_sink_shape_and_the_codec_list() {
        assert_eq!((sink_channels(8), sink_channels(6), sink_channels(5), sink_channels(0)), (6, 6, 2, 2));
        assert!(is_soft("FLAC") && is_soft("truehd") && is_soft("pcm_bluray") && is_soft("opus"));
        assert!(!is_soft("aac") && !is_soft("ac3") && !is_soft("eac3") && !is_soft("dts") && !is_soft("") && !is_soft("cook"));
        assert!(is_soft("wmav2") && is_soft("wmapro"), "WMA has no buffer-feed caps without its wmaInfo; decoded here");
    }

    /// TrueHD-sized decodes (40 samples, 0.83 ms) stamped to the millisecond come out as >= 20 ms
    /// AUs whose pts advance by EXACTLY their sample count — no rounding drift, no gap, no overlap.
    #[test]
    fn batches_are_sample_counted_and_continuous() {
        let mut b = PcmBatcher::new(6);
        let chunk = vec![0u8; 40 * 12];
        let mut aus = Vec::new();
        for i in 0..2400i64 {
            // 2 s of audio from 10 s; the container rounds each stamp to the ms.
            let exact = 10_000_000_000 + i * 40 * 1_000_000_000 / 48_000;
            aus.extend(b.push(&chunk, exact / 1_000_000 * 1_000_000));
        }
        aus.extend(b.take());
        assert_eq!(aus[0].pts_ns, 10_000_000_000);
        let mut next = aus[0].pts_ns;
        let mut samples = 0i64;
        for au in &aus {
            assert_eq!(au.data.len() % 12, 0);
            assert_eq!(au.pts_ns, 10_000_000_000 + samples * 1_000_000_000 / 48_000, "pts is the sample count");
            assert!(au.pts_ns >= next);
            next = au.pts_ns;
            samples += (au.data.len() / 12) as i64;
        }
        assert_eq!(samples, 2400 * 40, "every sample is fed exactly once");
        assert!(aus[..aus.len() - 1].iter().all(|a| a.data.len() >= 960 * 12), "AUs are >= 20 ms");
    }

    /// A jump in the container's time (a gap, or a seek the clock was not told about) flushes what
    /// was queued at its own time and starts the new timeline at the new stamp; `reset` (a seek)
    /// drops the queue and anchors at the next decode.
    #[test]
    fn a_new_timeline_reanchors_and_a_seek_drops_the_queue() {
        let mut b = PcmBatcher::new(2);
        assert!(b.push(&[0u8; 400], 1_000_000_000).is_empty());
        let out = b.push(&[0u8; 400], 9_000_000_000);
        assert_eq!(out, vec![PcmAu { data: vec![0u8; 400], pts_ns: 1_000_000_000 }], "the old batch keeps its time");
        assert_eq!(b.take().map(|a| a.pts_ns), Some(9_000_000_000));
        b.push(&[0u8; 400], 9_002_083_333);
        b.reset();
        assert_eq!(b.take(), None, "a seek drops the queue");
        assert!(b.push(&[0u8; 400], 3_000_000_000).is_empty());
        assert_eq!(b.take().map(|a| a.pts_ns), Some(3_000_000_000), "after a seek the next decode anchors");
    }
}
