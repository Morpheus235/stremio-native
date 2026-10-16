//! **The probe before the Load**: open a stream with the bundled demuxer, read what the container
//! says about every stream, close it.
//!
//! The Starfish Load must declare the video codec, the raster, the frame rate, the Dolby Vision
//! layering and the audio codec BEFORE the first access unit is fed (`stream.rs`), and Stremio gives
//! none of it. So a start without a caller's declaration is opened here first, on the same
//! transports the demuxer uses (curl for https or any stream carrying request headers, the socket
//! with redirects followed otherwise), through the same AVIO callbacks, and `avformat_open_input` +
//! `avformat_find_stream_info` read the streams exactly as `demux` will read them a moment later.
//! What comes back is plain data ([`crate::stream::ProbedStreams`]); the declaration is derived
//! from it by the pure `stream::derive_declaration`.
//!
//! **Never a hang.** The probe runs on its own thread (`player::probe`), owns its transports, and
//! is cancelled through [`ProbeIo::cancel`]: the AU-lane abort flag the AVIO callbacks check, a
//! `shutdown` of the socket, and the private curl abort handle published before the first byte is
//! requested. `player::probe` bounds the whole probe in time and turns an overrun into
//! `ProbeFailure::Timeout`.
//!
//! **No read-out side effects.** The probe writes nothing into `SHARED`'s per-playback diagnostics
//! (the demuxer publishes those for the playback that actually runs), and its curl transfer never
//! registers as teardown's wake target (`curlio::ACTIVE`), so a probe still finishing after a
//! cancel can neither clobber nor deregister a demuxer's.

use super::*;
use crate::stream::{Dovi, ProbeFailure, ProbedAudio, ProbedStreams, ProbedVideo};
use std::sync::{Arc, Mutex};

/// The probe's transports and their cancellation, shared between the probe thread and whoever
/// may cancel it. Freed when the last holder drops it — which may be the probe thread, after a
/// cancel that did not wait for it — so nothing here is ever closed under a thread still using it.
pub struct ProbeIo {
    /// A private AU lane, used only for its abort flag: the AVIO callbacks check it on entry.
    lane: *mut AuQueue,
    /// The socket for a plaintext stream (unused, fd -1, for curl).
    hs: *mut HttpStream,
    /// The curl transfer's private abort handle, published before its first byte is requested.
    curl: Mutex<Option<Arc<crate::curlio::Abort>>>,
}

// The lane and the socket are built for cross-thread signalling (`aq_abort` and `http_shutdown`
// take their own locks); the probe thread is their only other user.
unsafe impl Send for ProbeIo {}
unsafe impl Sync for ProbeIo {}

impl ProbeIo {
    pub fn new() -> Arc<ProbeIo> {
        Arc::new(ProbeIo {
            lane: Box::into_raw(crate::aq::aq_new(1)),
            hs: Box::into_raw(sn_net::stream::http_stream_boxed()),
            curl: Mutex::new(None),
        })
    }

    /// Stop the probe wherever it is: the callbacks answer EOF from here on, a blocked socket read
    /// is woken by `shutdown`, a blocked `curl_multi_wait` by its abort pipe. Returns at once.
    pub fn cancel(&self) {
        crate::aq::aq_abort(self.lane);
        sn_net::stream::http_shutdown(self.hs);
        if let Some(a) = self.curl.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            a.signal();
        }
    }

    fn cancelled(&self) -> bool {
        unsafe { crate::aq::aq_is_aborted(self.lane) }
    }
}

impl Drop for ProbeIo {
    fn drop(&mut self) {
        // Last holder: no thread reads either transport any more.
        sn_net::stream::http_close(self.hs);
        unsafe {
            drop(Box::from_raw(self.hs));
            drop(Box::from_raw(self.lane));
        }
    }
}

/// Open `url` (with its `headers` on every request), read its streams, close it. Blocking: run it
/// off the main thread (`player::probe`).
/// How the probe reaches a stream: the demuxer's own transport rule (`demux`'s `via_curl` and its
/// playlist test), so the probe reads what the demuxer will.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeTransport {
    /// An HLS playlist, read through [`crate::hls`] as one byte stream.
    Hls,
    /// TLS, or a stream whose requests carry its own headers.
    Curl,
    /// Plaintext without headers.
    Socket,
}

fn probe_transport(url: &str, headers: &crate::headers::RequestHeaders) -> ProbeTransport {
    let (origin, _) = sn_net::net::origin::split(url);
    if crate::hls::is_playlist_url(url) {
        ProbeTransport::Hls
    } else if origin.is_tls() || !headers.is_empty() {
        ProbeTransport::Curl
    } else {
        ProbeTransport::Socket
    }
}

