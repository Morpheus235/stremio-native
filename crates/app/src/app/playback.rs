//! **Playback orchestration**: a Detail page's chosen stream started on `sn_media`'s buffer-feed
//! player, pumped once a frame, and read back as the [`PlaybackView`] the player page draws.
//!
//! The application owns the Player machine and the adapter that holds the main-thread token
//! (`sn_media::player::adapter`); this module is the only one that touches either. A start that
//! cannot reach the native pipeline (a stream whose headers would split a request, a codec the
//! television cannot decode) still pushes the player page, which then shows the failure read-out
//! rather than leaving the viewer on the list wondering whether the press landed.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::Mutex;

use sn_base::eventlog::log;
use sn_base::task::MainThread;
use sn_media::headers::RequestHeaders;
use sn_media::player::adapter::PlayerAdapter;
use sn_media::player::machine::Player;
use sn_media::player::{self, PlaybackState};
use sn_media::session::PlaybackStart;
use sn_platform::i18n::msg;
use sn_screens::registry::{PlayPhase, PlayRequest, PlaybackView};

pub struct Playback {
    pub player: Player,
    pub adapter: PlayerAdapter,
    pub view: PlaybackView,
    /// The viewer paused (and the sink accepted it).
    paused: bool,
    /// A start refused before the native pipeline (malformed headers): shown until the next one.
    refused: bool,
}

impl Playback {
    pub fn new(mt: MainThread) -> Self {
        Playback { player: Player::new(), adapter: PlayerAdapter::new(mt), view: PlaybackView::IDLE, paused: false, refused: false }
    }

    /// Start `req`, replacing whatever plays.
    pub fn start(&mut self, req: &PlayRequest) {
        self.stop();
        self.refused = false;
        let headers = match RequestHeaders::from_pairs(&req.headers) {
            Ok(h) => h,
            Err(why) => {
                log(&format!("play: refused the stream's request headers ({why:?})"));
                self.refused = true;
                self.refresh();
                return;
            }
        };
        let mut start = PlaybackStart::new(req.url.clone());
        start.headers = headers;
        match player::begin_playback(&mut self.player.session, &mut self.adapter, start) {
            Ok(outcome) => log(&format!("play: start {outcome:?}")),
            Err(refusal) => log(&format!("play: refused before the pipeline ({refusal:?})")),
        }
        self.refresh();
    }

    /// End the playback (idempotent).
    pub fn stop(&mut self) {
        if self.adapter.is_live() {
            player::stop_bufferfeed(&mut self.player.session, &mut self.adapter);
        }
        self.player.session.clear();
        self.paused = false;
        self.refused = false;
        self.refresh();
    }

    /// Pause a playing stream, resume a paused one.
    pub fn toggle_pause(&mut self) {
        if !self.adapter.is_live() {
            return;
        }
        if self.paused {
            if player::resume(&mut self.adapter) {
                self.paused = false;
            }
        } else if player::pause(&mut self.adapter) {
            self.paused = true;
        }
        self.refresh();
    }

    /// The frame's pump: advance the pipeline, re-read the view. Returns the video plane's edge
    /// (`Some(bound)` when it changed), which the loop forwards to the present gate.
    pub fn tick(&mut self, now_ms: u32) -> (bool, Option<bool>) {
        if self.adapter.is_live() {
            player::pump(&mut self.player.session, &mut self.adapter, now_ms);
        }
        let edge = player::observe_video_plane(&mut self.player, &mut self.adapter);
        (self.refresh(), edge)
    }

    /// Re-read the view; `true` when it changed.
    fn refresh(&mut self) -> bool {
        let next = self.read();
        let changed = next != self.view;
        self.view = next;
        changed
    }

