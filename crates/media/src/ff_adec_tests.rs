//! The software decode -> PCM path through the FFmpeg the app SHIPS and the shim built against it
//! (`HOST=1 ci/build-ffmpeg.sh` then `HOST=1 ci/build-adec.sh`; the dylibs/ELFs staged in one
//! directory named by `SN_FFMPEG_DIR`). `#[ignore]`d everywhere else, like `ff_transcode_tests`.
//! The fixtures are written by the `ffmpeg` CLI and the tests skip, loudly, without it. Run with
//! `SN_FFMPEG_DIR=<dir> CARGO_INCREMENTAL=0 cargo test -p sn_media --lib ff::adec_tests -- --ignored --nocapture`.

use super::adec::{self, PcmFeed};
use super::image_subtitle_tests::{bind_host_ffmpeg, host_ffmpeg_dir, HostFfmpeg};
use super::late_audio_tests::{tool_present, FileServer, Scratch};
use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The host FFmpeg and the shim bound, the path detected; undone however the test leaves.
struct Bound {
    _ffmpeg: HostFfmpeg,
}

impl Bound {
    fn new() -> Bound {
        let ffmpeg = bind_host_ffmpeg();
        assert!(adec::load_from(&host_ffmpeg_dir()), "no libsnadec in {} — run `HOST=1 ci/build-adec.sh`", host_ffmpeg_dir().display());
        adec::AVAILABLE.store(true, Ordering::Release);
        Bound { _ffmpeg: ffmpeg }
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        adec::AVAILABLE.store(false, Ordering::Release);
    }
}

/// Every codec the feed decision sends down this path has a decoder in the bundled build: a
/// codec missing from `ci/build-ffmpeg.sh` would be declared PCM and then fail to open.
#[test]
#[ignore = "requires the bundled host FFmpeg and libsnadec"]
fn the_bundled_build_decodes_every_soft_codec() {
    let _g = sn_base::testlock::serial();
    let _b = Bound::new();
    let missing: Vec<_> = adec::SOFT_CODECS.iter().filter(|c| !adec::has_decoder(c)).collect();
    assert!(missing.is_empty(), "no decoder for {missing:?}");
    assert!(!adec::has_decoder("cook"), "and the check is not vacuous");
}

