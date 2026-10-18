//! **Playback orchestration**: a Detail page's chosen stream started on `sn_media`'s buffer-feed
//! player, pumped once a frame, driven by the player page's [`PlayerCmd`]s (pause, seek, the
//! audio and subtitle tracks) and read back as the [`PlaybackView`] the page draws: the phase, the
//! clock and the media facts (the probed tracks, the ones in use, the video).
//!
//! The application owns the Player machine and the adapter that holds the main-thread token
//! (`sn_media::player::adapter`); this module is the only one that touches either. A start that
//! cannot reach the native pipeline (a stream whose headers would split a request, a codec the
//! television cannot decode) still pushes the player page, which then shows the failure read-out
//! rather than leaving the viewer on the list wondering whether the press landed.
//!
//! **Where the viewer is** goes to the account's library (stremio-core's `TimeChanged`,
//! [`Progress`]): every [`PROGRESS_EVERY_MS`] while it plays, when it pauses, and once more when it
//! stops (the last write), for a playback that knows what it is of ([`PlayItem`]). The bridge
//! sends them ([`Playback::take_progress`]); signed out nothing is written.
//!
//! **Skip markers** (`sn_stremio::skip`): once a playback has shown its first picture and knows
//! its duration, ONE lookup of its intro/recap/credits markers is handed to the bridge
//! ([`Playback::take_skip_lookup`]), which runs it on a worker; what lands
//! ([`Playback::land_skips`]) is published as `PlaybackView::skips`. A failure, a live stream or
//! an id no service knows is simply no markers.
//!
//! **Addon subtitles** (`super::addon_subs`): the bridge asks every subtitles addon when the
//! playback starts; what lands ([`Playback::land_subtitles`]) is listed after the file's tracks.
//! Picking one fetches it into the sidecar renderer (`sn_media::player::sidecar`, the pipeline the
//! file's text tracks use, so the size, position and delay apply) and turns the file's track off;
//! picking a file track or Off turns the sidecar off. The viewer's languages may pick one late,
//! when its answer lands, unless the viewer has chosen in the menu.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::Mutex;

use sn_base::eventlog::log;
use sn_base::task::MainThread;
use sn_media::headers::RequestHeaders;
use sn_media::player::adapter::PlayerAdapter;
use sn_media::player::machine::Player;
use sn_media::player::{self, sidecar, PlaybackState};
use sn_media::session::{PlaybackSession, PlaybackStart};
use sn_platform::i18n::msg;
use sn_screens::registry::{ChapterFacts, MediaFacts, PlayClock, PlayItem, PlayPhase, PlayRequest, PlaybackView, PlayerCmd, TrackFacts, VideoFacts};
use sn_stremio::library::Progress;
use sn_stremio::skip::{Lookup, Skips};

/// How often a playing stream's position is recorded.
pub const PROGRESS_EVERY_MS: u32 = 30_000;
/// A jump of the position larger than this between two frames is a seek, not watching.
const WATCH_STEP_MAX_MS: u64 = 5_000;

pub struct Playback {
    pub player: Player,
    pub adapter: PlayerAdapter,
    pub view: PlaybackView,
    /// The viewer paused (and the sink accepted it).
    paused: bool,
    /// A start refused before the native pipeline (malformed headers): shown until the next one.
    refused: bool,
    /// What plays, for its progress; `None` records nothing.
    item: Option<PlayItem>,
    progress: ProgressClock,
    /// Progress writes for the bridge to send: `(where, last)`.
    pending: Vec<(Progress, bool)>,
    /// The addon that offered what plays (Stats for nerds' Source row).
    addon: String,
    /// This playback's skip markers, its lookup's generation, and whether it was asked yet.
    skips: Skips,
    /// The markers the file's chapter titles name ([`sn_stremio::skip::from_chapters`]).
    chapter_skips: Skips,
    skip_gen: u32,
    skip_asked: bool,
    /// The language preferences this playback started with (the late addon-subtitle pick).
    tracks: sn_base::tracks::TrackPrefs,
    /// What subtitles addons offer for what plays.
    pub subs: super::addon_subs::AddonSubs,
    /// The addon subtitle shown, by its URL (never logged); `None` while the file's track or none is.
    sidecar_url: Option<String>,
    /// What [`PlayerCmd::ToggleSubtitles`] turned off: the pick that brings it back.
    last_sub: Option<PlayerCmd>,
}

/// One playback's marker lookup, for a worker: `gen` names the playback it is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipLookup {
    pub gen: u32,
    pub lookup: Lookup,
    pub dur_ms: u64,
}

impl Playback {
    pub fn new(mt: MainThread) -> Self {
        sn_screens::player::install_subtitle_painter(super::subtitles::paint);
        Playback {
            player: Player::new(),
            adapter: PlayerAdapter::new(mt),
            view: PlaybackView::IDLE,
            paused: false,
            refused: false,
            item: None,
            progress: ProgressClock::default(),
            pending: Vec::new(),
            addon: String::new(),
            skips: Skips::NONE,
            chapter_skips: Skips::NONE,
            skip_gen: 0,
            skip_asked: false,
            tracks: sn_base::tracks::TrackPrefs::default(),
            subs: super::addon_subs::AddonSubs::default(),
            sidecar_url: None,
            last_sub: None,
        }
    }

