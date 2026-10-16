//! **The seam between the media layer and whoever decides what to play.**
//!
//! Everything a playback needs from above arrives here as plain data, at start, in one
//! [`PlaybackStart`]: the stream URL and the request headers its addon names, where to start, which
//! audio and subtitle track, and what the stream is ([`StreamDeclaration`], which the caller
//! probes). The device's own capabilities are read from `sn_platform` (`devcaps`, `tv::sandbox`),
//! never passed. Everything the media layer reports back leaves through one
//! [`PlaybackObserver`] the start carries — nothing here knows what an addon, a library or a
//! watch-history is.
//!
//! [`PlaybackSession`] is the main-thread value the player machine owns
//! (`player::machine::Player::session`) and every engine entry point takes: the start it is
//! playing, the two decisions frozen from it when it began (Dolby Vision presentation and the
//! audio feed, so a reload or a seek never re-reads a capability that answered later), and the
//! refusals the player's `Error` state is derived from when no Engine exists to publish one.

use crate::headers::RequestHeaders;
use crate::stream::{AudioFeed, DvPresentation, StreamDeclaration};
use std::sync::Arc;

/// One playback, as the caller asks for it.
#[derive(Clone)]
pub struct PlaybackStart {
    /// The stream: `http` or `https`. Carries whatever query its addon put there; never logged.
    pub url: String,
    /// Sent on every request the media transport makes for this stream — the first open, every
    /// Range seek, every redirect hop (credential headers only to the stream's own origin).
    pub headers: RequestHeaders,
    /// Where to start, in nanoseconds of content time (a resume). 0 = the beginning.
    pub start_ns: i64,
    /// The audio stream to feed, as its 0-based position among the file's AUDIO streams. `None`:
    /// the first audio stream of the declared codec.
    pub audio_track: Option<i32>,
    /// The subtitle stream to show, as its 0-based position among the file's SUBTITLE streams.
    /// `None`: subtitles off.
    pub subtitle_track: Option<i32>,
    /// What the stream is, as the Load must declare it.
    pub stream: StreamDeclaration,
    /// A background preview (the detail page's trailer), not a playback: no observer reports and
    /// the preview admission budget (`player::preview`) applies.
    pub preview: bool,
    /// Who hears about this playback's progress and its end. `None`: nobody.
    pub observer: Option<Arc<dyn PlaybackObserver>>,
}

impl PlaybackStart {
    /// A start for `url` from the beginning, with no headers, no track choice and an empty
    /// declaration: the shape the dev triggers and the tests build on.
    pub fn new(url: impl Into<String>) -> PlaybackStart {
        PlaybackStart {
            url: url.into(),
            headers: RequestHeaders::none(),
            start_ns: 0,
            audio_track: None,
            subtitle_track: None,
            stream: StreamDeclaration::default(),
            preview: false,
            observer: None,
        }
    }
}

impl std::fmt::Debug for PlaybackStart {
    /// Never the URL (its query may carry an addon's credential) and never header values.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackStart")
            .field("start_ns", &self.start_ns)
            .field("audio_track", &self.audio_track)
            .field("subtitle_track", &self.subtitle_track)
            .field("stream", &self.stream)
            .field("headers", &self.headers)
            .field("preview", &self.preview)
            .field("observer", &self.observer.is_some())
            .finish()
    }
}

/// The playback position as reported to a [`PlaybackObserver`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Content time, milliseconds.
    pub position_ms: i64,
    /// The stream's duration, milliseconds; 0 when the container does not say (live).
    pub duration_ms: i64,
    pub paused: bool,
}

/// Why a playback stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The viewer (or the app) stopped it.
    Stopped,
    /// It played to the end of the stream.
    Ended,
    /// It failed; `player::error_now` says why.
    Failed,
}

