//! **The transport HUD's state, PURE** — when it is on screen, where its cursor is, the scrub
//! gesture (tap hops, held-key acceleration, pointer drag) and the undo window after a seek. The
//! player screen feeds it keys, pointer gestures and ticks and carries out the [`Action`]s it
//! answers; nothing here touches the media layer, the dispatcher or GL, so every transition is a
//! host test (PlxNative's `screens::player::input` and `appkit::{held_key, scrub_undo}`, ported
//! onto plain milliseconds).
//!
//! - **Visibility.** Any input shows the HUD for [`LINGER_MS`]; it stays while a scrub is live, a
//!   menu is open over the page or the pipeline is loading. A paused film lets it go after its
//!   linger like a playing one: the page's centred pause badge says *paused* on its own. BACK on a
//!   HUD over a picture puts it away ([`Hud::dismiss`]); BACK with nothing on screen leaves the
//!   player.
//! - **The cursor**: the scrubber, or one item of the control row above it — for an episode the
//!   Episodes pill, then the Subtitles, Audio and `…` discs, or, near an episode's end, the Up
//!   Next pair standing in for them.
//! - **The scrub.** Every step, rate and delay of it is a constant below, in one place. A tap of
//!   LEFT/RIGHT hops [`STEP_MS`], always the same (no escalation: fine-tuning stays predictable);
//!   taps accumulate into one target the HUD shows and the burst commits ONE seek
//!   [`TAP_COMMIT_MS`] after its last tap. Holding the key hops every [`HOLD_HOP_MS`] in steps that
//!   grow the longer it is held ([`HOLD_STEPS`]: 10 s, then 30 s, then 60 s) and commits on
//!   release — never while held, so a hold fires one seek.
//! - **The speed scrub** (⏪/⏩, [`HudKey::Rewind`]/[`HudKey::FastForward`]): each press doubles
//!   the rate ([`SPEEDS`]: ×2 … ×32), the opposite key starts over the other way. The picture is
//!   HELD (a playing film is paused; Starfish has no trick play here) while the target runs at that
//!   rate; OK or PLAY seeks there and plays, BACK cancels (and resumes a film it paused).
//! - **Jumps**: a number key `1`–`9` goes to 10–90 % of the length and `0` to the start; CH ▲/▼
//!   to the next chapter or back to this one's start (the one before within [`CHAPTER_BACK_MS`] of
//!   it), and [`JUMP_MS`] on or back in a file without chapters. They wait [`TAP_COMMIT_MS`] like a burst, so `3` then `5` is one seek,
//!   to 50 %. None of it applies to a live stream (no length). The remote's driver can bounce `Up`/`Down` mid-hold,
//!   so a held key's release waits [`RELEASE_DEBOUNCE_MS`] for the same key to come back; a hold
//!   whose release never arrives commits after [`LOST_KEYUP_MS`] without a repeat. A tap on a
//!   HIDDEN HUD only raises it. A scrub begun while PAUSED moves the target and commits nothing
//!   until OK/PLAY seeks and resumes there (or BACK drops it). A pointer drag commits on release.
//! - **Undo.** For [`UNDO_MS`] after a scrub of a playing film commits, BACK returns to where it
//!   started.
//! - **Skip.** Inside an intro or recap ([`Now::skip`]) a Skip button stands above the control
//!   row for [`SKIP_SHOW_MS`], and for as long as the HUD is up; appearing over a hidden HUD it
//!   takes the cursor, so one OK seeks [`SKIP_PAST_MS`] past the segment's end (with the scrub's
//!   undo). Automatic ([`Now::auto_skip`]) seeks there itself, once per segment, and never into a
//!   segment the viewer's last seek landed in; a short note says what was skipped.

use sn_stremio::skip::{SegKind, Segment, Span};

/// How long the HUD stays after the input that raised it.
pub const LINGER_MS: u32 = 4500;
/// What one tap of LEFT/RIGHT hops (and a held key, every [`HOLD_HOP_MS`]).
pub const STEP_MS: u64 = 10_000;
/// A burst (or a jump) commits this long after its last press, as one seek.
pub const TAP_COMMIT_MS: u32 = 700;
/// A held key hops the preview this often.
pub const HOLD_HOP_MS: u32 = 200;
/// A held key's hop grows the longer it is held: `(held at least ms, hop ms)`, in order. A tap is
/// always [`STEP_MS`] (fine-tuning stays predictable); only a hold escalates.
pub const HOLD_STEPS: [(u32, u64); 3] = [(0, STEP_MS), (1500, 30_000), (4000, 60_000)];
/// The speed scrub's rates (playback-seconds per second), one more each press of ⏪/⏩.
pub const SPEEDS: [u32; 5] = [2, 4, 8, 16, 32];
/// What CH ▲/▼ jump in a file without chapters.
pub const JUMP_MS: u64 = 600_000;
/// CH ▼ this close to a chapter's start goes to the one before (a DVD player's "previous").
pub const CHAPTER_BACK_MS: u64 = 3_000;

/// CH ▲/▼'s target from `from` in a file whose chapters start at `chapters` (in order): the next
/// chapter, or back to the start of this one. `None`: on, past the last chapter.
pub fn chapter_jump(chapters: &[u64], from: u64, fwd: bool) -> Option<i64> {
    if chapters.len() < 2 {
        return Some(if fwd { from as i64 + JUMP_MS as i64 } else { from as i64 - JUMP_MS as i64 });
    }
    if fwd {
        chapters.iter().find(|c| **c > from).map(|c| *c as i64)
    } else {
        Some(chapters.iter().rev().find(|c| **c + CHAPTER_BACK_MS <= from).map_or(0, |c| *c as i64))
    }
}
/// A held key whose repeats stop for this long lost its key-up: it is released (and commits).
pub const LOST_KEYUP_MS: u32 = 600;
/// How long a held key's release waits for the same key to come back (a driver bounce).
pub const RELEASE_DEBOUNCE_MS: u32 = 50;
/// How long BACK undoes a committed scrub.
pub const UNDO_MS: u32 = 5000;
/// A seek never lands inside the stream's last seconds: the pipeline cannot prime from there.
pub const END_GUARD_MS: u64 = 3000;
/// How long before an episode's end Up Next is offered by default (stremio-core's default
/// `next_video_notification_duration`); Settings > Up Next appears picks another
/// (`Pref::up_next_lead`).
pub const UP_NEXT_LEAD_MS: u64 = crate::stores::settings::UP_NEXT_FALLBACK_S * 1000;
/// How long the Skip button stands on its own (the HUD being up holds it longer).
pub const SKIP_SHOW_MS: u32 = 8000;
/// How far past a segment's end a skip lands.
pub const SKIP_PAST_MS: u64 = 500;
/// How long the automatic skip's note stays.
pub const SKIPPED_NOTE_MS: u32 = 2500;
/// The control row's discs: Subtitles, Audio, `…` (an episode's Episodes pill stands before them,
/// [`Focus::Episodes`]).
pub const DISCS: u8 = 3;
pub const DISC_SUBTITLES: u8 = 0;
pub const DISC_AUDIO: u8 = 1;
pub const DISC_MORE: u8 = 2;
/// How long OK is held on the Subtitles disc before it opens the subtitle menu instead of turning
/// subtitles on or off (GREEN's own rule, `GREEN_HOLD_MS`, on the disc).
pub const DISC_HOLD_MS: u32 = 500;
/// The Up Next pair, in the order it is drawn: Watch credits, Next episode (the primary).
pub const UP_NEXT_ITEMS: u8 = 2;
pub const UP_NEXT_CREDITS: u8 = 0;
pub const UP_NEXT_PLAY: u8 = 1;


/// Where a target may land: never past the end guard of a known duration.
pub fn clamp_target(ms: i64, dur: u64) -> u64 {
    let ms = ms.max(0) as u64;
    if dur > END_GUARD_MS {
        ms.min(dur - END_GUARD_MS)
    } else {
        ms
    }
}

/// `a` is later than `b` on the wrapping millisecond clock.
fn after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) >= 0
}

/// Where the HUD's cursor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Focus {
    Scrub,
    /// A disc of the control row ([`DISC_SUBTITLES`], [`DISC_AUDIO`], [`DISC_MORE`]).
    Disc(u8),
    /// An item of the Up Next pair ([`UP_NEXT_CREDITS`], [`UP_NEXT_PLAY`]).
    UpNext(u8),
    /// The Skip intro / Skip recap button.
    Skip,
    /// An episode's Episodes pill, first in the control row ([`Now::episodes`]).
    Episodes,
}