    /// `addon`'s subtitles for playback `gen` landed; `true` when they are this playback's.
    pub fn land_subtitles(&mut self, gen: u32, addon: &str, subs: Vec<sn_stremio::types::Subtitle>) -> bool {
        let landed = self.subs.land(gen, addon, subs, &self.tracks.subtitles);
        if landed {
            self.apply_late_pick();
            self.refresh();
        }
        landed
    }

    /// The late addon-subtitle pick (`addon_subs::AddonSubs::pick`), once the probe has read the
    /// file's own tracks (whose pick it applied itself).
    fn apply_late_pick(&mut self) {
        if !self.subs.due() {
            return;
        }
        let Some(probed) = self.player.session.probed() else { return };
        let embedded: Vec<sn_base::tracks::Track<'_>> =
            probed.subs.iter().map(|t| sn_base::tracks::Track { language: &t.language, forced: t.forced, default: t.default }).collect();
        let audio = usize::try_from(self.player.session.audio_track()).ok().and_then(|i| probed.audio.get(i)).map_or("", |a| a.language.as_str());
        let Some(j) = self.subs.pick(&self.tracks, audio, &embedded) else { return };
        let s = &self.subs.list[j];
        if self.sidecar_url.as_deref() != Some(s.url.as_str()) {
            let lang = if s.lang.is_empty() { "und" } else { s.lang.as_str() };
            log(&format!("tracks: subs from {} ({lang}) by pref, landed late", s.addon));
            self.select_addon_sub(j);
        }
    }

    /// The subtitle the subtitle languages pick now, among the file's tracks and the addons', as if
    /// the subtitle mode were On (GREEN asked for subtitles).
    fn auto_sub(&self) -> Option<PlayerCmd> {
        use sn_base::tracks::{SubPick, SubsMode, Track, TrackPrefs};
        let probed = self.player.session.probed()?;
        let embedded: Vec<Track<'_>> = probed.subs.iter().map(|t| Track { language: &t.language, forced: t.forced, default: t.default }).collect();
        let addon: Vec<Track<'_>> = self.subs.list.iter().map(|s| Track { language: &s.lang, forced: false, default: false }).collect();
        let audio = usize::try_from(self.player.session.audio_track()).ok().and_then(|i| probed.audio.get(i)).map_or("", |a| a.language.as_str());
        let prefs = TrackPrefs { mode: SubsMode::On, ..self.tracks.clone() };
        match prefs.subtitle_among(audio, &embedded, &addon)? {
            SubPick::Embedded(i) => Some(PlayerCmd::Subtitle(Some(i))),
            SubPick::Addon(j) => Some(PlayerCmd::AddonSubtitle(self.subs.list.get(j)?.key)),
        }
    }

    /// Show the `j`-th addon subtitle: the file's track off, the sidecar fetching it.
    fn select_addon_sub(&mut self, j: usize) {
        let Some(s) = self.subs.list.get(j) else { return };
        player::request_subtitle(-1);
        let id = if s.id.is_empty() { s.addon.clone() } else { format!("{}#{}", s.addon, s.id) };
        // the file says what it is (SubRip, WebVTT, a styled script): no codec is guessed
        sidecar::select(id, s.url.clone(), RequestHeaders::none(), String::new(), s.lang.clone());
        self.sidecar_url = Some(s.url.clone());
    }

    /// The marker lookup this playback is ready for: once, after its first picture, with a
    /// duration (never a live stream) and an id a service knows.
    /// The file's chapter names are published as markers at that moment. `imdb` names the
    /// work's IMDb id for an id no service knows (a `tmdb:` episode).
    pub fn take_skip_lookup(&mut self, imdb: impl FnOnce(&PlayItem) -> Option<String>) -> Option<SkipLookup> {
        let c = self.view.clock;
        if self.skip_asked || !c.picture || c.dur_ms == 0 {
            return None;
        }
        self.skip_asked = true;
        let chapters = self.view.media.chapters.iter().map(|ch| (ch.start_ms, ch.title.as_str()));
        self.chapter_skips = sn_stremio::skip::from_chapters(chapters, c.dur_ms);
        self.land_skips(self.skip_gen, Skips::NONE);
        let item = self.item.as_ref()?;
        let lookup = Lookup::of(&item.video_id).or_else(|| Lookup::with_imdb(&item.video_id, &imdb(item)?))?;
        Some(SkipLookup { gen: self.skip_gen, lookup, dur_ms: c.dur_ms })
    }

    /// A lookup's markers landed; one for an earlier playback is dropped. The file's own chapter
    /// markers win: they were cut on this encode, a crowd-sourced one may not have been.
    pub fn land_skips(&mut self, gen: u32, skips: Skips) {
        if gen == self.skip_gen {
            self.skips = self.chapter_skips.or(skips);
            self.view.skips = self.skips;
        }
    }