/// **What the media layer reports upward**, one value per playback. Every method has an empty
/// default, so an observer implements only what it records. Called from the progress worker
/// (`progress`) or the main thread (the rest); an implementation must not block — a slow one is
/// handed the facts and does its own I/O on its own thread.
pub trait PlaybackObserver: Send + Sync {
    /// The playback's first frame is on the panel, at `position_ms`.
    fn started(&self, _position_ms: i64) {}
    /// Roughly every 10 s while the playback runs (paused or not).
    fn progress(&self, _at: Progress) {}
    /// The playback ended, once. `at` is the last position presented.
    fn stopped(&self, _at: Progress, _reason: StopReason) {}
}

/// Why a start was refused before any native object was built — each is an explicit `Error`
/// state (`player::state`), never a Load that cannot advance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartRefusal {
    /// The television cannot decode the stream's video codec, or its table lists no such decoder.
    VideoUnsupported(String),
    /// Dolby Vision whose base layer alone is a wrong picture, on a set that cannot be told.
    DolbyVision(&'static str),
    /// The selected audio track needs a software decoder ([`AudioFeed::NeedsSoftwareDecode`]).
    AudioUnsupported { codec: String, channels: i64 },
}

/// The main-thread playback state (see the module doc).
pub struct PlaybackSession {
    start: Option<PlaybackStart>,
    dv: DvPresentation,
    audio: AudioFeed,
    refusal: Option<StartRefusal>,
    /// This playback was refused by the cached device sandbox preflight. No Engine exists.
    pub jail_load_blocked: bool,
    /// The last fresh native start was refused because a timed-out Load is still inside `sf_load`
    /// (an `AbandonedLoad` owns the one native object). `player::state` derives `Error` from it.
    pub native_start_refused: bool,
    /// Read-only publication of the sandbox repair state for the HUD.
    pub repair_status: sn_platform::tv::sandbox::State,
    /// The observer has been told this playback started (first frame), so it is told once.
    started_reported: bool,
}

impl PlaybackSession {
    /// Nothing playing.
    pub const IDLE: PlaybackSession = PlaybackSession {
        start: None,
        dv: DvPresentation::NotDv,
        audio: AudioFeed::Silent,
        refusal: None,
        jail_load_blocked: false,
        native_start_refused: false,
        repair_status: sn_platform::tv::sandbox::State::Idle,
        started_reported: false,
    };

    /// Install `start` as this session's playback and freeze its two decisions from the
    /// television's current capability answers. `Err` is a start that must not reach the native
    /// pipeline; the refusal stays on the session (`refusal`) for the `Error` read-out until the
    /// next `begin` or [`clear`](Self::clear).
    pub fn begin(&mut self, start: PlaybackStart) -> Result<(), StartRefusal> {
        let caps = sn_platform::devcaps::caps();
        self.decide(start, caps, |d, hevc| d.presentation_now(hevc))
    }

    /// [`begin`](Self::begin) against an explicit codec table, for tests of the layers that read
    /// the session (no Dolby Vision on the stream).
    #[cfg(test)]
    pub(crate) fn begin_against(&mut self, start: PlaybackStart, caps: &sn_platform::devcaps::Caps) -> Result<(), StartRefusal> {
        self.decide(start, caps, |_, _| DvPresentation::NotDv)
    }

    /// [`begin`](Self::begin) against an explicit table and Dolby Vision answer, for tests.
    fn decide(
        &mut self,
        start: PlaybackStart,
        caps: &sn_platform::devcaps::Caps,
        dv_of: impl Fn(&crate::stream::Dovi, bool) -> DvPresentation,
    ) -> Result<(), StartRefusal> {
        let decl = &start.stream;
        let hevc = matches!(decl.vcodec.to_ascii_lowercase().as_str(), "hevc" | "h265");
        let dv = dv_of(&decl.dovi, hevc);
        let audio = AudioFeed::decide(&decl.acodec, decl.audio_channels, caps);
        let refusal = video_refusal(&decl.vcodec, caps)
            .or_else(|| dv.refusal().map(StartRefusal::DolbyVision))
            .or_else(|| match &audio {
                AudioFeed::NeedsSoftwareDecode { codec, channels } => {
                    Some(StartRefusal::AudioUnsupported { codec: codec.clone(), channels: *channels })
                }
                _ => None,
            });
        self.start = Some(start);
        self.dv = dv;
        self.audio = audio;
        self.refusal = refusal.clone();
        self.jail_load_blocked = false;
        self.native_start_refused = false;
        self.started_reported = false;
        match refusal {
            Some(r) => {
                sn_base::eventlog::log(&format!("session: start refused — {r:?}"));
                Err(r)
            }
            None => Ok(()),
        }
    }