/// The remote keys the HUD answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudKey {
    Up,
    Down,
    Left,
    Right,
    /// ⏪ / ⏩: the speed scrub.
    Rewind,
    FastForward,
    /// A number key: to its tenth of the length.
    Digit(u8),
    /// CH ▲ / CH ▼: [`JUMP_MS`] on / back.
    ChUp,
    ChDown,
    Ok,
    Back,
    Play,
    Pause,
    PlayPause,
    Stop,
}

/// A key's edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEdge {
    Down,
    Repeat,
    Up,
}

/// What the HUD asks of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    TogglePause,
    SetPaused(bool),
    /// Seek to this position (ms); the film stays as it is (playing or paused).
    Seek(u64),
    /// Seek here and play: a paused scrub's target taken by OK/PLAY.
    SeekAndPlay(u64),
    /// Skip a segment: seek to this position, landing on the first keyframe AT or after it (a
    /// seek lands on the keyframe before its target, which put a skip back inside the intro).
    SkipTo(u64),
    /// Open the menu behind a disc.
    OpenMenu(u8),
    /// Subtitles on or off: a short OK on the Subtitles disc (GREEN's press, on the disc).
    ToggleSubtitles,
    /// Watch credits (`UP_NEXT_CREDITS`) or play the next episode (`UP_NEXT_PLAY`).
    UpNext(u8),
    /// Open the Episodes panel.
    OpenEpisodes,
    /// *Start over*: from the beginning (the offer after a resume, in the Skip slot).
    StartOver,
    Leave,
}

/// What the screen knows this instant, for a decision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Now {
    pub ms: u32,
    /// The published position and the duration (0 unknown).
    pub pos: u64,
    pub dur: u64,
    pub paused: bool,
    /// The playback has shown a picture.
    pub picture: bool,
    /// Something holds the HUD up besides its timer: a menu over the page, the pipeline loading.
    pub pinned: bool,
    /// Up Next is on offer (an episode's end with a next one).
    pub offered: bool,
    /// The intro or recap the playhead is in, when Skip intros is not Off.
    pub skip: Option<Segment>,
    /// Skip intros is Automatic.
    pub auto_skip: bool,
    /// The Episodes pill stands in the control row (an episode whose show lists its episodes).
    pub episodes: bool,
    /// *Start over* is on offer in the Skip slot (a resumed playback's first seconds); a Skip
    /// button, when one stands, takes the slot.
    pub start_over: bool,
}

/// The HUD's timer, dismissal and cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hud {
    until: u32,
    dismissed: bool,
    pub focus: Focus,
}

impl Hud {
    /// A fresh playback: the HUD up for its linger, the cursor on the scrubber.
    pub fn new(now: u32) -> Self {
        Hud { until: now.wrapping_add(LINGER_MS), dismissed: false, focus: Focus::Scrub }
    }

    /// Any input: up for another linger.
    pub fn wake(&mut self, now: u32) {
        self.until = now.wrapping_add(LINGER_MS);
        self.dismissed = false;
    }

    /// Put away (BACK, UP off the top row); the cursor goes home.
    pub fn dismiss(&mut self) {
        self.dismissed = true;
        self.focus = Focus::Scrub;
    }

    /// On screen at `now`? `pinned` holds it past its timer; a dismissal hides it anyway.
    pub fn visible(&self, now: u32, pinned: bool) -> bool {
        !self.dismissed && (pinned || !after(now, self.until))
    }
}

/// The scrub gesture (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scrub {
    /// Where the knob is shown, while a scrub is live.
    pub preview: Option<u64>,
    /// Where the film was when it began.
    origin: u64,
    /// The key held: -1, 0, 1.
    dir: i8,
    held: bool,
    hold_since: u32,
    last_advance: u32,
    last_repeat: u32,
    /// A held key's provisional release (0 none).
    release_at: u32,
    /// A tap's commit (0 none).
    commit_at: u32,
    /// Begun on a hidden HUD: a tap only raised it.
    reveal: bool,
    /// Begun while paused: commits only on OK/PLAY.
    paused: bool,
    /// A pointer is dragging the knob.
    drag: bool,
    /// The speed scrub: the rate's index into [`SPEEDS`] plus one, signed by direction (0 off).
    speed: i8,
    /// The speed scrub paused a playing film: a cancel resumes it.
    resume: bool,
}

/// A committed scrub: where to, and where from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Commit {
    pub target: u64,
    pub origin: u64,
    pub paused: bool,
}

impl Scrub {
    pub fn live(&self) -> bool {
        self.preview.is_some()
    }

    pub fn dragging(&self) -> bool {
        self.drag
    }

    /// A paused film's target, waiting for OK/PLAY.
    pub fn waiting(&self) -> bool {
        self.preview.is_some() && self.paused && self.dir == 0 && !self.drag
    }

    /// Is the hold engaged (repeats arrived)?
    pub fn holding(&self) -> bool {
        self.held && self.dir != 0
    }

    /// The hop a hold makes at `now` ([`HOLD_STEPS`]), while one is engaged.
    pub fn hold_step(&self, now: u32) -> Option<u64> {
        self.holding().then(|| {
            let held = now.wrapping_sub(self.hold_since);
            HOLD_STEPS.iter().rev().find(|(from, _)| held >= *from).map_or(STEP_MS, |(_, step)| *step)
        })
    }

    /// The speed scrub running: forward or not, and its multiple.
    pub fn speed(&self) -> Option<(bool, u32)> {
        (self.speed != 0).then(|| (self.speed > 0, SPEEDS[usize::from(self.speed.unsigned_abs()) - 1]))
    }

    /// Does dropping this gesture resume the film (the speed scrub paused it)?
    pub fn resumes(&self) -> bool {
        self.resume
    }

    /// A press of ⏪/⏩: the rate doubles (the other way starts over at ×2). `true` when the
    /// picture must be held now (the film was playing).
    pub fn speed_press(&mut self, fwd: bool, n: &Now) -> bool {
        let at = self.begin(n);
        self.preview = Some(at);
        let sign: i8 = if fwd { 1 } else { -1 };
        let mag = if self.speed.signum() == sign { (self.speed.abs() + 1).min(SPEEDS.len() as i8) } else { 1 };
        self.speed = sign * mag;
        (self.dir, self.held, self.release_at, self.commit_at, self.drag, self.reveal) = (0, false, 0, 0, false, false);
        self.last_advance = n.ms;
        let hold = !self.paused;
        if hold {
            (self.paused, self.resume) = (true, true);
        }
        hold
    }

    /// A jump to `ms` (a number key, CH ▲/▼): it commits after [`TAP_COMMIT_MS`] of quiet, or
    /// waits for OK while paused.
    pub fn jump_to(&mut self, ms: i64, n: &Now) {
        self.begin(n);
        (self.dir, self.held, self.release_at, self.drag, self.reveal, self.speed) = (0, false, 0, false, false, 0);
        self.preview = Some(clamp_target(ms, n.dur));
        self.commit_at = if self.paused { 0 } else { n.ms.max(1).wrapping_add(TAP_COMMIT_MS) };
    }

    fn begin(&mut self, n: &Now) -> u64 {
        match self.preview {
            Some(p) => p,
            None => {
                *self = Scrub { origin: n.pos, paused: n.paused, ..Scrub::default() };
                n.pos
            }
        }
    }

    /// A fresh LEFT/RIGHT press. `reveal_only`: the HUD was hidden, so a tap raises it and hops
    /// nothing (a hold still scrubs).
    pub fn press(&mut self, fwd: bool, n: &Now, reveal_only: bool) {
        let dir = if fwd { 1 } else { -1 };
        // the held key coming back inside the debounce is the same hold
        if self.release_at != 0 && self.dir == dir {
            self.release_at = 0;
            self.last_repeat = n.ms;
            self.last_advance = n.ms;
            return;
        }
        let fresh = self.preview.is_none();
        let at = self.begin(n);
        self.speed = 0;
        self.dir = dir;
        self.held = false;
        self.release_at = 0;
        self.commit_at = 0;
        self.drag = false;
        self.last_repeat = n.ms;
        if reveal_only && fresh {
            self.reveal = true;
            self.preview = Some(at);
            return;
        }
        self.reveal = false;
        self.preview = Some(clamp_target(at as i64 + STEP_MS as i64 * i64::from(dir), n.dur));
    }

    /// An auto-repeat of the held key: the hold engages.
    pub fn repeat(&mut self, now: u32) {
        if self.dir == 0 {
            return;
        }
        self.last_repeat = now;
        if !self.held {
            self.held = true;
            self.hold_since = now;
            self.last_advance = now;
            self.reveal = false;
        }
    }