    /// The addon that offered what plays, by its name; "" when nothing does.
    pub fn addon(&self) -> &str {
        &self.addon
    }

    /// Start `req`, replacing whatever plays; its audio and subtitle tracks follow `tracks`
    /// (Settings > Playback, `SettingsStore::track_prefs`), picked by the probe before the Load.
    pub fn start(&mut self, req: &PlayRequest, tracks: sn_base::tracks::TrackPrefs) {
        self.stop();
        self.refused = false;
        self.item = req.item.clone();
        self.addon = req.addon.clone();
        self.progress = ProgressClock::default();
        self.tracks = tracks.clone();
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
        start.tracks = Some(tracks);
        // a resume starts where the account says the viewer stopped
        start.start_ns = i64::try_from(req.start_ms).unwrap_or(i64::MAX / 1_000_000).saturating_mul(1_000_000);
        self.begin(start);
    }

    /// **A debug build's `/tmp/stremionative-playurl`**, started at boot: no catalog item, and the
    /// declaration the trigger names — or none (`"probe": true`), which runs the probe a Stremio
    /// stream gets. Folded away without `devtriggers`.
    #[cfg(feature = "devtriggers")]
    pub fn start_dev(&mut self, start: PlaybackStart) {
        self.stop();
        self.refused = false;
        self.progress = ProgressClock::default();
        self.begin(start);
    }

    fn begin(&mut self, start: PlaybackStart) {
        match player::begin_playback(&mut self.player.session, &mut self.adapter, start) {
            Ok(outcome) => log(&format!("play: start {outcome:?}")),
            Err(refusal) => log(&format!("play: refused before the pipeline ({refusal:?})")),
        }
        self.refresh();
    }

    /// The audio sync in force from now on (clamped); the view says it at once.
    pub fn set_audio_offset(&mut self, ms: i64) {
        player::set_audio_offset(ms);
        self.refresh();
    }

    /// End the playback (idempotent). What was watched is recorded once more, as the last write.
    pub fn stop(&mut self) {
        // A debrid "still downloading" clip is not the title: it records no progress, and once it
        // has played out the viewer is told why, over the stream list the player returns to.
        let placeholder = player::debrid_placeholder();
        if placeholder && player::ended() {
            sn_base::task::spawn("toast", || {
                let _ = sn_platform::tv::toast::toast(&msg::app_player_debrid_downloading());
            });
        }
        if let Some(p) = self.progress.last(self.item.as_ref(), &self.view.clock).filter(|_| !placeholder) {
            self.pending.push((p, true));
        }
        self.item = None;
        self.addon.clear();
        self.skips = Skips::NONE;
        self.chapter_skips = Skips::NONE;
        self.skip_gen = self.skip_gen.wrapping_add(1);
        self.skip_asked = false;
        self.subs.reset();
        self.sidecar_url = None;
        self.last_sub = None;
        // the read-out ends with the playback it was opened over, never surviving into the next
        // one; switched on with nothing playing (Settings), it stays for the playback it waits for
        if self.player.session.start().is_some() {
            super::diagnostics::close();
        }
        if self.adapter.is_live() {
            player::stop_bufferfeed(&mut self.player.session, &mut self.adapter);
        }
        self.adapter.cancel_probe();
        self.player.session.clear();
        super::subtitles::release();
        self.paused = false;
        self.refused = false;
        self.refresh();
    }

    /// The player page's word.
    pub fn command(&mut self, cmd: PlayerCmd) {
        match cmd {
            PlayerCmd::TogglePause => self.set_paused(!self.paused),
            PlayerCmd::SetPaused(p) => self.set_paused(p),
            PlayerCmd::Seek(ms) | PlayerCmd::SkipTo(ms) => {
                if self.adapter.is_live() {
                    if matches!(cmd, PlayerCmd::SkipTo(_)) {
                        player::seek_forward_once();
                    }
                    let dur = u64::try_from(player::duration_ns() / 1_000_000).unwrap_or(0);
                    let ms = if dur > 0 { ms.min(dur) } else { ms };
                    player::request_seek(i64::try_from(ms).unwrap_or(i64::MAX / 1_000_000).saturating_mul(1_000_000));
                }
            }
            PlayerCmd::Audio(i) => {
                if let Err(r) = player::request_probed_audio_track(&mut self.player.session, i as i32) {
                    log(&format!("play: audio track {i} refused ({r:?})"));
                }
            }
            PlayerCmd::Subtitle(i) => {
                self.subs.chosen = true;
                let embedded = self.view.media.subs.len();
                match i {
                    Some(i) if i >= embedded && i - embedded < self.subs.list.len() => self.select_addon_sub(i - embedded),
                    _ => {
                        sidecar::deselect();
                        self.sidecar_url = None;
                        player::request_subtitle(i.map_or(-1, |i| i as i32));
                    }
                }
            }
            PlayerCmd::AddonSubtitle(key) => {
                if let Some(j) = self.subs.position_of_key(key) {
                    self.subs.chosen = true;
                    self.select_addon_sub(j);
                }
            }
            PlayerCmd::SubtitleDelay(ms) => player::set_subtitle_offset(ms),
            PlayerCmd::Stop => self.stop(),
            // the bridge restarts the request it keeps (`Bridge::retry`)
            PlayerCmd::Retry => {}
            PlayerCmd::AudioDelay(ms) => self.set_audio_offset(ms),
            PlayerCmd::Aspect(i) => player::set_aspect_mode(player::video_geometry::AspectMode::from_index(i)),
            PlayerCmd::ToggleStats => super::diagnostics::toggle(),
            PlayerCmd::ToggleSubtitles => {
                let m = &self.view.media;
                let pick = match m.sub_sel {
                    Some(i) => {
                        self.last_sub = Some(match m.addon_subs.get(i.wrapping_sub(m.subs.len())) {
                            Some(s) if i >= m.subs.len() => PlayerCmd::AddonSubtitle(s.key),
                            _ => PlayerCmd::Subtitle(Some(i)),
                        });
                        Some(PlayerCmd::Subtitle(None))
                    }
                    None => self.last_sub.take().or_else(|| self.auto_sub()),
                };
                if let Some(cmd) = pick {
                    return self.command(cmd);
                }
            }
        }
        self.refresh();
    }