/// Write a 2 s Matroska: 640x360 H.264 at 25 fps plus one audio track (`codec`, `layout`).
fn generate_mkv(dir: &Path, name: &str, codec: &str, layout: &str) -> PathBuf {
    let out = dir.join(name);
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=s=640x360:r=25:d=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
        .args(["-map", "0:v", "-map", "1:a"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-g", "25"])
        .args(["-af", &format!("aformat=channel_layouts={layout}")])
        .args(["-c:a", codec, "-strict", "-2"])
        .arg(&out)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg could not write {name}");
    out
}

/// `ff::demux` with `channels` of PCM decoding `file`: the audio AUs it pushed.
fn demux_pcm(file: &Path, channels: u32, acodec: &str) -> Vec<(Vec<u8>, i64)> {
    demux_audio(file, channels, false, acodec)
}

/// [`demux_pcm`], or with `ac3` the session's decode -> AC-3 decision (`pcm_channels` 0).
fn demux_audio(file: &Path, channels: u32, ac3: bool, acodec: &str) -> Vec<(Vec<u8>, i64)> {
    let server = FileServer::start(file);
    SHARED.seek_to_ns.store(-1, Ordering::Relaxed);
    SHARED.desired_audio_idx.store(-1, Ordering::Relaxed);
    SHARED.side_subs_owner.store(false, Ordering::Relaxed);
    SHARED.demux_failed.store(false, Ordering::Relaxed);
    SHARED.demux_io_failed.store(false, Ordering::Relaxed);
    FEED_AUDIO.store(true, Ordering::Relaxed);
    set_soft_pcm(channels);
    set_transcode_ac3(ac3);
    let (video_cap, audio_cap) = crate::player::engine::aq_caps();
    let mut video = crate::aq::aq_new(video_cap as c_long);
    let mut audio = crate::aq::aq_new(audio_cap as c_long);
    let mut hs = sn_net::stream::http_stream_boxed();
    let (vp, ap, hp) = (
        SendPtr(&mut *video as *mut AuQueue),
        SendPtr(&mut *audio as *mut AuQueue),
        SendPtr(&mut *hs as *mut HttpStream),
    );
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    let url = format!("http://127.0.0.1:{}/media/{name}", server.port);
    let acodec = acodec.to_string();
    let th = std::thread::spawn(move || {
        let off = sn_base::task::OffFrame::for_test();
        demux(&off, url, crate::headers::RequestHeaders::none(), acodec, vp, ap, hp)
    });
    let pop_all = |q: &mut AuQueue, out: &mut Vec<(Vec<u8>, i64)>| loop {
        let node = crate::aq::aq_pop(q, std::ptr::null_mut());
        if node.is_null() {
            break;
        }
        let (_es, _key, pts, len, data) = unsafe { crate::aq::au_fields(node) };
        out.push((unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec(), pts));
        unsafe { libc::free(node as *mut c_void) };
    };
    let (mut aus, mut vids) = (Vec::new(), Vec::new());
    let deadline = Instant::now() + Duration::from_secs(20);
    while !th.is_finished() {
        assert!(Instant::now() < deadline, "the demuxer did not finish a 2 s file");
        pop_all(&mut audio, &mut aus);
        pop_all(&mut video, &mut vids);
        std::thread::sleep(Duration::from_millis(5));
    }
    th.join().unwrap();
    pop_all(&mut audio, &mut aus);
    pop_all(&mut video, &mut vids);
    set_soft_pcm(0);
    set_transcode_ac3(false);
    assert!(!SHARED.demux_failed.load(Ordering::Acquire), "the software audio demux must not fail");
    assert!(vids.len() >= 45, "the video lane carries the video ({} AUs)", vids.len());
    aus
}

/// PCM AUs: whole frames, >= 20 ms but the last, pts advancing by exactly their sample count from
/// the track start, and the track's whole 2 s delivered.
fn assert_continuous_pcm(aus: &[(Vec<u8>, i64)], channels: usize) {
    let frame = channels * 2;
    let mut samples = 0i64;
    for (i, (data, pts)) in aus.iter().enumerate() {
        assert_eq!(data.len() % frame, 0, "AU {i}: whole sample frames");
        if i + 1 < aus.len() {
            assert!(data.len() >= 960 * frame, "AU {i}: >= 20 ms ({} bytes)", data.len());
        }
        let want = aus[0].1 + samples * 1_000_000_000 / 48_000;
        assert!((pts - want).abs() <= 1, "AU {i}: pts {pts} vs sample count {want}");
        samples += (data.len() / frame) as i64;
    }
    assert!(aus[0].1.abs() < 30_000_000, "the first AU sits at the track start ({} ns)", aus[0].1);
    assert!((95_000..=97_000).contains(&samples), "2 s at 48 kHz, got {samples} samples");
    assert!(aus.iter().any(|(d, _)| d.iter().any(|&b| b != 0)), "not silence");
}

/// **The C1 beta's file, end to end:** H.264 + FLAC stereo. `ff::demux` with the session's PCM
/// decision feeds S16LE stereo 48 kHz, sample-counted and continuous, for the whole track.
#[test]
#[ignore = "needs the host build of the bundled FFmpeg, libsnadec and the ffmpeg CLI"]
fn demux_feeds_a_flac_track_as_stereo_pcm() {
    let _g = sn_base::testlock::serial();
    if !tool_present("ffmpeg") {
        eprintln!("SKIP: the ffmpeg command line tool is not installed");
        return;
    }
    let _b = Bound::new();
    let scratch = Scratch::new("adec-flac");
    let file = generate_mkv(&scratch.0, "flac2.mkv", "flac", "stereo");
    assert_continuous_pcm(&demux_pcm(&file, 2, "flac"), 2);
}

/// A Blu-ray remux's LPCM 7.1 (24-bit) is folded to the declared 5.1 by swresample.
#[test]
#[ignore = "needs the host build of the bundled FFmpeg, libsnadec and the ffmpeg CLI"]
fn demux_feeds_lpcm_7_1_as_5_1_pcm() {
    let _g = sn_base::testlock::serial();
    if !tool_present("ffmpeg") {
        eprintln!("SKIP: the ffmpeg command line tool is not installed");
        return;
    }
    let _b = Bound::new();
    let scratch = Scratch::new("adec-lpcm");
    let file = generate_mkv(&scratch.0, "lpcm71.mkv", "pcm_s24le", "7.1");
    assert_continuous_pcm(&demux_pcm(&file, 6, "pcm_s24le"), 6);
}

/// **The CPU budget, measured through the shipping path** (shim + bundled FFmpeg): decode +
/// resample cost per second of audio, single-threaded, for each file in `SN_ADEC_BENCH` (a
/// `:`-separated list of `path=channels`). Prints, asserts nothing but that it decoded.
#[test]
#[ignore = "a measurement: set SN_ADEC_BENCH and run with --nocapture"]
fn soft_decode_cost_per_second_of_audio() {
    let _g = sn_base::testlock::serial();
    let Ok(list) = std::env::var("SN_ADEC_BENCH") else {
        eprintln!("SKIP: SN_ADEC_BENCH not set");
        return;
    };
    let _b = Bound::new();
    for item in list.split(':') {
        let (path, ch) = item.rsplit_once('=').expect("path=channels");
        let ch: u32 = ch.parse().unwrap();
        unsafe {
            let mut fmt: *mut AVFormatContext = std::ptr::null_mut();
            let c = CString::new(path).unwrap();
            assert!(avformat_open_input(&mut fmt, c.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut()) >= 0, "open {path}");
            assert!(avformat_find_stream_info(fmt, std::ptr::null_mut()) >= 0);
            let ai = av_find_best_stream(fmt, AVMEDIA_TYPE_AUDIO, -1, -1, std::ptr::null_mut(), 0);
            assert!(ai >= 0);
            let st = *(*fmt).streams.add(ai as usize);
            let mut feed = PcmFeed::open(stream_codecpar(st), stream_time_base(st), ch).expect("open the decoder");
            let pkt = av_packet_alloc();
            let (mut busy, mut bytes) = (Duration::ZERO, 0usize);
            while av_read_frame(fmt, pkt) >= 0 {
                if (*pkt).stream_index == ai {
                    let t = Instant::now();
                    let out = feed.push(pkt, 0).expect("decode");
                    busy += t.elapsed();
                    bytes += out.iter().map(|a| a.data.len()).sum::<usize>();
                }
                av_packet_unref(pkt);
            }
            let mut pkt_m = pkt;
            av_packet_free(&mut pkt_m);
            avformat_close_input(&mut fmt);
            let secs = bytes as f64 / (48_000.0 * f64::from(feed.channels) * 2.0);
            assert!(secs > 1.0, "{path}: decoded only {secs:.2} s");
            eprintln!(
                "BENCH {path} -> PCM {}ch: {:.3} ms CPU per s of audio ({:.0}x realtime) over {secs:.0} s",
                feed.channels,
                busy.as_secs_f64() * 1000.0 / secs,
                secs / busy.as_secs_f64()
            );
        }
    }
}

/// One tone per channel, so a channel is identified by its frequency wherever it lands.
const TONES: [(&str, u32); 6] = [("FL", 300), ("FR", 500), ("FC", 700), ("LFE", 100), ("SL", 1100), ("SR", 1300)];

/// Write a 2 s Matroska (H.264 + one `codec` 5.1(side) track) carrying [`TONES`].
fn generate_tones_mkv(dir: &Path, name: &str, codec: &str) -> PathBuf {
    let out = dir.join(name);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=s=640x360:r=25:d=2"]);
    for (_, hz) in TONES {
        cmd.args(["-f", "lavfi", "-i", &format!("sine=frequency={hz}:sample_rate=48000:duration=2")]);
    }
    let map: Vec<String> = TONES.iter().enumerate().map(|(i, (ch, _))| format!("{i}.0-{ch}")).collect();
    let filter = format!(
        "{}join=inputs=6:channel_layout=5.1(side):map={}[a]",
        (1..=6).map(|i| format!("[{i}:a]")).collect::<String>(),
        map.join("|")
    );
    let status = cmd
        .args(["-filter_complex", &filter, "-map", "0:v", "-map", "[a]"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-g", "25"])
        .args(["-c:a", codec, "-strict", "-2"])
        .arg(&out)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg could not write {name}");
    out
}

/// The amplitude of `hz` in one channel of interleaved S16 `pcm` (a single-bin DFT over 0.1 s
/// from 0.5 s in, past any decoder/resampler start-up).
fn tone_level(pcm: &[i16], channels: usize, ch: usize, hz: u32) -> f64 {
    let (re, im) = tone_bin(pcm, channels, ch, hz);
    (re * re + im * im).sqrt()
}

/// [`tone_level`]'s DFT bin itself (normalised), so two channels' phases can be compared.
fn tone_bin(pcm: &[i16], channels: usize, ch: usize, hz: u32) -> (f64, f64) {
    let x: Vec<f64> = pcm.iter().skip(ch).step_by(channels).skip(24_000).take(4_800).map(|&s| f64::from(s)).collect();
    let w = 2.0 * std::f64::consts::PI * f64::from(hz) / 48_000.0;
    let (re, im) = x.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, v)| (re + v * (w * i as f64).cos(), im - v * (w * i as f64).sin()));
    (re / x.len() as f64, im / x.len() as f64)
}

/// The correlation of one tone between two channels: +1 in phase, -1 in opposite phase.
fn tone_correlation(pcm: &[i16], channels: usize, a: usize, b: usize, hz: u32) -> f64 {
    let ((ar, ai), (br, bi)) = (tone_bin(pcm, channels, a, hz), tone_bin(pcm, channels, b, hz));
    (ar * br + ai * bi) / ((ar * ar + ai * ai).sqrt() * (br * br + bi * bi).sqrt())
}

/// The host libavcodec's AC-3 path detected for a test, and undetected however it leaves.
struct Ac3Path;

impl Ac3Path {
    fn new() -> Ac3Path {
        assert!(transcode::detect() && transcode::decoders() != 0, "the bundled libavcodec must carry the ac3 encoder");
        Ac3Path
    }
}

impl Drop for Ac3Path {
    fn drop(&mut self) {
        let abi = ABI_OK.swap(false, Ordering::AcqRel);
        transcode::detect();
        ABI_OK.store(abi, Ordering::Release);
    }
}

/// **TrueHD 5.1 keeps every channel in its place.** A Blu-ray TrueHD 5.1 track carries a stereo
/// downmix substream and the 5.1 one; decoded to the declared "6-channel" PCM, each output slot
/// (FL FR FC LFE SL SR) must hold its own source channel and nothing else.
#[test]
#[ignore = "needs the host build of the bundled FFmpeg, libsnadec and the ffmpeg CLI"]
fn demux_feeds_truehd_5_1_with_every_channel_in_place() {
    let _g = sn_base::testlock::serial();
    if !tool_present("ffmpeg") {
        eprintln!("SKIP: the ffmpeg command line tool is not installed");
        return;
    }
    let _b = Bound::new();
    let scratch = Scratch::new("adec-truehd");
    let file = generate_tones_mkv(&scratch.0, "thd51.mkv", "truehd");
    let aus = demux_pcm(&file, 6, "truehd");
    assert_continuous_pcm(&aus, 6);
    let pcm: Vec<i16> = aus.iter().flat_map(|(d, _)| d.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]))).collect();
    for (slot, (name, hz)) in TONES.iter().enumerate() {
        let own = tone_level(&pcm, 6, slot, *hz);
        assert!(own > 500.0, "{name} (slot {slot}) lost its {hz} Hz tone ({own:.0})");
        for (other, ohz) in TONES.iter().filter(|(_, h)| h != hz) {
            let leak = tone_level(&pcm, 6, slot, *ohz);
            assert!(leak < own / 10.0, "{name} (slot {slot}) carries {other}'s {ohz} Hz tone ({leak:.0} vs {own:.0})");
        }
    }
}