/// Open `url` as HLS for the probe. Unlike the demuxer's open it leaves `SHARED` alone (the armed
/// seek target belongs to the demuxer that follows) and cancels on the probe's own lane.
fn open_hls_for_probe(url: &str, headers: &crate::headers::RequestHeaders, io: &ProbeIo) -> Result<(Src, i64), ()> {
    let lane = io.lane as usize;
    let cancel = Box::new(move || unsafe { crate::aq::aq_is_aborted(lane as *mut AuQueue) });
    let transport = crate::hls::CurlTransport::new(headers.clone(), cancel);
    crate::hls::CurlHls::open(transport, crate::hls::SystemClock::new(), url, None, sn_platform::devcaps::caps())
        .map(|h| (Src::Hls(Box::new(h)), -1))
        .map_err(|e| crate::player::log(&format!("probe: hls open FAILED: {e}")))
}

pub fn probe(url: &str, headers: &crate::headers::RequestHeaders, io: &ProbeIo) -> Result<ProbedStreams, ProbeFailure> {
    if !ABI_OK.load(Ordering::Acquire) {
        crate::player::log("probe: refusing — FFmpeg is not loaded, or not the ABI built for");
        return Err(ProbeFailure::Unavailable);
    }
    if sn_net::net::origin::Origin::parse(url).is_none() {
        crate::player::log("probe: the URL is not an http(s) URL");
        return Err(ProbeFailure::Open);
    }
    ensure_registered();
    let began = std::time::Instant::now();
    let (origin, path) = sn_net::net::origin::split(url);
    let transport = probe_transport(url, headers);
    let publish = |a: &Arc<crate::curlio::Abort>| {
        *io.curl.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(a));
    };
    let opened = match transport {
        ProbeTransport::Hls => open_hls_for_probe(url, headers, io),
        ProbeTransport::Curl => crate::curlio::CurlSource::open_private_with(url, 0, headers, &publish)
            .map_err(|e| crate::player::log(&format!("probe: open FAILED: {e:?}")))
            .and_then(|cs| {
                // A playlist served from a URL that does not say so: the demuxer recognises it by
                // its Content-Type after the curl open, and so does the probe.
                if cs.content_type().is_some_and(crate::hls::is_playlist_content_type) {
                    drop(cs);
                    open_hls_for_probe(url, headers, io)
                } else {
                    let size = cs.size();
                    Ok((Src::Curl(cs), size))
                }
            }),
        ProbeTransport::Socket => {
        let mut hop = |hop_url: &str| crate::curlio::CurlSource::open_private_with(hop_url, 0, &crate::headers::RequestHeaders::none(), &publish);
        open_plain_following(io.hs, &origin, path, io.lane, &mut hop)
            .map(|(src, size, _)| (src, size))
            .map_err(|_| ())
        }
    };
    let Ok((src, size)) = opened else {
        return Err(ProbeFailure::Open);
    };
    if io.cancelled() {
        return Err(ProbeFailure::Open);
    }
    let mut state = Box::new(AvioState {
        src,
        aq: io.lane,
        off: 0,
        size,
        io_failed: false,
        bounce: Vec::new(),
        bounce_pos: 0,
        parked_at_end: false,
    });
    let result = unsafe { read_streams(&mut state) };
    crate::player::log(&format!(
        "probe: {} in {} ms",
        match &result {
            Ok(p) => format!(
                "v={} a={} ({})",
                p.video.as_ref().map_or("-", |v| v.codec.as_str()),
                p.audio.len(),
                p.audio.iter().map(|a| format!("{}/{}ch", a.codec, a.channels)).collect::<Vec<_>>().join(" ")
            ),
            Err(f) => format!("FAILED {f:?}"),
        },
        began.elapsed().as_millis()
    ));
    drop(state); // closes the curl source; the socket closes with `ProbeIo`
    result
}

/// The libavformat half: an AVIO over `state`, open, find the streams, copy them out, close.
unsafe fn read_streams(state: &mut Box<AvioState>) -> Result<ProbedStreams, ProbeFailure> {
    let buf = av_malloc(65536) as *mut u8;
    if buf.is_null() {
        return Err(ProbeFailure::Unavailable);
    }
    let avio = avio_alloc_context(
        buf,
        65536,
        0,
        &mut **state as *mut AvioState as *mut c_void,
        Some(read_cb),
        None,
        // An HLS stream is one forward byte stream with no positions to seek to, as in `demux`.
        if matches!(state.src, Src::Hls(_)) { None } else { Some(seek_cb) },
    );
    if avio.is_null() {
        free_ptr(buf as *mut c_void);
        return Err(ProbeFailure::Unavailable);
    }
    let mut fmt = avformat_alloc_context();
    if fmt.is_null() {
        free_avio(avio);
        return Err(ProbeFailure::Unavailable);
    }
    (*fmt).pb = avio;
    let r = avformat_open_input(&mut fmt, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut());
    if r < 0 || fmt.is_null() {
        crate::player::log(&format!("probe: open_input failed r={r}"));
        free_avio(avio);
        return Err(ProbeFailure::Open);
    }
    let r = avformat_find_stream_info(fmt, std::ptr::null_mut());
    if r < 0 {
        crate::player::log(&format!("probe: find_stream_info failed r={r}"));
        avformat_close_input(&mut fmt);
        free_avio(avio);
        return Err(ProbeFailure::StreamInfo);
    }
    let probed = collect(fmt);
    avformat_close_input(&mut fmt);
    free_avio(avio);
    Ok(probed)
}