    fn set_paused(&mut self, paused: bool) {
        if !self.adapter.is_live() || paused == self.paused {
            return;
        }
        if paused {
            if player::pause(&mut self.adapter) {
                self.paused = true;
            }
        } else if player::resume(&mut self.adapter) {
            self.paused = false;
        }
    }

    /// The writes waiting to be sent.
    pub fn take_progress(&mut self) -> Vec<(Progress, bool)> {
        std::mem::take(&mut self.pending)
    }

    /// The frame's pump: advance the pipeline, re-read the view. Returns whether the view's
    /// phase or media facts changed (the clock is read, not announced), and the video plane's
    /// edge (`Some(bound)` when it changed), which the loop forwards to the present gate.
    pub fn tick(&mut self, now_ms: u32) -> (bool, Option<bool>) {
        // A stopped playback's native objects are released off-frame; their main-thread tail
        // (decoder, video memory and exported window handed back) runs the first frame after
        // they return. Without these two calls a stop held the TV's decoder until the next
        // playback started: other apps (YouTube) could not play after ours had.
        player::engine::reap_abandoned_load(&mut self.adapter);
        if self.adapter.poll_release() {
            // the playback's queues, renderers and stores were just freed: hand the pages back
            super::heap::trim_soon();
        }
        // A probing start has no engine yet; the pump is what collects the probe's answer and
        // launches the Load, so it runs for either.
        if self.adapter.is_live() || self.adapter.has_probe() {
            player::pump(&mut self.player.session, &mut self.adapter, now_ms);
        }
        let edge = player::observe_video_plane(&mut self.player, &mut self.adapter);
        self.apply_late_pick();
        let moved = self.refresh();
        if super::subtitles::update(&self.player.session, now_ms) {
            sn_machine::idle::invalidate();
        }
        let playing = self.view.phase == PlayPhase::Playing;
        if let Some(p) = self.progress.frame(now_ms, playing, self.item.as_ref(), &self.view.clock).filter(|_| !player::debrid_placeholder()) {
            self.pending.push((p, false));
        }
        (moved, edge)
    }

    /// Re-read the view; `true` when its phase or media facts changed.
    fn refresh(&mut self) -> bool {
        let next = self.read();
        let changed = !next.same_phase(&self.view) || next.media != self.view.media || next.sub_delay_ms != self.view.sub_delay_ms;
        self.view = next;
        changed
    }

    fn read(&self) -> PlaybackView {
        let mut v = self.read_phase();
        v.aspect = player::aspect_mode().index();
        v.stats = super::diagnostics::enabled();
        v.skips = self.skips;
        v.sub_delay_ms = player::subtitle_offset_ms();
        v.audio_delay_ms = player::audio_offset_ms();
        if v.phase != PlayPhase::Idle {
            v.media.addon_subs = self.subs.facts();
            if let Some(j) = self.sidecar_url.as_deref().and_then(|u| self.subs.position(u)) {
                v.media.sub_sel = Some(v.media.subs.len() + j);
            }
        }
        v
    }

    fn read_phase(&self) -> PlaybackView {
        if self.refused {
            return PlaybackView { phase: PlayPhase::Failed, caption: msg::app_player_refused_c(), reason: c"", ..PlaybackView::IDLE };
        }
        let ps = &self.player.session;
        let st = player::state(ps);
        if st == PlaybackState::Idle && ps.start().is_none() {
            return PlaybackView::IDLE;
        }
        let mut v = view_of(st, self.paused, || {
            let shape = player::error_now(ps);
            (shape.caption, static_cstr(shape.readout))
        });
        let picture = player::seen_frame();
        let ms = |ns: i64| u64::try_from(ns / 1_000_000).unwrap_or(0);
        v.clock = PlayClock {
            pos_ms: ms(player::intended_pos_ns(ps)),
            dur_ms: ms(player::duration_ns()),
            seeking: picture && v.phase == PlayPhase::Working,
            ended: player::ended(),
            picture,
            buffer_pct: (v.phase == PlayPhase::Working && !picture).then(player::prime_fill_pct).flatten(),
            buffered_ms: player::buffer_ahead_ms().filter(|a| *a > 0).map_or(0, |a| ms(player::intended_pos_ns(ps)) + a as u64),
        };
        v.media = media_facts(ps, &player::track_names(), player::desired_sub_idx());
        v
    }
}