    /// Switch the fed audio track (a viewer pick) and re-decide its feed. `Err` leaves the session
    /// on the track it had.
    pub fn select_audio(&mut self, track: i32, codec: &str, channels: i64, atmos: bool) -> Result<(), StartRefusal> {
        let feed = AudioFeed::decide(codec, channels, sn_platform::devcaps::caps());
        if let AudioFeed::NeedsSoftwareDecode { codec, channels } = &feed {
            return Err(StartRefusal::AudioUnsupported { codec: codec.clone(), channels: *channels });
        }
        if let Some(s) = self.start.as_mut() {
            s.audio_track = Some(track);
            s.stream.acodec = codec.to_ascii_lowercase();
            s.stream.audio_channels = channels;
            s.stream.atmos = atmos;
        }
        self.audio = feed;
        Ok(())
    }

    /// Forget the playback (a real stop). Device facts (`repair_status`) stay.
    pub fn clear(&mut self) {
        let repair = self.repair_status;
        *self = PlaybackSession::IDLE;
        self.repair_status = repair;
    }

    /// The playback being played, if any.
    pub fn start(&self) -> Option<&PlaybackStart> {
        self.start.as_ref()
    }

    /// The stream URL; empty when nothing is installed.
    pub fn url(&self) -> &str {
        self.start.as_ref().map_or("", |s| s.url.as_str())
    }

    /// The stream's request headers.
    pub fn headers(&self) -> RequestHeaders {
        self.start.as_ref().map_or_else(RequestHeaders::none, |s| s.headers.clone())
    }

    /// The installed declaration (empty when nothing is installed: the sample feeds' baseline).
    pub fn stream(&self) -> StreamDeclaration {
        self.start.as_ref().map_or_else(StreamDeclaration::default, |s| s.stream.clone())
    }

    /// The Dolby Vision presentation frozen at [`begin`](Self::begin).
    pub fn dv(&self) -> DvPresentation {
        self.dv
    }

    /// The audio feed frozen at [`begin`](Self::begin) (or the last [`select_audio`](Self::select_audio)).
    pub fn audio(&self) -> &AudioFeed {
        &self.audio
    }

    /// Why the installed start was refused, if it was.
    pub fn refusal(&self) -> Option<&StartRefusal> {
        self.refusal.as_ref()
    }

    pub fn is_preview(&self) -> bool {
        self.start.as_ref().is_some_and(|s| s.preview)
    }

    /// The chosen audio stream (position among the file's audio streams), `-1` for the default.
    pub fn audio_track(&self) -> i32 {
        self.start.as_ref().and_then(|s| s.audio_track).unwrap_or(-1)
    }

    /// The chosen subtitle stream (position among the file's subtitle streams), `-1` for off.
    pub fn subtitle_track(&self) -> i32 {
        self.start.as_ref().and_then(|s| s.subtitle_track).unwrap_or(-1)
    }

    /// Where the installed start begins, in nanoseconds.
    pub fn start_ns(&self) -> i64 {
        self.start.as_ref().map_or(0, |s| s.start_ns.max(0))
    }

    /// The observer, unless this is a preview (which reports nothing).
    pub fn observer(&self) -> Option<Arc<dyn PlaybackObserver>> {
        self.start.as_ref().filter(|s| !s.preview).and_then(|s| s.observer.clone())
    }

    /// Tell the observer the first frame is up, once per [`begin`](Self::begin).
    pub fn report_started(&mut self, position_ns: i64) {
        if std::mem::replace(&mut self.started_reported, true) {
            return;
        }
        if let Some(o) = self.observer() {
            o.started(position_ns.max(0) / 1_000_000);
        }
    }
}

