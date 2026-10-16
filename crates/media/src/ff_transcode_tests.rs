//! The DTS -> AC-3 transcoder and the pre-Load probe, through the FFmpeg the app SHIPS.
//!
//! What these grade is a property of `ci/build-ffmpeg.sh`'s component list (the `dca` decoder,
//! the `dca` parser and the float `ac3` encoder) and of struct fields only the real libraries fill,
//! so — like `ff_image_subtitle_tests` — they load the host build of that same FFmpeg
//! (`HOST=1 ci/build-ffmpeg.sh`; on Linux `ci/stage-host-ffmpeg.sh` prints the `SN_FFMPEG_DIR` to
//! export) and are `#[ignore]`d everywhere else: an honest "not run" in the plain suite, never a
//! pass that executed nothing. The two that write a Matroska fixture also need the `ffmpeg` CLI
//! and skip, loudly, without it. Run them with
//! `SN_FFMPEG_DIR=<dir holding libav*-sn.*> CARGO_INCREMENTAL=0 cargo test -p sn_media --lib ff::transcode_tests -- --ignored`.

use super::image_subtitle_tests::{bind_host_ffmpeg, HostFfmpeg};
use super::late_audio_tests::{tool_present, FileServer, Scratch};
use super::transcode::{Ac3Frame, DtsToAc3, TranscodeError, AC3_FRAME_SAMPLES};
use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The fixture: one 5.1 DTS core frame, 48 kHz, 512 samples (NBLKS 15), 1884 bytes.
const DTS_FRAME: &[u8] = include_bytes!("../../../tests/fixtures/dts-core-frame.dts");
const DTS_FRAME_SAMPLES: i64 = 512;
/// 640 kb/s at 48 kHz: 640000 * 1536 / 48000 / 8.
const AC3_FRAME_BYTES: usize = 2560;
/// One AC-3 frame's duration, ns.
const AC3_FRAME_NS: i64 = 32_000_000;

/// The host FFmpeg bound and the software path detected, restored however the test leaves.
struct Bound {
    _ffmpeg: HostFfmpeg,
}

impl Bound {
    fn new() -> Bound {
        let ffmpeg = bind_host_ffmpeg();
        assert!(transcode::detect(), "the bundled libavcodec must carry the dca decoder and the ac3 encoder");
        Bound { _ffmpeg: ffmpeg }
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        // Leave the software path as undetected as the plain suite expects.
        ABI_OK.store(false, Ordering::Relaxed);
        transcode::detect();
    }
}

/// `(acmod, lfeon)` of an AC-3 frame's bit stream information (ATSC A/52 §5.4.2).
fn ac3_acmod_lfe(frame: &[u8]) -> (u8, bool) {
    let acmod = frame[6] >> 5;
    let mut bit = 3usize; // bits of byte 6 consumed
    if acmod & 1 != 0 && acmod != 1 {
        bit += 2; // cmixlev
    }
    if acmod & 4 != 0 {
        bit += 2; // surmixlev
    }
    if acmod == 2 {
        bit += 2; // dsurmod
    }
    let byte = frame[6 + bit / 8];
    (acmod, (byte >> (7 - bit % 8)) & 1 == 1)
}

/// Every AC-3 frame starts with the sync word, is one 640 kb/s frame long, and carries 3/2 + LFE.
fn assert_ac3_5_1(frames: &[Ac3Frame]) {
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(&f.data[..2], &[0x0b, 0x77], "frame {i}: AC-3 sync word");
        assert_eq!(f.data.len(), AC3_FRAME_BYTES, "frame {i}: one 640 kb/s frame at 48 kHz");
        assert_eq!(f.data[4] >> 6, 0, "frame {i}: fscod 0 = 48 kHz");
        assert_eq!(ac3_acmod_lfe(&f.data), (7, true), "frame {i}: acmod 3/2 with LFE = 5.1");
    }
}

#[test]
#[ignore = "requires the bundled host FFmpeg; run make check-ffmpeg"]
fn the_bundled_build_carries_the_dts_to_ac3_path() {
    let _g = sn_base::testlock::serial();
    let _b = Bound::new();
    assert!(transcode::available());
    assert_eq!(crate::stream::SoftwareAudio::detected(), crate::stream::SoftwareAudio { dts_to_ac3: true });
}