    /// The key came up.
    pub fn release(&mut self, now: u32) {
        if self.dir == 0 {
            return;
        }
        if self.held {
            self.release_at = now.max(1).wrapping_add(RELEASE_DEBOUNCE_MS);
            return;
        }
        self.dir = 0;
        if self.reveal {
            self.cancel();
        } else if !self.paused {
            self.commit_at = now.max(1).wrapping_add(TAP_COMMIT_MS);
        }
    }

    /// A frame: run a hold, settle a release, fire a tap's commit.
    pub fn tick(&mut self, n: &Now) -> Option<Commit> {
        if let (Some((fwd, rate)), Some(p)) = (self.speed(), self.preview) {
            let dt = i64::from(n.ms.wrapping_sub(self.last_advance));
            self.last_advance = n.ms;
            let delta = i64::from(rate) * dt * if fwd { 1 } else { -1 };
            self.preview = Some(clamp_target(p as i64 + delta, n.dur));
        }
        // a provisional release holds the preview still while it waits for the key to come back
        if self.holding() && self.release_at == 0 {
            // a hop every HOLD_HOP_MS, whatever the remote's repeat rate
            while after(n.ms, self.last_advance.wrapping_add(HOLD_HOP_MS)) {
                self.last_advance = self.last_advance.wrapping_add(HOLD_HOP_MS);
                let step = self.hold_step(self.last_advance).unwrap_or(STEP_MS);
                let hop = step as i64 * i64::from(self.dir);
                if let Some(p) = self.preview {
                    self.preview = Some(clamp_target(p as i64 + hop, n.dur));
                }
            }
            if self.release_at == 0 && n.ms.wrapping_sub(self.last_repeat) > LOST_KEYUP_MS {
                self.release_at = n.ms.max(1);
            }
        }
        if self.release_at != 0 && after(n.ms, self.release_at) {
            self.release_at = 0;
            self.held = false;
            self.dir = 0;
            if !self.paused {
                return self.commit();
            }
        }
        if self.commit_at != 0 && after(n.ms, self.commit_at) {
            self.commit_at = 0;
            return self.commit();
        }
        None
    }

    /// A pointer on the bar at `ms`.
    pub fn drag_to(&mut self, ms: u64, n: &Now) {
        self.begin(n);
        self.dir = 0;
        self.held = false;
        self.release_at = 0;
        self.commit_at = 0;
        self.drag = true;
        self.preview = Some(clamp_target(ms as i64, n.dur));
    }

    /// End the gesture with its target.
    pub fn commit(&mut self) -> Option<Commit> {
        let target = self.preview.take()?;
        let c = Commit { target, origin: self.origin, paused: self.paused };
        *self = Scrub::default();
        Some(c)
    }

    /// Throw the gesture away.
    pub fn cancel(&mut self) {
        *self = Scrub::default();
    }
}

/// After a scrub of a playing film: where BACK returns to, until when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Undo {
    pub origin: u64,
    until: u32,
}

impl Undo {
    pub fn armed(&self, now: u32) -> bool {
        !after(now, self.until)
    }
}

/// Is Up Next on offer? An episode with a next one, from its credits' start (`credits`, the
/// marker's, when the viewer's setting consults it) or else inside `lead` ms of its end, not
/// declined (Watch credits) and not over.
pub fn up_next_offered(has_next: bool, declined: bool, pos: u64, dur: u64, ended: bool, credits: Option<u64>, lead: u64) -> bool {
    let from = credits.filter(|c| *c < dur).unwrap_or(dur.saturating_sub(lead));
    has_next && !declined && !ended && dur > lead && pos >= from
}

/// The HUD: timer, cursor, scrub and undo, driven by keys, pointer gestures and ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct Controller {
    pub hud: Hud,
    pub scrub: Scrub,
    pub undo: Option<Undo>,
    /// Up Next was on offer at the last tick (its appearing edge seats the cursor on it).
    offered: bool,
    /// The segment the Skip button was last raised for, and until when it stands on its own.
    skip_seg: Option<Span>,
    skip_until: u32,
    /// The intro, the recap was skipped automatically: never again in this playback.
    auto_spent: [bool; 2],
    /// Where the viewer's last seek landed.
    manual_seek: Option<u64>,
    /// The automatic skip's note: what was skipped, until when.
    note: Option<(SegKind, u32)>,
    /// OK went down on the Subtitles disc at this time: its release toggles subtitles, a hold
    /// past [`DISC_HOLD_MS`] opens their menu instead.
    disc_down: Option<u32>,
    /// The file's chapter starts, for CH ▲/▼ (the screen keeps them current).
    pub chapters: Vec<u64>,
}

impl Controller {
    pub fn new(now: u32) -> Self {
        Controller {
            hud: Hud::new(now),
            scrub: Scrub::default(),
            undo: None,
            offered: false,
            skip_seg: None,
            skip_until: now,
            auto_spent: [false; 2],
            manual_seek: None,
            note: None,
            disc_down: None,
            chapters: Vec::new(),
        }
    }

    /// OK is held on the Subtitles disc (the frame clock must run until it opens the menu).
    pub fn disc_held(&self) -> bool {
        self.disc_down.is_some()
    }

    /// The Subtitles disc's hold, once it has lasted: the menu (once), the press spent.
    fn disc_hold(&mut self, n: &Now) -> Option<Action> {
        let t = self.disc_down?;
        if self.hud.focus != Focus::Disc(DISC_SUBTITLES) {
            self.disc_down = None;
            return None;
        }
        if n.ms.wrapping_sub(t) < DISC_HOLD_MS {
            return None;
        }
        self.disc_down = None;
        self.hud.wake(n.ms);
        Some(Action::OpenMenu(DISC_SUBTITLES))
    }

    /// The segment the Skip button offers now: raised for it, and on its own time or with the HUD.
    pub fn skip_shown(&self, n: &Now) -> Option<Segment> {
        let s = n.skip?;
        (self.skip_seg == Some(s.span) && (!after(n.ms, self.skip_until) || self.visible(n))).then_some(s)
    }

    /// Something stands in the Skip slot: the Skip button, else the *Start over* offer.
    pub fn slot_shown(&self, n: &Now) -> bool {
        self.skip_shown(n).is_some() || (n.start_over && n.skip.is_none())
    }

    /// What the automatic skip's note names, while it shows.
    pub fn note(&self, n: &Now) -> Option<SegKind> {
        self.note.filter(|(_, until)| !after(n.ms, *until)).map(|(k, _)| k)
    }

    /// OK or a click on the Skip button: past the segment's end, with the scrub's undo.
    fn skip(&mut self, s: Segment, n: &Now) -> Action {
        let target = clamp_target((s.span.end_ms + SKIP_PAST_MS) as i64, n.dur);
        self.undo = Some(Undo { origin: n.pos, until: n.ms.wrapping_add(UNDO_MS) });
        self.manual_seek = Some(target);
        self.hud.focus = Focus::Scrub;
        Action::SkipTo(target)
    }

    /// A frame of the Skip button: raised when its segment arrives, holding the cursor while it
    /// stands over a hidden HUD; or the segment skipped automatically.
    fn tick_skip(&mut self, n: &Now) -> Option<Action> {
        if let Some(s) = n.skip {
            let slot = usize::from(s.kind == SegKind::Recap);
            let by_hand = self.manual_seek.is_some_and(|t| s.span.contains(t));
            if n.auto_skip && n.picture && !self.auto_spent[slot] && !by_hand && !self.scrub.live() {
                self.auto_spent[slot] = true;
                // the button never stands on its own for it
                (self.skip_seg, self.skip_until) = (Some(s.span), n.ms);
                self.note = Some((s.kind, n.ms.wrapping_add(SKIPPED_NOTE_MS)));
                return Some(Action::SkipTo(clamp_target((s.span.end_ms + SKIP_PAST_MS) as i64, n.dur)));
            }
        }
        let seg = n.skip.map(|s| s.span);
        if seg != self.skip_seg {
            (self.skip_seg, self.skip_until) = (seg, n.ms.wrapping_add(SKIP_SHOW_MS));
        }
        if self.slot_shown(n) {
            // alone on the picture, one OK skips (or starts over)
            if !self.visible(n) {
                self.hud.focus = Focus::Skip;
            }
        } else if self.hud.focus == Focus::Skip {
            self.hud.focus = Focus::Scrub;
        }
        None
    }

    /// Everything that holds the HUD up besides its timer.
    fn pinned(&self, n: &Now) -> bool {
        n.pinned || self.scrub.live()
    }