/// The media layer's state as the player page reads it (the phase; the clock and the media facts
/// are filled by the caller).
pub fn view_of(st: PlaybackState, paused: bool, error: impl FnOnce() -> (&'static CStr, &'static CStr)) -> PlaybackView {
    let (phase, caption, reason) = match st {
        PlaybackState::Error => {
            let (caption, reason) = error();
            (PlayPhase::Failed, caption, reason)
        }
        PlaybackState::Playing if paused => (PlayPhase::Paused, c"", c""),
        PlaybackState::Playing => (PlayPhase::Playing, c"", c""),
        PlaybackState::Paused => (PlayPhase::Paused, c"", c""),
        PlaybackState::Idle | PlaybackState::Resolving | PlaybackState::Connecting | PlaybackState::Buffering | PlaybackState::Seeking => {
            (PlayPhase::Working, player::busy_caption(st), c"")
        }
    };
    PlaybackView { phase, caption, reason, ..PlaybackView::IDLE }
}

/// What the media layer knows of the stream: the probe's audio tracks (named by the probe, else
/// the container), the demuxer's subtitle tracks, the ones in use, the video.
pub fn media_facts(ps: &PlaybackSession, names: &player::TrackNames, sub_sel: i32) -> MediaFacts {
    let at = |v: &[String], i: usize| v.get(i).cloned().unwrap_or_default();
    let probed = ps.probed();
    let (caps, sw) = (sn_platform::devcaps::caps(), sn_media::stream::SoftwareAudio::detected());
    let audio: Vec<TrackFacts> = match probed {
        Some(p) => p
            .audio
            .iter()
            .enumerate()
            .map(|(i, a)| TrackFacts {
                language: if a.language.is_empty() { at(&names.audio_langs, i) } else { a.language.clone() },
                name: if a.name.is_empty() { at(&names.audio, i) } else { a.name.clone() },
                codec: a.codec.clone(),
                channels: a.channels,
                atmos: a.atmos,
                fed: sn_media::stream::AudioFeed::decide_at(&a.codec, a.channels, a.sample_rate, caps, sw).fed_as(),
            })
            .collect(),
        None => Vec::new(),
    };
    let subs: Vec<TrackFacts> = (0..names.subs.len())
        .map(|i| TrackFacts { language: at(&names.sub_langs, i), name: at(&names.subs, i), ..TrackFacts::default() })
        .collect();
    let audio_sel = usize::try_from(ps.audio_track()).ok().filter(|i| *i < audio.len()).or((audio.len() == 1).then_some(0));
    let sub_sel = usize::try_from(sub_sel).ok().filter(|i| *i < subs.len());
    let video = probed.and_then(|p| p.video.as_ref()).map(|v| VideoFacts {
        codec: v.codec.clone(),
        width: v.width,
        height: v.height,
        fps_milli: (v.fps * 1000.0).round() as i64,
        dovi_profile: v.dovi.present.then_some(v.dovi.profile),
    });
    // the file's chapter titles are its text: never logged
    let chapters = player::chapters().into_iter().map(|c| ChapterFacts { start_ms: c.start_ms.max(0) as u64, title: c.title }).collect();
    MediaFacts { audio, audio_sel, subs, addon_subs: Vec::new(), sub_sel, video, chapters }
}

/// When the position is recorded, and how much was watched since the last write.
#[derive(Debug, Default)]
pub struct ProgressClock {
    /// The last write (loop ms); `None` before the first.
    last_write: Option<u32>,
    /// The position at the previous frame, and the watching it has accumulated since the write.
    last_pos: Option<u64>,
    watched: u64,
    was_playing: bool,
    /// Something played: a stop records it.
    played: bool,
}

impl ProgressClock {
    /// A frame: accumulate what was watched; answer a write when one is due (every
    /// [`PROGRESS_EVERY_MS`] while playing, and on the pause edge).
    pub fn frame(&mut self, now: u32, playing: bool, item: Option<&PlayItem>, clock: &PlayClock) -> Option<Progress> {
        if playing {
            if let Some(prev) = self.last_pos {
                let step = clock.pos_ms.saturating_sub(prev);
                if clock.pos_ms >= prev && step <= WATCH_STEP_MAX_MS {
                    self.watched += step;
                }
            }
            self.last_pos = Some(clock.pos_ms);
            self.played = true;
        } else {
            self.last_pos = None;
        }
        let paused_edge = self.was_playing && !playing && !clock.ended;
        self.was_playing = playing;
        let due = match self.last_write {
            None => playing,
            Some(at) => playing && now.wrapping_sub(at) >= PROGRESS_EVERY_MS,
        };
        if !(due || paused_edge) || clock.pos_ms == 0 {
            return None;
        }
        self.last_write = Some(now);
        Some(self.write(item?, clock, false))
    }