/// **The transcoder's contract.** Ten DTS core frames (5120 samples of 5.1) in, AC-3 out: every
/// frame sync-worded, 640 kb/s, 5.1; the pts strictly monotonic, exactly one AC-3 frame (32 ms)
/// apart, and the first one the first DTS packet's pts less the encoder's 256-sample delay.
#[test]
#[ignore = "requires the bundled host FFmpeg; run make check-ffmpeg"]
fn a_dts_core_track_becomes_640k_5_1_ac3_with_monotonic_pts() {
    let _g = sn_base::testlock::serial();
    let _b = Bound::new();
    let mut t = unsafe { DtsToAc3::open(std::ptr::null()) }.expect("open the transcoder");
    let start_ns = 10_000_000_000i64;
    let mut out = Vec::new();
    for i in 0..10 {
        // Millisecond-rounded, as Matroska stamps them: the count must win over the rounding.
        let pts = start_ns + (i * DTS_FRAME_SAMPLES * 1000 / 48) / 1_000_000 * 1_000_000;
        out.extend(t.push(DTS_FRAME, pts).expect("push"));
    }
    assert_eq!(out.len(), 3, "5120 samples complete three 1536-sample frames");
    assert_eq!((t.channels(), t.sample_rate()), (6, 48_000));
    out.extend(t.finish().expect("finish"));
    assert_eq!(out.len(), 4, "the last 512 samples go out padded");
    assert_ac3_5_1(&out);
    assert_eq!(out[0].pts_ns, start_ns - 5_333_333, "pts = first DTS pts - 256 samples of encoder delay");
    for w in out.windows(2) {
        assert!(w[1].pts_ns > w[0].pts_ns, "monotonic: {} then {}", w[0].pts_ns, w[1].pts_ns);
        assert!((w[1].pts_ns - w[0].pts_ns - AC3_FRAME_NS).abs() <= 1, "one AC-3 frame apart");
    }
    assert_eq!(AC3_FRAME_SAMPLES as i64 * 1_000_000_000 / 48_000, AC3_FRAME_NS);
}

/// A seek (`reset`) or a jump in the container's timeline starts the clock over at the new pts,
/// and the AC-3 pts follow it.
#[test]
#[ignore = "requires the bundled host FFmpeg; run make check-ffmpeg"]
fn a_seek_or_a_timeline_jump_reanchors_the_ac3_pts() {
    let _g = sn_base::testlock::serial();
    let _b = Bound::new();
    let mut t = unsafe { DtsToAc3::open(std::ptr::null()) }.expect("open the transcoder");
    let frame_ns = DTS_FRAME_SAMPLES * 1_000_000_000 / 48_000;
    let mut first = Vec::new();
    for i in 0..6 {
        first.extend(t.push(DTS_FRAME, i * frame_ns).unwrap());
    }
    assert_eq!(first.len(), 2);
    t.reset();
    let mut after = Vec::new();
    for i in 0..6 {
        after.extend(t.push(DTS_FRAME, 600_000_000_000 + i * frame_ns).unwrap());
    }
    assert_eq!(after.len(), 2, "the encoder reopened after the seek and the FIFO started empty");
    assert_ac3_5_1(&after);
    assert_eq!(after[0].pts_ns, 600_000_000_000 - 5_333_333);
    // A 30 s jump inside one run (a gap in the file) re-anchors without a reset.
    let mut jumped = Vec::new();
    for i in 0..3 {
        jumped.extend(t.push(DTS_FRAME, 630_000_000_000 + i * frame_ns).unwrap());
    }
    assert!(jumped.iter().all(|f| f.pts_ns >= 630_000_000_000 - 5_333_333), "{jumped:?}");
}

/// A damaged packet is skipped (the stream plays on); a stream of them is an explicit failure,
/// not a lane that never fills.
#[test]
#[ignore = "requires the bundled host FFmpeg; run make check-ffmpeg"]
fn undecodable_packets_are_skipped_then_refused() {
    let _g = sn_base::testlock::serial();
    let _b = Bound::new();
    let mut t = unsafe { DtsToAc3::open(std::ptr::null()) }.expect("open the transcoder");
    let junk = [0x5au8; 400];
    assert_eq!(t.push(&junk, 0), Ok(vec![]), "one bad packet is skipped");
    assert!(t.push(DTS_FRAME, 0).is_ok(), "and good ones decode after it");
    let mut failed = None;
    for i in 0..64 {
        if let Err(e) = t.push(&junk, i) {
            failed = Some(e);
            break;
        }
    }
    assert!(matches!(failed, Some(TranscodeError::Decode(_))), "{failed:?}");
}