    pub fn visible(&self, n: &Now) -> bool {
        self.hud.visible(n.ms, self.pinned(n))
    }

    /// How many items the control row holds now.
    pub fn row_len(n: &Now) -> u8 {
        if n.offered {
            UP_NEXT_ITEMS
        } else {
            DISCS
        }
    }

    fn row_item(n: &Now, i: u8) -> Focus {
        if n.offered {
            Focus::UpNext(i.min(UP_NEXT_ITEMS - 1))
        } else {
            Focus::Disc(i.min(DISCS - 1))
        }
    }

    /// The position the bar shows: a live scrub's preview, else the playhead.
    pub fn shown_pos(&self, n: &Now) -> u64 {
        self.scrub.preview.unwrap_or(n.pos)
    }

    fn committed(&mut self, c: Commit, now: u32) -> Action {
        self.manual_seek = Some(c.target);
        // a paused film's drag stays paused; a playing film's scrub can be taken back
        self.undo = (!c.paused && c.target.abs_diff(c.origin) >= 1000).then_some(Undo { origin: c.origin, until: now.wrapping_add(UNDO_MS) });
        Action::Seek(c.target)
    }

    /// A remote key. Returns what the screen must do.
    pub fn key(&mut self, key: HudKey, edge: KeyEdge, n: &Now) -> Option<Action> {
        let visible = self.visible(n);
        let skip = self.skip_shown(n);
        if edge == KeyEdge::Up {
            match key {
                HudKey::Left | HudKey::Right => self.scrub.release(n.ms),
                // the pointer button's release, which the loop spells as OK's up
                HudKey::Ok if self.scrub.dragging() => {
                    let c = self.scrub.commit()?;
                    self.hud.wake(n.ms);
                    return Some(self.committed(c, n.ms));
                }
                // a short press on the Subtitles disc: subtitles on or off
                HudKey::Ok if self.disc_down.is_some() => {
                    self.disc_down = None;
                    return (self.hud.focus == Focus::Disc(DISC_SUBTITLES)).then_some(Action::ToggleSubtitles);
                }
                _ => {}
            }
            return None;
        }
        let down = edge == KeyEdge::Down;
        match key {
            HudKey::Back | HudKey::Stop if !down => None,
            HudKey::Stop => Some(Action::Leave),
            HudKey::Back => {
                if self.scrub.live() {
                    let resume = self.scrub.resumes();
                    self.scrub.cancel();
                    self.hud.wake(n.ms);
                    return resume.then_some(Action::SetPaused(false));
                }
                if visible {
                    if let Some(u) = self.undo.take().filter(|u| u.armed(n.ms)) {
                        self.hud.wake(n.ms);
                        self.manual_seek = Some(u.origin);
                        return Some(Action::Seek(u.origin));
                    }
                    // nothing to hide the HUD over before the first picture: BACK leaves
                    if n.picture {
                        self.hud.dismiss();
                        return None;
                    }
                }
                Some(Action::Leave)
            }
            HudKey::Play | HudKey::Pause | HudKey::PlayPause => {
                if !down {
                    return None;
                }
                self.hud.wake(n.ms);
                if key != HudKey::Pause && self.scrub.waiting() {
                    let c = self.scrub.commit()?;
                    return Some(Action::SeekAndPlay(c.target));
                }
                Some(match key {
                    HudKey::Play => Action::SetPaused(false),
                    HudKey::Pause => Action::SetPaused(true),
                    _ => Action::TogglePause,
                })
            }
            HudKey::Ok => {
                if !down {
                    return self.disc_hold(n);
                }
                self.hud.wake(n.ms);
                if self.scrub.waiting() {
                    let c = self.scrub.commit()?;
                    return Some(Action::SeekAndPlay(c.target));
                }
                if let (Some(s), Focus::Skip) = (skip, self.hud.focus) {
                    return Some(self.skip(s, n));
                }
                if self.hud.focus == Focus::Skip && n.start_over {
                    self.hud.focus = Focus::Scrub;
                    return Some(Action::StartOver);
                }
                match (visible, self.hud.focus) {
                    (true, Focus::Episodes) => Some(Action::OpenEpisodes),
                    // the Subtitles disc is a toggle: its release decides (a hold opens the menu)
                    (true, Focus::Disc(DISC_SUBTITLES)) => {
                        self.disc_down = Some(n.ms);
                        None
                    }
                    (true, Focus::Disc(i)) => Some(Action::OpenMenu(i)),
                    (true, Focus::UpNext(i)) => Some(Action::UpNext(i)),
                    _ => Some(Action::TogglePause),
                }
            }
            HudKey::Up | HudKey::Down => {
                if !down {
                    return None;
                }
                if !visible {
                    self.hud.wake(n.ms);
                    // inside a segment the key raises the Skip button with the HUD, on it
                    let raised = n.skip.is_some() && self.skip_seg == n.skip.map(|s| s.span);
                    self.hud.focus = if raised { Focus::Skip } else { Focus::Scrub };
                    return None;
                }
                match (key, self.hud.focus) {
                    (HudKey::Up, Focus::Scrub) => {
                        self.scrub.cancel();
                        self.hud.focus = Self::row_item(n, if n.offered { UP_NEXT_PLAY } else { 0 });
                        self.hud.wake(n.ms);
                    }
                    // the Skip button stands above the row
                    (HudKey::Up, Focus::Disc(_) | Focus::UpNext(_) | Focus::Episodes) if skip.is_some() || n.start_over => {
                        self.hud.focus = Focus::Skip;
                        self.hud.wake(n.ms);
                    }
                    (HudKey::Down, Focus::Skip) => {
                        self.hud.focus = Self::row_item(n, Self::row_len(n) - 1);
                        self.hud.wake(n.ms);
                    }
                    // UP off the top row puts the HUD away
                    (HudKey::Up, _) => self.hud.dismiss(),
                    (_, _) => {
                        self.hud.focus = Focus::Scrub;
                        self.hud.wake(n.ms);
                    }
                }
                None
            }
            HudKey::Rewind | HudKey::FastForward | HudKey::Digit(_) | HudKey::ChUp | HudKey::ChDown => {
                self.hud.wake(n.ms);
                if !down || n.dur == 0 {
                    return None;
                }
                self.hud.focus = Focus::Scrub;
                let from = self.shown_pos(n) as i64;
                match key {
                    HudKey::Digit(d) => self.scrub.jump_to((n.dur as i64) * i64::from(d.min(9)) / 10, n),
                    HudKey::ChUp | HudKey::ChDown => {
                        if let Some(t) = chapter_jump(&self.chapters, from as u64, key == HudKey::ChUp) {
                            self.scrub.jump_to(t, n);
                        }
                    }
                    _ => {
                        if self.scrub.speed_press(key == HudKey::FastForward, n) {
                            return Some(Action::SetPaused(true));
                        }
                    }
                }
                None
            }
            HudKey::Left | HudKey::Right => {
                let fwd = key == HudKey::Right;
                match (visible, self.hud.focus) {
                    (true, Focus::Skip) => self.hud.wake(n.ms),
                    (true, Focus::Episodes) => {
                        if down && fwd {
                            self.hud.focus = Focus::Disc(0);
                        }
                        self.hud.wake(n.ms);
                    }
                    (true, Focus::Disc(0)) if down && !fwd && n.episodes && !n.offered => {
                        self.hud.focus = Focus::Episodes;
                        self.hud.wake(n.ms);
                    }
                    (true, Focus::Disc(i) | Focus::UpNext(i)) => {
                        if down {
                            let i = if fwd { i.saturating_add(1).min(Self::row_len(n) - 1) } else { i.saturating_sub(1) };
                            self.hud.focus = Self::row_item(n, i);
                        }
                        self.hud.wake(n.ms);
                    }
                    // live (no duration): nothing to seek to, the key only raises the HUD
                    _ if n.dur == 0 => self.hud.wake(n.ms),
                    _ => {
                        match edge {
                            KeyEdge::Down => self.scrub.press(fwd, n, !visible),
                            _ => self.scrub.repeat(n.ms),
                        }
                        self.hud.focus = Focus::Scrub;
                        self.hud.wake(n.ms);
                    }
                }
                None
            }
        }
    }