/// A video codec the pipeline cannot be fed, or that this set's table lists no decoder for.
fn video_refusal(vcodec: &str, caps: &sn_platform::devcaps::Caps) -> Option<StartRefusal> {
    let refused = match vcodec.to_ascii_lowercase().as_str() {
        "" | "h264" => false,
        "hevc" | "h265" => !caps.hevc,
        "av1" => !caps.av1,
        "vp9" => !caps.vp9,
        _ => true,
    };
    refused.then(|| StartRefusal::VideoUnsupported(vcodec.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{Dovi, DvPresentation};
    use sn_platform::devcaps::Caps;

    fn start(vcodec: &str, acodec: &str, channels: i64) -> PlaybackStart {
        let mut s = PlaybackStart::new("https://cdn.example/v.mkv");
        s.stream.vcodec = vcodec.into();
        s.stream.acodec = acodec.into();
        s.stream.audio_channels = channels;
        s
    }

    fn no_dv(_: &Dovi, _: bool) -> DvPresentation {
        DvPresentation::NotDv
    }

    #[test]
    fn a_dts_track_past_the_tables_ceiling_is_an_explicit_refusal_not_a_start() {
        let mut caps = Caps::assumed();
        caps.audio = "aac,ac3,eac3,dts".into();
        caps.audio_channels.insert("dts".into(), 6);
        let mut ps = PlaybackSession::IDLE;
        assert_eq!(ps.decide(start("hevc", "dts", 6), &caps, no_dv), Ok(()));
        assert_eq!(ps.audio(), &AudioFeed::DtsCore);
        let refused = ps.decide(start("hevc", "dts", 8), &caps, no_dv);
        assert_eq!(refused, Err(StartRefusal::AudioUnsupported { codec: "dts".into(), channels: 8 }));
        assert_eq!(ps.refusal(), Some(&StartRefusal::AudioUnsupported { codec: "dts".into(), channels: 8 }));
        ps.clear();
        assert_eq!(ps.refusal(), None, "a stop retires the refusal");
    }

    #[test]
    fn video_and_dolby_vision_refusals_come_before_audio() {
        let caps = Caps::assumed();
        let mut ps = PlaybackSession::IDLE;
        assert_eq!(
            ps.decide(start("vp9", "truehd", 8), &caps, no_dv),
            Err(StartRefusal::VideoUnsupported("vp9".into())),
            "the assumed table lists no VP9 decoder"
        );
        assert_eq!(
            ps.decide(start("hevc", "eac3", 6), &caps, |_, _| DvPresentation::Refuse("dual-layer")),
            Err(StartRefusal::DolbyVision("dual-layer"))
        );
        assert_eq!(ps.decide(start("h264", "aac", 2), &caps, no_dv), Ok(()));
        assert_eq!(ps.audio().payload(), Some(("AAC", 3)));
    }

    #[test]
    fn a_preview_reports_to_nobody_and_debug_prints_no_url() {
        struct Count(std::sync::atomic::AtomicU32);
        impl PlaybackObserver for Count {
            fn started(&self, _: i64) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let seen = Arc::new(Count(std::sync::atomic::AtomicU32::new(0)));
        let mut s = start("h264", "aac", 2);
        s.url = "https://cdn.example/v.mkv?token=secret".into();
        s.observer = Some(seen.clone());
        assert!(!format!("{s:?}").contains("secret"));
        let mut ps = PlaybackSession::IDLE;
        ps.decide(s.clone(), &Caps::assumed(), no_dv).unwrap();
        ps.report_started(5_000_000_000);
        ps.report_started(6_000_000_000);
        assert_eq!(seen.0.load(std::sync::atomic::Ordering::Relaxed), 1, "once per start");
        s.preview = true;
        ps.decide(s, &Caps::assumed(), no_dv).unwrap();
        ps.report_started(0);
        assert_eq!(seen.0.load(std::sync::atomic::Ordering::Relaxed), 1, "a preview reports nothing");
    }
}