/// Write a 2 s Matroska: 640x360 H.264 at 25 fps and 5.1 DTS (the system CLI's `dca` encoder).
fn generate_dts_mkv(dir: &Path) -> PathBuf {
    let out = dir.join("dts51.mkv");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=s=640x360:r=25:d=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
        .args(["-map", "0:v", "-map", "1:a"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-g", "25"])
        .args(["-c:a", "dca", "-strict", "-2", "-ac", "6", "-b:a", "1509k", "-disposition:a:0", "default"])
        .arg(&out)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg could not write the DTS fixture");
    out
}

/// Tools present, host FFmpeg bound, the DTS fixture written and served.
struct Served {
    server: FileServer,
    _scratch: Scratch,
    _bound: Bound,
}

fn serve_dts_fixture(tag: &str) -> Option<Served> {
    if !tool_present("ffmpeg") {
        eprintln!("SKIP {tag}: the ffmpeg command line tool is not installed");
        return None;
    }
    let bound = Bound::new();
    let scratch = Scratch::new(tag);
    let file = generate_dts_mkv(&scratch.0);
    let server = FileServer::start(&file);
    Some(Served { server, _scratch: scratch, _bound: bound })
}

/// **Task A end to end: probe before Load.** The probe opens the stream over HTTP with the
/// bundled demuxer and reads what the Load must declare; the declaration derived from it is the
/// file's, with no caller-supplied facts at all.
#[test]
#[ignore = "needs the host build of the bundled FFmpeg and the ffmpeg CLI"]
fn the_probe_reads_the_declaration_off_the_container() {
    let _g = sn_base::testlock::serial();
    let Some(s) = serve_dts_fixture("probe") else { return };
    let url = format!("http://127.0.0.1:{}/media/dts51.mkv", s.server.port);
    let io = probe::ProbeIo::new();
    let probed = probe::probe(&url, &crate::headers::RequestHeaders::none(), &io).expect("probe");
    let v = probed.video.clone().expect("a video stream");
    assert_eq!((v.codec.as_str(), v.width, v.height), ("h264", 640, 360));
    assert!((v.fps - 25.0).abs() < 1e-6, "fps {}", v.fps);
    assert_eq!(probed.audio.len(), 1);
    let a = &probed.audio[0];
    assert_eq!((a.codec.as_str(), a.channels, a.sample_rate), ("dts", 6, 48_000));
    assert!(a.default, "the fixture flags its audio track default (AVStream.disposition at OFF_STREAM_DISPOSITION)");
    assert!(probed.duration_ns > 1_900_000_000, "duration {}", probed.duration_ns);

    let mut caps = sn_platform::devcaps::Caps::assumed();
    caps.audio = "aac,ac3,eac3".into(); // no DTS row: the transcode case
    let sw = crate::stream::SoftwareAudio::detected();
    let (decl, track) = crate::stream::derive_declaration(&probed, None, |a| {
        crate::stream::AudioFeed::decide(&a.codec, a.channels, &caps, sw)
    })
    .unwrap();
    assert_eq!(track, Some(0));
    assert_eq!((decl.vcodec.as_str(), decl.acodec.as_str(), decl.audio_channels, decl.raster), ("h264", "dts", 6, (640, 360)));
    assert_eq!(
        crate::stream::AudioFeed::decide(&decl.acodec, decl.audio_channels, &caps, sw),
        crate::stream::AudioFeed::TranscodeAc3 { source: "dts".into(), channels: 6 }
    );

    // A stream that will not open is an explicit failure, not a hang (port 1: refused).
    let io = probe::ProbeIo::new();
    assert_eq!(
        probe::probe("http://127.0.0.1:1/media/dts51.mkv", &crate::headers::RequestHeaders::none(), &io),
        Err(crate::stream::ProbeFailure::Open)
    );
}

/// **Task B end to end.** `ff::demux` over a DTS Matroska with the transcode on: the audio lane
/// carries AC-3 — every AU a 640 kb/s 5.1 frame, pts strictly monotonic and 32 ms apart — for the
/// whole track, its last partial frame included, and the video lane carries the video untouched.
#[test]
#[ignore = "needs the host build of the bundled FFmpeg and the ffmpeg CLI"]
fn demux_feeds_a_dts_track_as_ac3_when_the_session_says_transcode() {
    let _g = sn_base::testlock::serial();
    let Some(s) = serve_dts_fixture("demux") else { return };
    SHARED.seek_to_ns.store(-1, Ordering::Relaxed);
    SHARED.desired_audio_idx.store(-1, Ordering::Relaxed);
    SHARED.dg_load_v.store(0, Ordering::Relaxed);
    SHARED.side_subs_owner.store(false, Ordering::Relaxed);
    SHARED.demux_failed.store(false, Ordering::Relaxed);
    SHARED.demux_io_failed.store(false, Ordering::Relaxed);
    FEED_AUDIO.store(true, Ordering::Relaxed);
    set_transcode_dts(true);

    let (video_cap, audio_cap) = crate::player::engine::aq_caps();
    let mut video = crate::aq::aq_new(video_cap as c_long);
    let mut audio = crate::aq::aq_new(audio_cap as c_long);
    let mut hs = sn_net::stream::http_stream_boxed();
    let (vp, ap, hp) = (
        SendPtr(&mut *video as *mut AuQueue),
        SendPtr(&mut *audio as *mut AuQueue),
        SendPtr(&mut *hs as *mut HttpStream),
    );
    let url = format!("http://127.0.0.1:{}/media/dts51.mkv", s.server.port);
    let th = std::thread::spawn(move || {
        let off = sn_base::task::OffFrame::for_test();
        demux(&off, url, crate::headers::RequestHeaders::none(), "dts".into(), vp, ap, hp)
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
    set_transcode_dts(false);

    assert!(!SHARED.demux_failed.load(Ordering::Acquire), "the transcoded demux must not fail");
    assert!(vids.len() >= 45, "the video lane carries the video ({} AUs)", vids.len());
    // 2 s of 48 kHz is 62.5 AC-3 frames: 62 full plus the padded tail.
    assert!((60..=64).contains(&aus.len()), "{} AC-3 AUs", aus.len());
    let frames: Vec<Ac3Frame> = aus.iter().map(|(d, p)| Ac3Frame { data: d.clone(), pts_ns: *p }).collect();
    assert_ac3_5_1(&frames);
    for w in frames.windows(2) {
        assert!(w[1].pts_ns > w[0].pts_ns, "monotonic");
        assert!((w[1].pts_ns - w[0].pts_ns - AC3_FRAME_NS).abs() <= 1, "{} -> {}", w[0].pts_ns, w[1].pts_ns);
    }
    assert!(frames[0].pts_ns.abs() < 20_000_000, "the first AC-3 frame sits at the track start ({} ns)", frames[0].pts_ns);
}

/// **The whole start path, Stremio-shaped.** `begin_playback` with a bare URL (no declaration):
/// the start is probed, reads `Connecting`, and `pump` — called as the application calls it, once a
/// frame — resolves it from the container and launches the native start with the Load declaring
/// AC-3 for the DTS track a table without a DTS row cannot take. The simulator's sink stands in for
/// Starfish (`hostsim`).
#[cfg(feature = "hostsim")]
#[test]
#[ignore = "needs the host build of the bundled FFmpeg and the ffmpeg CLI"]
fn begin_playback_probes_an_undeclared_start_and_loads_its_dts_as_ac3() {
    use crate::player::PlaybackState;
    let _g = sn_base::testlock::serial();
    let Some(s) = serve_dts_fixture("e2e") else { return };
    let url = format!("http://127.0.0.1:{}/media/dts51.mkv", s.server.port);
    let mut pa = crate::player::adapter::PlayerAdapter::new(unsafe { sn_base::task::MainThread::assume() });
    let mut ps = crate::session::PlaybackSession::IDLE;
    let outcome = crate::player::begin_playback(&mut ps, &mut pa, crate::session::PlaybackStart::new(url))
        .expect("an undeclared start is not refused up front");
    assert_eq!(outcome, crate::player::BufferfeedStartOutcome::Probing);
    assert_eq!(crate::player::state(&ps), PlaybackState::Connecting);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut now = 0u32;
    while !pa.is_live() {
        assert!(ps.refusal().is_none(), "refused: {:?}", ps.refusal());
        assert!(Instant::now() < deadline, "the probe never resolved the start");
        crate::player::pump(&mut ps, &mut pa, now);
        now += 16;
        std::thread::sleep(Duration::from_millis(16));
    }
    assert_eq!(ps.audio(), &crate::stream::AudioFeed::TranscodeAc3 { source: "dts".into(), channels: 6 });
    let d = ps.stream();
    assert_eq!((d.vcodec.as_str(), d.acodec.as_str(), d.raster), ("h264", "dts", (640, 360)));
    assert_eq!(SHARED.dg_load_a.load(Ordering::Relaxed), 5, "the Load declared AC3 (from DTS)");
    assert_eq!(ps.probed().map(|p| p.audio.len()), Some(1));
    crate::player::stop_bufferfeed(&mut ps, &mut pa);
    set_transcode_dts(false);
}