    /// A frame. Returns a seek the scrub committed, or an automatic skip.
    pub fn tick(&mut self, n: &Now) -> Option<Action> {
        if let Some(a) = self.disc_hold(n) {
            return Some(a);
        }
        if let Some(a) = self.tick_skip(n) {
            return Some(a);
        }
        // Up Next appearing seats the cursor on its primary; leaving returns it to the bar
        if n.offered && !self.offered {
            self.hud.wake(n.ms);
            self.hud.focus = Focus::UpNext(UP_NEXT_PLAY);
        } else if !n.offered && matches!(self.hud.focus, Focus::UpNext(_)) {
            self.hud.focus = Focus::Scrub;
        }
        if self.hud.focus == Focus::Episodes && (!n.episodes || n.offered) {
            self.hud.focus = if n.offered { Focus::UpNext(UP_NEXT_PLAY) } else { Focus::Disc(0) };
        }
        self.offered = n.offered;
        if self.undo.is_some_and(|u| !u.armed(n.ms)) {
            self.undo = None;
        }
        let c = self.scrub.tick(n)?;
        self.hud.wake(n.ms);
        Some(self.committed(c, n.ms))
    }

    /// The pointer moved over a control: the cursor follows it.
    pub fn hover(&mut self, focus: Option<Focus>, n: &Now) {
        self.hud.wake(n.ms);
        if let Some(f) = focus {
            self.hud.focus = f;
        }
    }

    /// A click on a control.
    pub fn click(&mut self, focus: Focus, n: &Now) -> Option<Action> {
        self.hud.wake(n.ms);
        self.hud.focus = focus;
        match focus {
            Focus::Disc(i) => Some(Action::OpenMenu(i)),
            Focus::UpNext(i) => Some(Action::UpNext(i)),
            Focus::Episodes => Some(Action::OpenEpisodes),
            Focus::Skip => n.skip.map(|s| self.skip(s, n)).or(n.start_over.then_some(Action::StartOver)),
            Focus::Scrub => None,
        }
    }