    fn read(&self) -> PlaybackView {
        if self.refused {
            return PlaybackView { phase: PlayPhase::Failed, caption: msg::app_player_refused_c(), reason: c"" };
        }
        let ps = &self.player.session;
        let st = player::state(ps);
        if st == PlaybackState::Idle && ps.start().is_none() {
            return PlaybackView::IDLE;
        }
        view_of(st, self.paused, || {
            let shape = player::error_now(ps);
            (shape.caption, static_cstr(shape.readout))
        })
    }
}

/// The media layer's state as the player page reads it.
pub fn view_of(st: PlaybackState, paused: bool, error: impl FnOnce() -> (&'static CStr, &'static CStr)) -> PlaybackView {
    match st {
        PlaybackState::Error => {
            let (caption, reason) = error();
            PlaybackView { phase: PlayPhase::Failed, caption, reason }
        }
        PlaybackState::Playing if paused => PlaybackView { phase: PlayPhase::Paused, caption: c"", reason: c"" },
        PlaybackState::Playing => PlaybackView { phase: PlayPhase::Playing, caption: c"", reason: c"" },
        PlaybackState::Paused => PlaybackView { phase: PlayPhase::Paused, caption: c"", reason: c"" },
        PlaybackState::Idle | PlaybackState::Resolving | PlaybackState::Connecting | PlaybackState::Buffering | PlaybackState::Seeking => {
            PlaybackView { phase: PlayPhase::Working, caption: player::busy_caption(st), reason: c"" }
        }
    }
}

/// A `&'static CStr` for one of the media layer's fixed read-out strings. The vocabulary is
/// closed (`ErrorShape`'s readouts), so the interned set is bounded by it.
fn static_cstr(s: &'static str) -> &'static CStr {
    static SEEN: Mutex<Option<HashMap<&'static str, &'static CStr>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let map = seen.get_or_insert_with(HashMap::new);
    map.entry(s).or_insert_with(|| {
        let owned = CString::new(s.replace('\0', "")).unwrap_or_default();
        Box::leak(owned.into_boxed_c_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_media_state_maps_to_one_page_phase() {
        let err = || (c"Can't play", c"The server refused");
        assert_eq!(view_of(PlaybackState::Playing, false, err).phase, PlayPhase::Playing);
        assert_eq!(view_of(PlaybackState::Playing, true, err).phase, PlayPhase::Paused);
        assert_eq!(view_of(PlaybackState::Paused, false, err).phase, PlayPhase::Paused);
        for st in [PlaybackState::Connecting, PlaybackState::Buffering, PlaybackState::Seeking, PlaybackState::Resolving] {
            let v = view_of(st, false, err);
            assert_eq!(v.phase, PlayPhase::Working);
            assert!(!v.caption.is_empty(), "a working read-out says what it waits for ({st:?})");
        }
        let failed = view_of(PlaybackState::Error, false, err);
        assert_eq!((failed.phase, failed.caption, failed.reason), (PlayPhase::Failed, c"Can't play", c"The server refused"));
    }

    #[test]
    fn a_read_out_string_is_interned_once() {
        let a = static_cstr("Playback stopped");
        let b = static_cstr("Playback stopped");
        assert!(std::ptr::eq(a, b));
        assert_eq!(a.to_str().unwrap(), "Playback stopped");
    }

    #[test]
    fn malformed_stream_headers_fail_on_the_page_without_touching_the_pipeline() {
        let _g = sn_base::testlock::serial();
        // SAFETY: the test thread stands in for the main thread; nothing native is reached.
        let mut pb = Playback::new(unsafe { MainThread::assume() });
        pb.start(&PlayRequest {
            url: "https://cdn.example/v.m3u8".into(),
            headers: vec![("Referer".into(), "a\r\nInjected: 1".into())],
            title: "T".into(),
        });
        assert!(!pb.adapter.is_live(), "nothing reached the native pipeline");
        assert_eq!(pb.view.phase, PlayPhase::Failed);
        assert_eq!(pb.view.caption, msg::app_player_refused_c());
        pb.stop();
        assert_eq!(pb.view, PlaybackView::IDLE, "a stop clears the read-out");
    }
}