/// **The optical-link fix, end to end.** TrueHD 5.1 through `ff::demux` with the session's
/// decode -> AC-3 decision: the audio lane carries 640 kb/s AC-3 5.1 with monotonic pts, and that
/// AC-3, decoded back by the `ffmpeg` CLI, holds every source tone in its own channel.
#[test]
#[ignore = "needs the host build of the bundled FFmpeg, libsnadec and the ffmpeg CLI"]
fn demux_feeds_truehd_5_1_as_ac3_with_every_channel_in_place() {
    let _g = sn_base::testlock::serial();
    if !tool_present("ffmpeg") {
        eprintln!("SKIP: the ffmpeg command line tool is not installed");
        return;
    }
    let _b = Bound::new();
    let _ac3 = Ac3Path::new();
    assert!(crate::stream::SoftwareAudio::detected().to_ac3("truehd"), "the truehd decoder and the ac3 encoder");
    let scratch = Scratch::new("adec-truehd-ac3");
    let file = generate_tones_mkv(&scratch.0, "thd51.mkv", "truehd");
    let aus = demux_audio(&file, 0, true, "truehd");
    assert!((60..=64).contains(&aus.len()), "2 s is ~63 AC-3 frames, got {}", aus.len());
    for (i, (d, _)) in aus.iter().enumerate() {
        assert_eq!((&d[..2], d.len()), (&[0x0b, 0x77][..], 2560), "AU {i}: a 640 kb/s 48 kHz AC-3 frame");
    }
    for w in aus.windows(2) {
        assert!((w[1].1 - w[0].1 - 32_000_000).abs() <= 1, "one AC-3 frame apart: {} -> {}", w[0].1, w[1].1);
    }
    assert!(aus[0].1.abs() < 20_000_000, "the first AC-3 frame sits at the track start ({} ns)", aus[0].1);
    let ac3 = scratch.0.join("out.ac3");
    std::fs::write(&ac3, aus.iter().flat_map(|(d, _)| d.iter().copied()).collect::<Vec<u8>>()).unwrap();
    let raw = scratch.0.join("out.s16");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(&ac3)
        .args(["-f", "s16le", "-acodec", "pcm_s16le"])
        .arg(&raw)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "the AC-3 must decode");
    let pcm: Vec<i16> = std::fs::read(&raw).unwrap().chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    // AC-3 5.1(side) decodes in FFmpeg's native order: FL FR FC LFE SL SR, the order of TONES.
    for (slot, (name, hz)) in TONES.iter().enumerate() {
        let own = tone_level(&pcm, 6, slot, *hz);
        assert!(own > 500.0, "{name} (slot {slot}) lost its {hz} Hz tone ({own:.0})");
        for (other, ohz) in TONES.iter().filter(|(_, h)| h != hz) {
            let leak = tone_level(&pcm, 6, slot, *ohz);
            assert!(leak < own / 10.0, "{name} (slot {slot}) carries {other}'s {ohz} Hz tone ({leak:.0} vs {own:.0})");
        }
    }
}