    /// The pointer pressed or dragged on the bar at `ms`.
    pub fn drag(&mut self, ms: u64, n: &Now) {
        self.hud.wake(n.ms);
        if n.dur == 0 {
            return; // live: no bar to scrub
        }
        self.hud.focus = Focus::Scrub;
        self.scrub.drag_to(ms, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(ms: u32) -> Now {
        Now { ms, pos: 600_000, dur: 3_600_000, paused: false, picture: true, pinned: false, offered: false, skip: None, auto_skip: false, episodes: false, start_over: false }
    }

    #[test]
    fn the_hud_shows_on_input_and_hides_after_its_linger() {
        let mut c = Controller::new(0);
        assert!(c.visible(&now(0)));
        assert!(c.visible(&now(LINGER_MS - 1)));
        assert!(!c.visible(&now(LINGER_MS)), "gone after its linger");
        assert!(!c.visible(&Now { paused: true, ..now(LINGER_MS + 10) }), "a paused film lets it go too: the pause badge stays");
        assert!(c.visible(&Now { pinned: true, ..now(LINGER_MS + 10) }), "a menu over the page keeps it");
        // any key shows it again
        assert_eq!(c.key(HudKey::Down, KeyEdge::Down, &now(10_000)), None);
        assert!(c.visible(&now(10_000 + LINGER_MS - 1)));
        // the wrapping clock
        let mut w = Controller::new(u32::MAX - 100);
        assert!(w.visible(&now(50)), "the deadline survives the clock's wrap");
        w.hud.wake(u32::MAX - 10);
        assert!(!w.visible(&now(LINGER_MS)));
    }

    #[test]
    fn back_hides_the_hud_first_and_a_second_back_leaves() {
        let mut c = Controller::new(0);
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &now(100)), None, "the first BACK hides the HUD");
        assert!(!c.visible(&Now { paused: true, ..now(200) }), "dismissed even over a paused film");
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &now(300)), Some(Action::Leave), "a second BACK leaves");
        // before the first picture there is nothing to hide the HUD over
        let mut c = Controller::new(0);
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &Now { picture: false, pinned: true, ..now(100) }), Some(Action::Leave));
        assert_eq!(c.key(HudKey::Stop, KeyEdge::Down, &now(100)), Some(Action::Leave));
        assert_eq!(c.key(HudKey::Back, KeyEdge::Up, &now(100)), None, "only the press counts");
    }

    #[test]
    fn ok_toggles_pause_on_the_bar_and_opens_the_focused_disc() {
        // (the Subtitles disc, first in the row, is a toggle: its own test below)
        let mut c = Controller::new(0);
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &now(10)), Some(Action::TogglePause));
        assert_eq!(c.key(HudKey::Up, KeyEdge::Down, &now(20)), None);
        assert_eq!(c.hud.focus, Focus::Disc(0), "UP from the bar reaches the control row");
        c.key(HudKey::Right, KeyEdge::Down, &now(30));
        c.key(HudKey::Right, KeyEdge::Down, &now(40));
        c.key(HudKey::Right, KeyEdge::Down, &now(50));
        assert_eq!(c.hud.focus, Focus::Disc(DISC_MORE), "the row clamps at its end");
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &now(60)), Some(Action::OpenMenu(DISC_MORE)));
        c.key(HudKey::Left, KeyEdge::Down, &now(70));
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &now(80)), Some(Action::OpenMenu(DISC_AUDIO)));
        assert_eq!(c.key(HudKey::Down, KeyEdge::Down, &now(90)), None);
        assert_eq!(c.hud.focus, Focus::Scrub);
        c.key(HudKey::Up, KeyEdge::Down, &now(100));
        c.key(HudKey::Up, KeyEdge::Down, &now(110));
        assert!(!c.visible(&now(120)), "UP off the top row puts the HUD away");
        // a hidden HUD's OK is play/pause, whatever the cursor was on
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &now(130)), Some(Action::TogglePause));
        assert_eq!(c.key(HudKey::Pause, KeyEdge::Down, &now(140)), Some(Action::SetPaused(true)));
        assert_eq!(c.key(HudKey::Play, KeyEdge::Down, &now(150)), Some(Action::SetPaused(false)));
    }

    /// The Subtitles disc is a toggle: a short OK turns subtitles on or off on its RELEASE; held
    /// past [`DISC_HOLD_MS`] it opens the menu once (by the key's repeat or the frame clock) and
    /// the release that follows toggles nothing. Moving off the disc spends the press.
    #[test]
    fn the_subtitles_disc_toggles_on_a_press_and_opens_its_menu_on_a_hold() {
        let mut c = Controller::new(0);
        c.key(HudKey::Up, KeyEdge::Down, &now(10));
        assert_eq!(c.hud.focus, Focus::Disc(DISC_SUBTITLES));
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &now(20)), None, "the press waits for its release");
        assert!(c.disc_held());
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Up, &now(120)), Some(Action::ToggleSubtitles));
        assert!(!c.disc_held());
        // held: the repeat opens the menu, the release is spent
        c.key(HudKey::Ok, KeyEdge::Down, &now(200));
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Repeat, &now(400)), None, "not yet");
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Repeat, &now(200 + DISC_HOLD_MS)), Some(Action::OpenMenu(DISC_SUBTITLES)));
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Up, &now(800)), None);
        // held with no repeat: the frame clock opens it
        c.key(HudKey::Ok, KeyEdge::Down, &now(1000));
        assert_eq!(c.tick(&now(1100)), None);
        assert_eq!(c.tick(&now(1000 + DISC_HOLD_MS)), Some(Action::OpenMenu(DISC_SUBTITLES)));
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Up, &now(1600)), None);
        // the other discs open at once
        c.key(HudKey::Right, KeyEdge::Down, &now(1700));
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &now(1710)), Some(Action::OpenMenu(DISC_AUDIO)));
    }

    /// A live channel has no duration and nothing to seek to: a seek there reopened the playlist
    /// at its edge and stalled the picture (the beta's live TV). LEFT/RIGHT and the bar's pointer
    /// only raise the HUD.
    #[test]
    fn a_live_stream_never_seeks() {
        let live = |ms| Now { dur: 0, ..now(ms) };
        let mut c = Controller::new(0);
        let mut acts = Vec::new();
        acts.push(c.key(HudKey::Right, KeyEdge::Down, &live(100)));
        acts.push(c.key(HudKey::Right, KeyEdge::Up, &live(150)));
        for ms in (300..3000).step_by(50) {
            acts.push(c.key(HudKey::Left, KeyEdge::Down, &live(ms))); // a hold
            acts.push(c.tick(&live(ms + 1)));
        }
        acts.push(c.key(HudKey::Left, KeyEdge::Up, &live(3000)));
        c.drag(1000, &live(3100));
        acts.push(c.key(HudKey::Ok, KeyEdge::Up, &live(3200)));
        for ms in (3200..20_000).step_by(100) {
            acts.push(c.tick(&live(ms)));
        }
        let seeks: Vec<_> = acts.into_iter().flatten().filter(|a| matches!(a, Action::Seek(_) | Action::SeekAndPlay(_))).collect();
        assert_eq!(seeks, [], "nothing seeks on a live stream");
        assert!(!c.scrub.live());
        let mut c = Controller::new(0);
        c.hud.dismiss();
        c.key(HudKey::Right, KeyEdge::Down, &live(100));
        assert!(c.visible(&live(200)), "the keys still raise the HUD");
    }

    #[test]
    fn taps_hop_and_accumulate_then_commit_once() {
        let mut c = Controller::new(0);
        c.key(HudKey::Right, KeyEdge::Down, &now(100));
        c.key(HudKey::Right, KeyEdge::Up, &now(150));
        assert_eq!(c.scrub.preview, Some(610_000));
        c.key(HudKey::Right, KeyEdge::Down, &now(300));
        c.key(HudKey::Right, KeyEdge::Up, &now(350));
        c.key(HudKey::Left, KeyEdge::Down, &now(400));
        c.key(HudKey::Left, KeyEdge::Up, &now(450));
        assert_eq!(c.shown_pos(&now(460)), 610_000, "two hops forward, one back");
        assert_eq!(c.tick(&now(450 + TAP_COMMIT_MS - 1)), None, "the burst is still open");
        assert_eq!(c.tick(&now(450 + TAP_COMMIT_MS)), Some(Action::Seek(610_000)), "one seek for the burst");
        assert!(!c.scrub.live());
        assert_eq!(c.tick(&now(2000)), None);
    }

    #[test]
    fn a_tap_on_a_hidden_hud_only_raises_it() {
        let mut c = Controller::new(0);
        let hidden = now(LINGER_MS + 100);
        assert!(!c.visible(&hidden));
        c.key(HudKey::Left, KeyEdge::Down, &hidden);
        c.key(HudKey::Left, KeyEdge::Up, &now(LINGER_MS + 150));
        assert!(!c.scrub.live(), "nothing travelled, nothing is committed");
        assert!(c.visible(&now(LINGER_MS + 200)));
        assert_eq!(c.tick(&now(LINGER_MS + 2000)), None);
    }

    /// A HELD LEFT/RIGHT accelerates (a tap stays ten seconds: fine-tuning is unchanged): ten
    /// seconds a hop at first, thirty after [`HOLD_STEPS`]' first threshold, sixty after the second.
    #[test]
    fn holding_accelerates_and_commits_once_on_release() {
        let mut c = Controller::new(0);
        c.key(HudKey::Right, KeyEdge::Down, &now(0));
        assert_eq!(c.scrub.preview, Some(610_000), "the press hops");
        let mut t = 500;
        c.key(HudKey::Right, KeyEdge::Repeat, &now(t));
        let mut last = c.shown_pos(&now(t));
        let mut hops = Vec::new();
        let mut acts = Vec::new();
        while t < 4500 {
            t += 40; // the remote's repeat rate does not set the pace
            c.key(HudKey::Right, KeyEdge::Repeat, &now(t));
            acts.push(c.tick(&now(t)));
            let p = c.shown_pos(&now(t));
            if p != last {
                hops.push(p - last);
            }
            last = p;
        }
        assert!(acts.iter().all(Option::is_none), "nothing seeks while the key is held");
        assert_eq!(hops.first(), Some(&STEP_MS), "a hold starts at the tap's step: {hops:?}");
        assert!(hops.windows(2).all(|w| w[0] <= w[1]), "the steps only grow: {hops:?}");
        assert!(hops.contains(&30_000) && hops.last() == Some(&60_000), "ten, thirty, then sixty seconds: {hops:?}");
        assert_eq!(hops.len(), 4000 / HOLD_HOP_MS as usize, "a hop every {HOLD_HOP_MS} ms");
        assert_eq!(c.scrub.hold_step(t), Some(60_000), "the step the time bubble names");
        c.key(HudKey::Right, KeyEdge::Up, &now(t + 10));
        assert_eq!(c.tick(&now(t + 10 + RELEASE_DEBOUNCE_MS - 1)), None, "a release waits out the debounce");
        assert_eq!(c.tick(&now(t + 10 + RELEASE_DEBOUNCE_MS)), Some(Action::Seek(last)), "one seek, on release");
        assert_eq!(c.tick(&now(t + 2000)), None);
    }

    /// A burst's taps add the same step each and wait ~700 ms of quiet before their one seek.
    #[test]
    fn a_burst_accumulates_and_commits_once_after_its_quiet() {
        let mut c = Controller::new(0);
        let mut shown = Vec::new();
        for i in 0..5 {
            c.key(HudKey::Right, KeyEdge::Down, &now(i * 300));
            c.key(HudKey::Right, KeyEdge::Up, &now(i * 300 + 50));
            shown.push(c.shown_pos(&now(i * 300 + 60)));
            assert_eq!(c.tick(&now(i * 300 + 299)), None, "taps 300 ms apart are one burst");
        }
        assert_eq!(shown, [610_000, 620_000, 630_000, 640_000, 650_000]);
        c.key(HudKey::Left, KeyEdge::Down, &now(1500));
        c.key(HudKey::Left, KeyEdge::Up, &now(1550));
        assert_eq!(c.tick(&now(1550 + TAP_COMMIT_MS - 1)), None);
        assert_eq!(c.tick(&now(1550 + TAP_COMMIT_MS)), Some(Action::Seek(640_000)), "one seek for the burst");
        assert_eq!(c.tick(&now(5000)), None);
    }

    /// ⏩ doubles its rate each press while the picture is held; OK seeks there and plays, BACK
    /// cancels and resumes.
    #[test]
    fn the_speed_scrub_doubles_holds_the_picture_and_commits_on_ok() {
        let mut c = Controller::new(0);
        assert_eq!(c.key(HudKey::FastForward, KeyEdge::Down, &now(0)), Some(Action::SetPaused(true)), "the picture is held");
        assert_eq!(c.scrub.speed(), Some((true, 2)));
        let paused = |ms| Now { paused: true, ..now(ms) };
        assert_eq!(c.key(HudKey::FastForward, KeyEdge::Down, &paused(100)), None, "already held");
        c.key(HudKey::FastForward, KeyEdge::Down, &paused(200));
        assert_eq!(c.scrub.speed(), Some((true, 8)));
        c.tick(&paused(200));
        assert_eq!(c.tick(&paused(1200)), None, "it never commits by itself");
        assert_eq!(c.shown_pos(&paused(1200)), 600_000 + 8_000, "×8 for a second: eight seconds on");
        for _ in 0..5 {
            c.key(HudKey::FastForward, KeyEdge::Down, &paused(1300));
        }
        assert_eq!(c.scrub.speed(), Some((true, 32)), "it tops out");
        c.key(HudKey::Rewind, KeyEdge::Down, &paused(1400));
        assert_eq!(c.scrub.speed(), Some((false, 2)), "the other way starts over");
        let target = c.scrub.preview.unwrap();
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &paused(1400)), Some(Action::SeekAndPlay(target)));
        assert!(!c.scrub.live());
        // BACK cancels and lets the film play again
        c.key(HudKey::Rewind, KeyEdge::Down, &now(2000));
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &paused(2100)), Some(Action::SetPaused(false)));
        assert!(!c.scrub.live());
        // a film paused by the viewer stays paused on BACK
        c.key(HudKey::FastForward, KeyEdge::Down, &paused(3000));
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &paused(3100)), None);
        let live = Now { dur: 0, ..now(4000) };
        assert_eq!(c.key(HudKey::FastForward, KeyEdge::Down, &live), None);
        assert!(!c.scrub.live(), "nothing to scrub on a live stream");
    }

    /// `3` then `5` is one seek, to 50 %; `0` is the start; CH ▲/▼ jump ten minutes, together.
    #[test]
    fn number_keys_and_ch_jump_with_the_commit_delay() {
        let mut c = Controller::new(0);
        c.key(HudKey::Digit(3), KeyEdge::Down, &now(0));
        assert_eq!(c.scrub.preview, Some(1_080_000));
        c.key(HudKey::Digit(5), KeyEdge::Down, &now(300));
        assert_eq!(c.tick(&now(300 + TAP_COMMIT_MS - 1)), None, "only the last counts");
        assert_eq!(c.tick(&now(300 + TAP_COMMIT_MS)), Some(Action::Seek(1_800_000)));
        c.key(HudKey::Digit(0), KeyEdge::Down, &now(5000));
        assert_eq!(c.tick(&now(5000 + TAP_COMMIT_MS)), Some(Action::Seek(0)));
        c.key(HudKey::ChUp, KeyEdge::Down, &now(9000));
        c.key(HudKey::ChUp, KeyEdge::Down, &now(9200));
        c.key(HudKey::ChDown, KeyEdge::Down, &now(9400));
        assert_eq!(c.tick(&now(9400 + TAP_COMMIT_MS)), Some(Action::Seek(600_000 + JUMP_MS)), "+10 +10 -10 min");
        assert_eq!(c.key(HudKey::Digit(5), KeyEdge::Repeat, &now(12_000)), None);
        assert!(!c.scrub.live(), "a repeat is not a press");
        let live = Now { dur: 0, ..now(13_000) };
        c.key(HudKey::Digit(5), KeyEdge::Down, &live);
        c.key(HudKey::ChUp, KeyEdge::Down, &live);
        assert!(!c.scrub.live(), "a live stream has nothing to jump in");
    }

    /// With chapters, CH ▲/▼ walk them: on to the next start, back to this one's start, and to the
    /// one before when pressed again (or right after a start); nothing past the last.
    #[test]
    fn ch_walks_the_chapters_when_the_file_has_them() {
        let ch = [0, 90_000, 600_000, 1_500_000];
        assert_eq!(chapter_jump(&ch, 100_000, true), Some(600_000));
        assert_eq!(chapter_jump(&ch, 100_000, false), Some(90_000));
        assert_eq!(chapter_jump(&ch, 91_000, false), Some(0), "just after a start: the one before");
        assert_eq!(chapter_jump(&ch, 1_600_000, true), None);
        assert_eq!(chapter_jump(&[0], 100_000, true), Some(100_000 + JUMP_MS as i64), "no chapters: ten minutes");
        let mut c = Controller::new(0);
        c.chapters = ch.to_vec();
        let at = |ms| Now { pos: 100_000, ..now(ms) };
        c.key(HudKey::ChUp, KeyEdge::Down, &at(1000));
        c.key(HudKey::ChUp, KeyEdge::Down, &at(1200));
        assert_eq!(c.tick(&at(1200 + TAP_COMMIT_MS)), Some(Action::Seek(1_500_000)), "two chapters on");
        c.key(HudKey::ChDown, KeyEdge::Down, &at(5000));
        c.key(HudKey::ChDown, KeyEdge::Down, &at(5200));
        assert_eq!(c.tick(&at(5200 + TAP_COMMIT_MS)), Some(Action::Seek(0)), "this one's start, then the one before");
    }

    /// The Episodes pill stands first in the row: LEFT from the first disc reaches it, OK opens the
    /// panel, and it gives way to Up Next.
    #[test]
    fn the_episodes_pill_leads_the_row_and_opens_the_panel() {
        let mut c = Controller::new(0);
        let ep = |ms| Now { episodes: true, ..now(ms) };
        c.key(HudKey::Up, KeyEdge::Down, &ep(10));
        assert_eq!(c.hud.focus, Focus::Disc(0));
        c.key(HudKey::Left, KeyEdge::Down, &ep(20));
        assert_eq!(c.hud.focus, Focus::Episodes);
        c.key(HudKey::Left, KeyEdge::Down, &ep(30));
        assert_eq!(c.hud.focus, Focus::Episodes, "the row clamps at its start");
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &ep(40)), Some(Action::OpenEpisodes));
        c.key(HudKey::Right, KeyEdge::Down, &ep(50));
        assert_eq!(c.hud.focus, Focus::Disc(0));
        assert_eq!(c.click(Focus::Episodes, &ep(60)), Some(Action::OpenEpisodes));
        c.tick(&now(70));
        assert_eq!(c.hud.focus, Focus::Disc(0), "no episodes, no pill");
        let mut c = Controller::new(0);
        c.key(HudKey::Up, KeyEdge::Down, &now(10));
        c.key(HudKey::Left, KeyEdge::Down, &now(20));
        assert_eq!(c.hud.focus, Focus::Disc(0), "a movie has none");
    }

    #[test]
    fn a_bounced_key_up_mid_hold_is_the_same_hold_and_a_lost_one_still_commits() {
        let mut c = Controller::new(0);
        c.key(HudKey::Left, KeyEdge::Down, &now(0));
        c.key(HudKey::Left, KeyEdge::Repeat, &now(500));
        c.tick(&now(500));
        c.key(HudKey::Left, KeyEdge::Up, &now(600));
        let before = c.scrub.preview;
        c.key(HudKey::Left, KeyEdge::Down, &now(610));
        assert!(c.scrub.holding(), "the driver's bounce did not end the hold");
        assert_eq!(c.scrub.preview, before, "and did not hop");
        assert_eq!(c.tick(&now(700)), None);
        // the repeats stop and no key-up ever comes
        assert_eq!(c.tick(&now(610 + LOST_KEYUP_MS)), None);
        let committed = c.tick(&now(610 + LOST_KEYUP_MS + 1));
        assert!(matches!(committed, Some(Action::Seek(_))), "a lost key-up commits: {committed:?}");
    }

    #[test]
    fn a_scrub_never_lands_in_the_last_seconds_or_before_zero() {
        let mut c = Controller::new(0);
        let end = Now { pos: 3_595_000, ..now(0) };
        c.key(HudKey::Right, KeyEdge::Down, &end);
        assert_eq!(c.scrub.preview, Some(3_600_000 - END_GUARD_MS));
        c.scrub.cancel();
        let start = Now { pos: 4_000, ..now(0) };
        c.key(HudKey::Left, KeyEdge::Down, &start);
        assert_eq!(c.scrub.preview, Some(0));
    }

    #[test]
    fn back_cancels_a_live_scrub_then_undoes_a_committed_one() {
        let mut c = Controller::new(0);
        c.key(HudKey::Right, KeyEdge::Down, &now(0));
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &now(10)), None);
        assert!(!c.scrub.live(), "BACK drops the preview");
        assert!(c.visible(&now(20)), "and keeps the HUD");
        c.key(HudKey::Right, KeyEdge::Down, &now(100));
        c.key(HudKey::Right, KeyEdge::Up, &now(110));
        assert_eq!(c.tick(&now(110 + TAP_COMMIT_MS)), Some(Action::Seek(610_000)));
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &now(1000)), Some(Action::Seek(600_000)), "BACK returns where it started");
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &now(1100)), None, "once: then BACK hides the HUD");
        // the window closes
        let mut c = Controller::new(0);
        c.key(HudKey::Right, KeyEdge::Down, &now(100));
        c.key(HudKey::Right, KeyEdge::Up, &now(110));
        c.tick(&now(110 + TAP_COMMIT_MS));
        c.tick(&now(110 + TAP_COMMIT_MS + UNDO_MS + 1));
        assert!(c.undo.is_none());
    }

    #[test]
    fn a_paused_scrub_waits_for_ok_which_seeks_and_plays() {
        let mut c = Controller::new(0);
        let paused = Now { paused: true, ..now(0) };
        c.key(HudKey::Right, KeyEdge::Down, &paused);
        c.key(HudKey::Right, KeyEdge::Up, &Now { ms: 50, ..paused });
        assert_eq!(c.tick(&Now { ms: 5000, ..paused }), None, "no commit on a timer while paused");
        assert!(c.scrub.waiting());
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &Now { ms: 5100, ..paused }), Some(Action::SeekAndPlay(610_000)));
        assert!(c.undo.is_none(), "a paused film's seek has no undo");
        c.key(HudKey::Left, KeyEdge::Down, &Now { ms: 6000, ..paused });
        c.key(HudKey::Left, KeyEdge::Up, &Now { ms: 6050, ..paused });
        assert_eq!(c.key(HudKey::Play, KeyEdge::Down, &Now { ms: 6100, ..paused }), Some(Action::SeekAndPlay(590_000)));
    }

    #[test]
    fn a_pointer_drag_previews_and_commits_on_release() {
        let mut c = Controller::new(0);
        c.drag(1_200_000, &now(10));
        c.drag(1_800_000, &now(20));
        assert_eq!(c.shown_pos(&now(30)), 1_800_000);
        assert_eq!(c.tick(&now(2000)), None, "a drag commits on release only");
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Up, &now(2100)), Some(Action::Seek(1_800_000)));
        assert!(c.undo.is_some());
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Up, &now(2200)), None, "a release with no drag is nothing");
        // controls answer the pointer
        assert_eq!(c.click(Focus::Disc(DISC_SUBTITLES), &now(3000)), Some(Action::OpenMenu(DISC_SUBTITLES)));
        c.hover(Some(Focus::Disc(DISC_AUDIO)), &now(3100));
        assert_eq!(c.hud.focus, Focus::Disc(DISC_AUDIO));
    }

    #[test]
    fn up_next_takes_the_row_near_an_episodes_end() {
        assert!(!up_next_offered(true, false, 1_000_000, 3_600_000, false, None, UP_NEXT_LEAD_MS));
        assert!(up_next_offered(true, false, 3_600_000 - UP_NEXT_LEAD_MS, 3_600_000, false, None, UP_NEXT_LEAD_MS));
        assert!(!up_next_offered(false, false, 3_599_000, 3_600_000, false, None, UP_NEXT_LEAD_MS), "no next episode");
        assert!(!up_next_offered(true, true, 3_599_000, 3_600_000, false, None, UP_NEXT_LEAD_MS), "declined");
        assert!(!up_next_offered(true, false, 3_599_000, 3_600_000, true, None, UP_NEXT_LEAD_MS), "over");
        assert!(!up_next_offered(true, false, 0, 0, false, None, UP_NEXT_LEAD_MS), "an unknown length");
        let mut c = Controller::new(0);
        let offered = Now { offered: true, ..now(LINGER_MS + 100) };
        assert!(!c.visible(&offered));
        c.tick(&offered);
        assert!(c.visible(&offered), "the offer raises the HUD");
        assert_eq!(c.hud.focus, Focus::UpNext(UP_NEXT_PLAY), "on its primary");
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &offered), Some(Action::UpNext(UP_NEXT_PLAY)));
        c.key(HudKey::Left, KeyEdge::Down, &offered);
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &offered), Some(Action::UpNext(UP_NEXT_CREDITS)));
        c.tick(&Now { offered: false, ..offered });
        assert_eq!(c.hud.focus, Focus::Scrub, "the offer gone, the cursor goes home");
    }

    #[test]
    fn up_next_follows_the_viewers_lead() {
        let d = 3_600_000;
        assert!(!up_next_offered(true, false, d - 20_000, d, false, None, 15_000), "15 s: not yet at 20 s left");
        assert!(up_next_offered(true, false, d - 15_000, d, false, None, 15_000));
        assert!(up_next_offered(true, false, d - 45_000, d, false, None, 45_000));
        assert!(!up_next_offered(true, false, 10_000, 20_000, false, None, 45_000), "shorter than the lead: never");
    }

    #[test]
    fn up_next_follows_the_credits_marker() {
        let d = 3_600_000;
        assert!(up_next_offered(true, false, 3_000_000, d, false, Some(3_000_000), UP_NEXT_LEAD_MS), "from the credits' start");
        assert!(!up_next_offered(true, false, 2_999_999, d, false, Some(3_000_000), UP_NEXT_LEAD_MS));
        assert!(!up_next_offered(true, false, 3_000_000, d, false, None, UP_NEXT_LEAD_MS), "without a marker, the last 35 s");
        assert!(up_next_offered(true, false, d - UP_NEXT_LEAD_MS, d, false, Some(d + 1), UP_NEXT_LEAD_MS), "a marker past the end is no marker");
    }

    const INTRO: Segment = Segment { kind: SegKind::Intro, span: Span { start_ms: 60_000, end_ms: 120_000 } };

    fn inside(ms: u32) -> Now {
        Now { pos: 70_000, skip: Some(INTRO), ..now(ms) }
    }

    #[test]
    fn the_skip_button_takes_the_cursor_over_a_hidden_hud_and_ok_skips_with_an_undo() {
        let mut c = Controller::new(0);
        let t = LINGER_MS + 100;
        assert_eq!(c.tick(&inside(t)), None);
        assert_eq!(c.skip_shown(&inside(t)), Some(INTRO));
        assert_eq!(c.hud.focus, Focus::Skip, "alone on the picture, it holds the cursor");
        assert!(!c.visible(&inside(t)), "the HUD stays down");
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &inside(t + 100)), Some(Action::SkipTo(120_500)), "one OK: past the end");
        assert_eq!(c.hud.focus, Focus::Scrub);
        assert_eq!(c.key(HudKey::Back, KeyEdge::Down, &inside(t + 1000)), Some(Action::Seek(70_000)), "BACK undoes it");
    }

    #[test]
    fn the_skip_button_goes_after_its_time_or_its_segment_and_any_key_brings_it_back() {
        let mut c = Controller::new(0);
        let t = LINGER_MS + 100;
        c.tick(&inside(t));
        c.tick(&inside(t + SKIP_SHOW_MS - 1));
        assert!(c.skip_shown(&inside(t + SKIP_SHOW_MS - 1)).is_some());
        c.tick(&inside(t + SKIP_SHOW_MS));
        assert_eq!(c.skip_shown(&inside(t + SKIP_SHOW_MS)), None, "gone after its time");
        assert_eq!(c.hud.focus, Focus::Scrub);
        // a key brings the HUD and the button back, the cursor on it
        let k = t + SKIP_SHOW_MS + 500;
        c.key(HudKey::Down, KeyEdge::Down, &inside(k));
        assert_eq!(c.skip_shown(&inside(k)), Some(INTRO));
        assert_eq!(c.hud.focus, Focus::Skip);
        // it stands above the row: DOWN to the row, UP back to it
        c.key(HudKey::Down, KeyEdge::Down, &inside(k + 10));
        assert_eq!(c.hud.focus, Focus::Disc(DISC_MORE));
        c.key(HudKey::Up, KeyEdge::Down, &inside(k + 20));
        assert_eq!(c.hud.focus, Focus::Skip);
        // the segment over, it goes
        let past = Now { pos: 120_000, skip: None, ..now(k + 30) };
        c.tick(&past);
        assert_eq!(c.skip_shown(&past), None);
        assert_eq!(c.hud.focus, Focus::Scrub);
        // the pointer
        let mut c = Controller::new(0);
        c.tick(&inside(10));
        assert_eq!(c.click(Focus::Skip, &inside(20)), Some(Action::SkipTo(120_500)));
        // over a visible HUD it appears without taking the cursor
        let mut c = Controller::new(0);
        c.tick(&inside(10));
        assert!(c.skip_shown(&inside(10)).is_some());
        assert_eq!(c.hud.focus, Focus::Scrub);
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Down, &inside(20)), Some(Action::TogglePause));
    }

    #[test]
    fn automatic_skips_once_per_segment_and_never_where_the_viewer_seeked() {
        let auto = |ms| Now { auto_skip: true, ..inside(ms) };
        let mut c = Controller::new(0);
        assert_eq!(c.tick(&auto(100)), Some(Action::SkipTo(120_500)));
        assert_eq!(c.note(&auto(200)), Some(SegKind::Intro), "the note says so");
        assert_eq!(c.note(&auto(100 + SKIPPED_NOTE_MS)), None);
        assert!(c.undo.is_none(), "an automatic skip is no scrub to undo");
        assert_eq!(c.tick(&auto(300)), None, "the playhead not yet moved: once");
        // the viewer scrubs back into the intro: it plays, the button offered instead
        c.key(HudKey::Left, KeyEdge::Down, &Now { pos: 125_000, auto_skip: true, ..now(1000) });
        c.key(HudKey::Left, KeyEdge::Up, &Now { pos: 125_000, auto_skip: true, ..now(1050) });
        assert_eq!(c.tick(&Now { pos: 125_000, auto_skip: true, ..now(1050 + TAP_COMMIT_MS) }), Some(Action::Seek(115_000)));
        let back = |ms| Now { pos: 115_000, ..auto(ms) };
        assert_eq!(c.tick(&back(2000)), None);
        assert!(c.skip_shown(&back(2000)).is_some(), "the button still offers it");
        // a seek that lands inside a segment never skipped is respected too
        let mut c = Controller::new(0);
        c.drag(80_000, &Now { pos: 10_000, auto_skip: true, ..now(10) });
        assert_eq!(c.key(HudKey::Ok, KeyEdge::Up, &Now { pos: 10_000, auto_skip: true, ..now(20) }), Some(Action::Seek(80_000)));
        assert_eq!(c.tick(&Now { pos: 80_000, ..auto(30) }), None, "landed inside by hand");
        // and a recap is its own segment
        let recap = Segment { kind: SegKind::Recap, span: Span { start_ms: 0, end_ms: 50_000 } };
        let mut c = Controller::new(0);
        assert_eq!(c.tick(&Now { pos: 1000, skip: Some(recap), auto_skip: true, ..now(10) }), Some(Action::SkipTo(50_500)));
        assert_eq!(c.tick(&auto(20)), Some(Action::SkipTo(120_500)));
    }
}