/// A rational as a rate, or 0.0 when it is unknown (`0/x`, `x/0`, negative).
fn rate_of(r: AVRational) -> f64 {
    if r.num > 0 && r.den > 0 {
        f64::from(r.num) / f64::from(r.den)
    } else {
        0.0
    }
}

/// Copy every video and audio stream's facts out of an open context. Everything returned is
/// OWNED: the strings and records point into structs `avformat_close_input` frees.
unsafe fn collect(fmt: *mut AVFormatContext) -> ProbedStreams {
    let streams = (*fmt).streams;
    let vi = av_find_best_stream(fmt, AVMEDIA_TYPE_VIDEO, -1, -1, std::ptr::null_mut(), 0);
    let name = |id: c_int| std::ffi::CStr::from_ptr(avcodec_get_name(id)).to_string_lossy().to_ascii_lowercase();
    let video = (vi >= 0).then(|| {
        let st = *streams.add(vi as usize);
        let cp = stream_codecpar(st);
        let at = |off: usize| *((st as *const u8).add(off) as *const AVRational);
        let fps = match rate_of(at(OFF_STREAM_AVG_FRAME_RATE)) {
            f if f > 0.0 => f,
            _ => rate_of(at(OFF_STREAM_R_FRAME_RATE)),
        };
        let dovi = dovi_conf(cp).map_or(Dovi::NONE, |d| Dovi {
            present: true,
            profile: i64::from(d.dv_profile),
            bl_compat: i64::from(d.dv_bl_signal_compatibility_id),
            el_present: d.el_present_flag != 0,
        });
        ProbedVideo { codec: name((*cp).codec_id), width: (*cp).width, height: (*cp).height, fps, dovi }
    });
    let mut audio = Vec::new();
    for i in 0..(*fmt).nb_streams {
        let st = *streams.add(i as usize);
        let cp = stream_codecpar(st);
        if (*cp).codec_type != AVMEDIA_TYPE_AUDIO {
            continue;
        }
        let codec = name((*cp).codec_id);
        let disposition = *((st as *const u8).add(OFF_STREAM_DISPOSITION) as *const c_int);
        audio.push(ProbedAudio {
            atmos: codec == "eac3" && (*cp).profile == AV_PROFILE_EAC3_DDP_ATMOS,
            codec,
            channels: i64::from((*cp).ch_layout.nb_channels.max(0)),
            sample_rate: i64::from((*cp).sample_rate.max(0)),
            default: disposition & AV_DISPOSITION_DEFAULT != 0,
            name: stream_name(st),
            language: stream_language(st).unwrap_or_default(),
        });
    }
    let dur = fmt_duration(fmt);
    ProbedStreams { video, audio, duration_ns: if dur > 0 { dur.saturating_mul(1000) } else { 0 } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_rate_reads_as_zero() {
        assert_eq!(rate_of(AVRational { num: 24000, den: 1001 }), 24000.0 / 1001.0);
        for r in [(0, 1), (25, 0), (-1, 1), (0, 0)] {
            assert_eq!(rate_of(AVRational { num: r.0, den: r.1 }), 0.0, "{r:?}");
        }
    }

    /// With the ABI gate closed (no FFmpeg bound, as in the plain host suite) the probe refuses
    /// before it opens anything: an explicit failure, never a socket to nowhere.
    #[test]
    fn a_probe_without_ffmpeg_is_refused_before_any_io() {
        let _g = sn_base::testlock::serial();
        assert!(!abi_ok());
        let io = ProbeIo::new();
        assert_eq!(
            probe("http://127.0.0.1:1/film.mkv", &crate::headers::RequestHeaders::none(), &io),
            Err(ProbeFailure::Unavailable)
        );
        io.cancel();
        assert!(io.cancelled(), "cancel latches the lane every callback checks");
    }

    /// The probe reads a stream through the transport the demuxer will use: a playlist through
    /// the HLS client (a curl open would hand libavformat the playlist TEXT, the probe would fail,
    /// and every HLS start would be refused before its Load), TLS or a header-carrying stream
    /// through curl, anything else through the socket.
    #[test]
    fn the_probe_picks_the_demuxers_transport() {
        let none = crate::headers::RequestHeaders::none();
        let ua = crate::headers::RequestHeaders::from_pairs(&[("User-Agent", "Live/1")]).unwrap();
        assert_eq!(probe_transport("http://live.example/ch/1/index.m3u8", &none), ProbeTransport::Hls);
        assert_eq!(probe_transport("https://live.example/ch/1/index.m3u8?token=x", &ua), ProbeTransport::Hls);
        assert_eq!(probe_transport("https://cdn.example/film.mkv", &none), ProbeTransport::Curl);
        assert_eq!(probe_transport("http://cdn.example/film.mkv", &ua), ProbeTransport::Curl);
        assert_eq!(probe_transport("http://cdn.example/film.mkv", &none), ProbeTransport::Socket);
    }
}