/// **Stereo (Pro Logic II).** The 5.1 tones folded by the shim with swresample's DPLII matrix: the
/// centre lands equally and in phase in both channels, each surround in both channels in OPPOSITE
/// phase (the matrix's surround encoding), the fronts stay on their own side and the LFE is
/// dropped (swr's default `lfe_mix_level` 0).
#[test]
#[ignore = "needs the host build of the bundled FFmpeg, libsnadec and the ffmpeg CLI"]
fn demux_folds_5_1_to_pro_logic_ii_stereo() {
    let _g = sn_base::testlock::serial();
    if !tool_present("ffmpeg") {
        eprintln!("SKIP: the ffmpeg command line tool is not installed");
        return;
    }
    let _b = Bound::new();
    let scratch = Scratch::new("adec-dplii");
    let file = generate_tones_mkv(&scratch.0, "thd51.mkv", "truehd");
    let aus = demux_pcm(&file, 2 | adec::DPLII, "truehd");
    assert_continuous_pcm(&aus, 2);
    let pcm: Vec<i16> = aus.iter().flat_map(|(d, _)| d.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]))).collect();
    let lvl = |ch, hz| tone_level(&pcm, 2, ch, hz);
    let (fl, fr, fc, lfe, sl, sr) = (300, 500, 700, 100, 1100, 1300);
    assert!(lvl(0, fl) > 500.0 && lvl(1, fl) < lvl(0, fl) / 10.0, "FL left only ({:.0} / {:.0})", lvl(0, fl), lvl(1, fl));
    assert!(lvl(1, fr) > 500.0 && lvl(0, fr) < lvl(1, fr) / 10.0, "FR right only");
    assert!(lvl(0, fc) > 300.0 && (lvl(0, fc) / lvl(1, fc) - 1.0).abs() < 0.05, "FC equal in both ({:.0} / {:.0})", lvl(0, fc), lvl(1, fc));
    assert!(tone_correlation(&pcm, 2, 0, 1, fc) > 0.99, "FC in phase");
    for (name, hz) in [("SL", sl), ("SR", sr)] {
        assert!(lvl(0, hz) > 150.0 && lvl(1, hz) > 150.0, "{name} in both ({:.0} / {:.0})", lvl(0, hz), lvl(1, hz));
        let c = tone_correlation(&pcm, 2, 0, 1, hz);
        assert!(c < -0.99, "{name}: opposite phase in L and R (correlation {c:.3})");
    }
    assert!(lvl(0, sl) > lvl(1, sl) && lvl(1, sr) > lvl(0, sr), "each surround leans to its own side (0.87 vs 0.5)");
    assert!(lvl(0, lfe) < 50.0 && lvl(1, lfe) < 50.0, "the LFE is not mixed in");
}