    /// The last write of a playback that played.
    pub fn last(&mut self, item: Option<&PlayItem>, clock: &PlayClock) -> Option<Progress> {
        if !std::mem::take(&mut self.played) || clock.pos_ms == 0 {
            return None;
        }
        Some(self.write(item?, clock, true))
    }

    /// `unload`: the playback's last write (core's `Unload`). The meta's videos are the bridge's
    /// to add (`Bridge::send_progress`), from the meta the pages hold.
    fn write(&mut self, item: &PlayItem, clock: &PlayClock, unload: bool) -> Progress {
        Progress {
            id: item.id.clone(),
            kind: item.kind.clone(),
            name: item.name.clone(),
            poster: None,
            video_id: item.video_id.clone(),
            time_offset: clock.pos_ms,
            duration: clock.dur_ms,
            watched: std::mem::take(&mut self.watched),
            unload,
            videos: None,
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

    fn req(url: &str, headers: Vec<(String, String)>) -> PlayRequest {
        PlayRequest { url: url.into(), headers, title: "T".into(), start_ms: 0, addon: "Addon".into(), item: Some(item()), hints: Default::default() }
    }

    fn item() -> PlayItem {
        PlayItem { kind: "series".into(), id: "tt2".into(), name: "Show".into(), video_id: "tt2:1:3".into(), episode: "S1 E3".into(), release: None }
    }

    #[test]
    fn malformed_stream_headers_fail_on_the_page_without_touching_the_pipeline() {
        let _g = sn_base::testlock::serial();
        // SAFETY: the test thread stands in for the main thread; nothing native is reached.
        let mut pb = Playback::new(unsafe { MainThread::assume() });
        pb.start(&req("https://cdn.example/v.m3u8", vec![("Referer".into(), "a\r\nInjected: 1".into())]), Default::default());
        assert!(!pb.adapter.is_live(), "nothing reached the native pipeline");
        assert_eq!(pb.view.phase, PlayPhase::Failed);
        assert_eq!(pb.view.caption, msg::app_player_refused_c());
        pb.stop();
        assert_eq!(pb.view, PlaybackView::IDLE, "a stop clears the read-out");
        assert!(pb.take_progress().is_empty(), "nothing played, nothing is recorded");
    }

    /// Every Stremio stream starts with a probe and no engine yet. The frame pump must run while
    /// the probe does, or its answer is never collected and the start sits on "connecting"
    /// forever (the first beta on the television: `probe: v=h264 …` and then nothing). Without
    /// FFmpeg on the host the probe answers at once, so the start must reach its refusal.
    #[test]
    fn a_probed_start_is_pumped_to_its_outcome() {
        let _g = sn_base::testlock::serial();
        // SAFETY: the test thread stands in for the main thread; no FFmpeg, so no native start.
        let mut pb = Playback::new(unsafe { MainThread::assume() });
        pb.start(&req("https://cdn.example/film.mkv", vec![]), Default::default());
        assert_eq!(pb.view.phase, PlayPhase::Working, "the probe runs");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut now = 0u32;
        while pb.view.phase == PlayPhase::Working && std::time::Instant::now() < deadline {
            now += 16;
            pb.tick(now);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(pb.view.phase, PlayPhase::Failed, "the probe's answer reached the page");
        pb.stop();
        assert!(!pb.adapter.has_probe());
    }

    #[test]
    fn commands_on_a_dead_playback_do_nothing_and_a_subtitle_pick_reaches_the_demuxer() {
        let _g = sn_base::testlock::serial();
        // SAFETY: as above; no engine, so every command is refused before the seam.
        let mut pb = Playback::new(unsafe { MainThread::assume() });
        for cmd in [PlayerCmd::TogglePause, PlayerCmd::SetPaused(true), PlayerCmd::Seek(5_000)] {
            pb.command(cmd);
        }
        assert_eq!(pb.view, PlaybackView::IDLE);
        pb.command(PlayerCmd::Subtitle(Some(1)));
        assert_eq!(player::desired_sub_idx(), 1, "the pick is the demuxer's from its next packet");
        pb.command(PlayerCmd::Subtitle(None));
        assert_eq!(player::desired_sub_idx(), -1);
    }

    /// GREEN: subtitles off remembers the one shown, and on again brings that one back.
    #[test]
    fn the_subtitle_toggle_turns_off_and_restores_the_last_one() {
        let _g = sn_base::testlock::serial();
        // SAFETY: as above; no engine, nothing native.
        let mut pb = Playback::new(unsafe { MainThread::assume() });
        pb.view.media.subs = vec![TrackFacts::default(), TrackFacts::default()];
        pb.view.media.sub_sel = Some(1);
        pb.command(PlayerCmd::ToggleSubtitles);
        assert_eq!(player::desired_sub_idx(), -1, "off");
        assert_eq!(pb.last_sub, Some(PlayerCmd::Subtitle(Some(1))));
        pb.view.media.sub_sel = None;
        pb.command(PlayerCmd::ToggleSubtitles);
        assert_eq!(player::desired_sub_idx(), 1, "the one shown before");
        assert_eq!(pb.last_sub, None);
        pb.command(PlayerCmd::ToggleSubtitles);
        assert_eq!(player::desired_sub_idx(), 1, "nothing to pick from with no probe: nothing changes");
        pb.command(PlayerCmd::Subtitle(None));
    }

    /// An addon's subtitle is picked past the file's tracks: the file's track off and the sidecar
    /// on; a file track or Off turns the sidecar off again; the delay is the player's offset. A
    /// pick in the menu is the viewer's: nothing late overrides it, and a new playback forgets it.
    #[test]
    fn an_addon_subtitle_pick_switches_to_the_sidecar_and_back() {
        let _g = sn_base::testlock::serial();
        // SAFETY: as above; no engine, nothing native.
        let mut pb = Playback::new(unsafe { MainThread::assume() });
        let gen = pb.subs.gen;
        let sub = |id: &str, lang: &str| sn_stremio::types::Subtitle { id: id.into(), url: format!("https://subs.example/{id}.srt"), lang: lang.into(), ..Default::default() };
        assert!(pb.land_subtitles(gen, "Subs", vec![sub("1", "eng"), sub("2", "ger")]));
        assert!(!pb.land_subtitles(gen.wrapping_add(7), "Late", vec![sub("3", "eng")]), "another playback's answer");
        assert_eq!(pb.subs.list.len(), 2);
        pb.command(PlayerCmd::Subtitle(Some(1)));
        assert!(pb.subs.chosen, "a menu pick is the viewer's");
        assert!(sidecar::selected(), "the addon's file goes to the sidecar renderer");
        assert_eq!(player::desired_sub_idx(), -1, "and the file's track is off");
        assert_eq!(pb.sidecar_url.as_deref(), Some("https://subs.example/2.srt"));
        pb.command(PlayerCmd::SubtitleDelay(-750));
        assert_eq!(player::subtitle_offset_ms(), -750, "a sidecar may be advanced");
        assert_eq!(pb.read().sub_delay_ms, -750);
        pb.command(PlayerCmd::Subtitle(None));
        assert!(!sidecar::selected() && pb.sidecar_url.is_none(), "Off turns the sidecar off");
        // the menu picks by key: a later landing that sorts in ahead does not move the pick
        let german = pb.subs.list[1].key;
        pb.tracks.subtitles = vec!["DE".into()];
        assert!(pb.land_subtitles(gen, "Late", vec![sub("0", "ger"), sub("4", "ger")]));
        assert_eq!(pb.subs.position_of_key(german), Some(0), "German sorted ahead of English");
        pb.command(PlayerCmd::AddonSubtitle(german));
        assert_eq!(pb.sidecar_url.as_deref(), Some("https://subs.example/2.srt"), "the key's subtitle, wherever it now stands");
        pb.command(PlayerCmd::AddonSubtitle(9_999));
        assert_eq!(pb.sidecar_url.as_deref(), Some("https://subs.example/2.srt"), "a key nobody has changes nothing");
        pb.command(PlayerCmd::Subtitle(None));
        assert!(!pb.land_subtitles(gen, "More", vec![]) || pb.sidecar_url.is_none(), "a late answer picks nothing after a choice");
        pb.stop();
        assert!(!pb.subs.chosen && pb.subs.list.is_empty() && pb.subs.gen != gen, "a new playback starts afresh");
        player::reset_subtitle();
    }

    /// One lookup per playback, after the first picture and with a duration (never live), for
    /// an id a service knows; a landing for an earlier playback is dropped.
    #[test]
    fn skip_markers_are_asked_once_after_the_first_picture() {
        let _g = sn_base::testlock::serial();
        // SAFETY: the test thread stands in for the main thread; nothing native is reached.
        let mut pb = Playback::new(unsafe { MainThread::assume() });
        pb.item = Some(item());
        assert_eq!(pb.take_skip_lookup(|_| None), None, "no picture yet");
        pb.view.clock = PlayClock { dur_ms: 0, ..clock(1000) };
        assert_eq!(pb.take_skip_lookup(|_| None), None, "live: no duration");
        pb.view.clock = clock(1000);
        let job = pb.take_skip_lookup(|_| None).expect("the first picture with a duration");
        assert_eq!((job.lookup.clone(), job.dur_ms), (Lookup::IntroDb { imdb: "tt2".into(), episode: Some((1, 3)) }, 3_000_000));
        assert_eq!(pb.take_skip_lookup(|_| None), None, "once");
        let intro = Skips { intro: Some(sn_stremio::skip::Span { start_ms: 5000, end_ms: 60_000 }), ..Skips::NONE };
        pb.land_skips(job.gen, intro);
        assert_eq!(pb.read().skips, intro, "published on the view");
        pb.stop();
        assert_eq!(pb.view.skips, Skips::NONE, "a stop clears them");
        pb.land_skips(job.gen, intro);
        assert_eq!(pb.read().skips, Skips::NONE, "a landing for an earlier playback is dropped");
        // an id no service knows asks nothing
        pb.item = Some(PlayItem { video_id: "yt_id:abc".into(), ..item() });
        pb.view.clock = clock(1000);
        assert_eq!(pb.take_skip_lookup(|_| None), None);
    }

    /// A `tmdb:` episode is asked by its show's IMDb id; the file's chapter names publish their
    /// markers at once and win over the service's.
    #[test]
    fn chapters_and_a_tmdb_id_still_find_markers() {
        let _g = sn_base::testlock::serial();
        // SAFETY: the test thread stands in for the main thread; nothing native is reached.
        let mut pb = Playback::new(unsafe { MainThread::assume() });
        pb.item = Some(PlayItem { video_id: "tmdb:9:2:4".into(), ..item() });
        pb.view.media.chapters = vec![
            ChapterFacts { start_ms: 0, title: "Cold open".into() },
            ChapterFacts { start_ms: 120_000, title: "Opening".into() },
            ChapterFacts { start_ms: 210_000, title: "Part 1".into() },
        ];
        pb.view.clock = clock(1000);
        let job = pb.take_skip_lookup(|i| (i.video_id == "tmdb:9:2:4").then(|| "tt9".to_string())).expect("asked by IMDb id");
        assert_eq!(job.lookup, Lookup::IntroDb { imdb: "tt9".into(), episode: Some((2, 4)) });
        let ours = sn_stremio::skip::Span { start_ms: 120_000, end_ms: 210_000 };
        assert_eq!(pb.read().skips.intro, Some(ours), "the chapter marker stands before the service answers");
        let credits = sn_stremio::skip::Span { start_ms: 2_800_000, end_ms: 3_000_000 };
        pb.land_skips(job.gen, Skips { intro: Some(sn_stremio::skip::Span { start_ms: 100_000, end_ms: 190_000 }), credits: Some(credits), ..Skips::NONE });
        assert_eq!((pb.read().skips.intro, pb.read().skips.credits), (Some(ours), Some(credits)));
    }

    fn clock(pos_ms: u64) -> PlayClock {
        PlayClock { pos_ms, dur_ms: 3_000_000, seeking: false, ended: false, picture: true, buffer_pct: None, buffered_ms: 0 }
    }

    #[test]
    fn progress_is_written_every_thirty_seconds_on_pause_and_on_stop() {
        let mut p = ProgressClock::default();
        let it = item();
        let first = p.frame(1000, true, Some(&it), &clock(600_000)).expect("the first playing frame records where it starts");
        assert_eq!((first.id.as_str(), first.video_id.as_str(), first.time_offset, first.watched), ("tt2", "tt2:1:3", 600_000, 0));
        let mut t = 1000;
        let mut pos = 600_000;
        let mut writes = Vec::new();
        while t < 1000 + PROGRESS_EVERY_MS + 100 {
            t += 100;
            pos += 100;
            writes.extend(p.frame(t, true, Some(&it), &clock(pos)));
        }
        assert_eq!(writes.len(), 1, "one write in thirty seconds of playing");
        assert_eq!(writes[0].watched, 30_000);
        // a seek is not watching
        assert!(p.frame(t + 100, true, Some(&it), &clock(pos + 900_000)).is_none());
        let paused = p.frame(t + 200, false, Some(&it), &clock(pos + 900_000)).expect("the pause edge records");
        assert_eq!(paused.watched, 100, "the frame after the write counted, the jump did not");
        assert!(p.frame(t + 40_000, false, Some(&it), &clock(pos + 900_000)).is_none(), "nothing while paused");
        let last = p.last(Some(&it), &clock(pos + 900_000)).expect("a stop records");
        assert!(last.unload, "the stop is core's Unload");
        assert!(!paused.unload && !writes[0].unload);
        assert!(p.last(Some(&it), &clock(pos)).is_none(), "once");
        // an unknown item records nothing; the end is the last write too
        let mut q = ProgressClock::default();
        assert!(q.frame(0, true, None, &clock(5000)).is_none());
        let mut r = ProgressClock::default();
        r.frame(0, true, Some(&it), &clock(2_999_000));
        let end = r.last(Some(&it), &PlayClock { ended: true, ..clock(2_999_000) }).unwrap();
        assert!(end.unload);
    }

    #[test]
    fn media_facts_name_tracks_by_the_probe_then_the_container() {
        let ps = PlaybackSession::IDLE;
        let names = player::TrackNames {
            audio: vec![],
            subs: vec!["SDH".into(), String::new()],
            audio_langs: vec![],
            sub_langs: vec!["eng".into(), "ger".into()],
        };
        let m = media_facts(&ps, &names, 1);
        assert!(m.audio.is_empty(), "no probe yet");
        assert_eq!(m.subs.len(), 2);
        assert_eq!((m.subs[0].language.as_str(), m.subs[0].name.as_str()), ("eng", "SDH"));
        assert_eq!(m.sub_sel, Some(1));
        assert_eq!(media_facts(&ps, &names, 5).sub_sel, None, "a pick past the list is none");
        assert_eq!(media_facts(&ps, &names, -1).sub_sel, None);
    }
}