/// **Stereo.** The 5.1 tones folded by the shim's plain swresample downmix (no matrix): the centre
/// lands equally and in phase in both channels, each surround only on its own side (no anti-phase
/// copy in the other), the fronts stay on their side and the LFE is dropped. FLAC, so the fold is
/// swresample's (TrueHD would hand over its own stereo substream).
#[test]
#[ignore = "needs the host build of the bundled FFmpeg, libsnadec and the ffmpeg CLI"]
fn demux_folds_5_1_to_plain_stereo() {
    let _g = sn_base::testlock::serial();
    if !tool_present("ffmpeg") {
        eprintln!("SKIP: the ffmpeg command line tool is not installed");
        return;
    }
    let _b = Bound::new();
    let scratch = Scratch::new("adec-stereo");
    let file = generate_tones_mkv(&scratch.0, "flac51.mkv", "flac");
    let aus = demux_pcm(&file, 2, "flac");
    assert_continuous_pcm(&aus, 2);
    let pcm: Vec<i16> = aus.iter().flat_map(|(d, _)| d.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]))).collect();
    let lvl = |ch, hz| tone_level(&pcm, 2, ch, hz);
    let (fl, fr, fc, lfe, sl, sr) = (300, 500, 700, 100, 1100, 1300);
    assert!(lvl(0, fl) > 500.0 && lvl(1, fl) < lvl(0, fl) / 10.0, "FL left only");
    assert!(lvl(1, fr) > 500.0 && lvl(0, fr) < lvl(1, fr) / 10.0, "FR right only");
    assert!(lvl(0, fc) > 300.0 && (lvl(0, fc) / lvl(1, fc) - 1.0).abs() < 0.05, "FC equal in both ({:.0} / {:.0})", lvl(0, fc), lvl(1, fc));
    assert!(tone_correlation(&pcm, 2, 0, 1, fc) > 0.99, "FC in phase");
    assert!(lvl(0, sl) > 300.0 && lvl(1, sl) < lvl(0, sl) / 10.0, "SL left only ({:.0} / {:.0})", lvl(0, sl), lvl(1, sl));
    assert!(lvl(1, sr) > 300.0 && lvl(0, sr) < lvl(1, sr) / 10.0, "SR right only ({:.0} / {:.0})", lvl(1, sr), lvl(0, sr));
    assert!(lvl(0, lfe) < 50.0 && lvl(1, lfe) < 50.0, "the LFE is not mixed in");
}
