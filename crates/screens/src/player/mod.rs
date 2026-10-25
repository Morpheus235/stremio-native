//! **The player page** — the playing video, which is the television's hardware plane composited
//! UNDER this surface, and the transport HUD over it. The page punches its hole (a transparent
//! clear), draws the subtitles (the application's layer, [`install_subtitle_painter`]), then the
//! HUD while it is up, then the read-out the playback's phase calls for: the working read-out
//! before the first picture, the failure read-out when it fails.
//!
//! **The HUD** (PlxNative's transport, [`hud`]): the title block (an episode's `S1 E5 Title` over
//! the show's name, or the movie's name), the scrubber with its playhead, the elapsed and
//! remaining clocks and the state mark beside the elapsed one (a pause glyph while paused, a
//! spinner while a seek resolves), and the control row on the right — for an episode the Episodes
//! pill, then the Subtitles, Audio and `…` discs, or near an episode's end the Up Next pair
//! standing in for them. It auto-hides after [`hud::LINGER_MS`] (paused or not) and any key or
//! pointer motion brings it back; it fades in and out ([`HUD_FADE_S`]) over a front-loaded scrim.
//! The scrubber is `sn_ui::widgets::Scrubber`: the buffered range, the played part in the accent,
//! chapter cuts, a playhead that grows with focus and a scrub, and a time bubble while scrubbing
//! (with a held key's step). A playback that resumed a saved position offers *Start over* in the
//! Skip slot for [`START_OVER_MS`]; a stall with the picture up shows the centre buffering plate
//! after [`STALL_PLATE_MS`]. LEFT/RIGHT scrub (a tap hops ten seconds, a hold hops in growing
//! steps, [`hud::HOLD_STEPS`]), OK plays/pauses on the bar and opens what the cursor is on in the
//! row, BACK cancels a scrub, undoes a committed one, hides the HUD, and then leaves.
//!
//! **The centre of the picture** says what the transport is doing without the HUD: while paused,
//! a rounded badge with the pause glyph, *Paused* and the position under it, over a gentle dim
//! (drawn UNDER the subtitles, so captions stay crisp, and never an opaque block); it eases in and
//! out ([`BADGE_IN_S`]) and a play glyph flashes on resume. While a scrub is live the same place
//! shows its target time large, with the file's chapter there under it (`MediaFacts::chapter_at`)
//! and how far it moves.
//!
//! **The discs are toggles.** A short OK on the Subtitles disc turns subtitles on or off (GREEN's
//! press, with the same note); held ([`hud::DISC_HOLD_MS`]) it opens the subtitle menu. The audio
//! and `…` discs open their menus on OK. The disc whose menu is open, and the Subtitles disc while
//! subtitles show, wear a dot under the glyph (`TransportButton::lit`, [`PlayerScreen::disc_lit`]);
//! the key that opened a menu closes it (GREEN the subtitle menu, RED the audio menu, YELLOW the
//! Episodes panel, BLUE — or a film's YELLOW — the show info), as BACK does.
//!
//! **The menus** are the shared popover ([`crate::menu`], [`tracks`] builds the rows): the audio
//! and subtitle tracks the stream carries, the `…` menu (*Show info* — [`show_info`] —, the
//! subtitle size and position — the Settings preferences —, this playback's aspect ratio, and last
//! the Stats for nerds switch, which the application's read-out draws over the page), each rising
//! from the control row. The Episodes pill opens [`episodes`]: the show's seasons and episodes,
//! where OK plays another episode the way Up Next plays the next one.
//!
//! **Up Next** is the next episode of the series from its meta's videos
//! (`sn_stremio::library::next_episode`): the page holds the show's meta open while it plays an
//! episode, and near its end offers *Next episode* and *Watch credits*: from the credits' start
//! when the episode's credits are marked (below), else [`hud::UP_NEXT_LEAD_MS`] before the end —
//! or a fixed 15 / 25 / 35 / 45 s (Settings > Up Next appears, `Pref::UpNextAt`). An episode that
//! plays to its end goes to the next one too, unless Settings > Auto-play next episode is off
//! (`Pref::AutoPlay`: Up Next is still offered, but only a press plays it); anything else that
//! ends returns to where it was played from. **Same release** (Settings > Next episode, the default): when the offer
//! appears the page asks for the next episode's streams, and the next episode plays the same
//! addon's stream of the release that is playing (`sn_stremio::stream_facts::same_release`: the
//! `bingeGroup`, else the facts) straight away, over that episode's Streams page, the Up Next
//! line saying so once it is found. No match, a failed or slow addon ([`PICK_WAIT_MS`]), or
//! *Always choose* opens the Streams page instead (a stream is the viewer's to pick).
//!
//! **Skip intro / Skip recap**: the playback's markers (`PlaybackView::skips`, which the
//! application looks up from introdb.app or AniSkip after the first picture,
//! `sn_stremio::skip`) put a Skip button above the control row while the playhead is inside an
//! intro or recap ([`hud`]'s Skip). Settings > Skip intros turns it off, or makes it automatic
//! (with a short note saying what was skipped).
//!
//! **The Magic Remote**: every control registers a stop, the cursor follows the pointer over
//! them, a click activates, and a press or drag on the bar scrubs (committing on the release).
//!
//! The page names no media type: what it shows arrives as [`PlaybackView`] and what it asks for
//! leaves as [`PlayerCmd`]. Leaving the page by any road ends the playback (`Unmount`).
//!
//! [`PlaybackView`]: crate::registry::PlaybackView

pub mod episodes;
pub mod hud;
pub mod show_info;
pub mod subtitle_menu;
pub mod tracks;

use std::borrow::Cow;
use std::ffi::CString;
use std::sync::OnceLock;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputEvent, InputKind, Leave, LogicalState, Machine,
    Measure, NavOp,
};
use sn_machine::present::{PresentEvent, Provenance};
use sn_platform::i18n::msg;
use sn_ui::consts::{classify_input, Key as UiKey, MARGIN_X, SCR_H, SCR_W};
use sn_ui::label::{HAlign, Label, VAlign};
use sn_ui::icons::Icon;
use sn_ui::screen::{
    Activate, At, Dir, DrawFrame, FocusSource, Focusable, GroupSpec, HitSource, Hover, Placed, RenderStrategy, Screen,
    ScreenEvent, Step, Stop,
};
use sn_ui::widgets::{Button, ControlGround, CtlPop, Spinner, StatusKind, StatusOverlay, TransportButton};
use sn_ui::{theme, Env, Painter, Rect, View};

use crate::registry::{
    word, AppArg, AppFx, AppMsg, Bundle, ChoiceTarget, DetailCmd, MenuArg, MenuKind, PlayItem, PlayPhase, PlayRequest, PlayerArg,
    PlayerCmd, PlayerPanel, SettingsCmd, VideoRef,
};
use crate::stores::detail::{MetaKey, MetaState, StreamKey};
use crate::stores::settings::{Pref, NEXT_SAME, SKIP_AUTO, SKIP_OFF};
use crate::stores::StoreId;
use hud::{Action, Controller, Focus, HudKey, KeyEdge, Now};
use sn_stremio::skip::SegKind;

/// **The application's subtitle layer**: drawn first on the page, under the HUD, every frame.
/// `hud_up` lifts captions clear of the transport. The screens cannot name the media layer, so
/// the application installs its painter at boot; with none installed (the host tests) nothing
/// is drawn.
pub type SubtitlePainter = fn(hud_up: bool, measure: &dyn Measure);

static SUBTITLES: OnceLock<SubtitlePainter> = OnceLock::new();

/// Install the subtitle layer (once; later calls are ignored).
pub fn install_subtitle_painter(f: SubtitlePainter) {
    let _ = SUBTITLES.set(f);
}

// ---- geometry (PlxNative's transport, at 1080p) ------------------------------------------------

/// The now-playing title is a HUD *display* title, deliberately above `theme::size::TITLE`: the
/// documented carve-out of the type scale (`ui/CLAUDE.md`), named here rather than left bare.
const HUD_TITLE_SZ: i32 = 54;
/// The transport's dark ground: transparent at its top, front-loaded — it reaches
/// [`SCRIM_KNEE_A`] a third of the way down (where the title block and the Up Next line sit, so
/// they read over a bright picture) and [`SCRIM_FOOT_A`] at the bottom.
const SCRIM_H: f32 = 470.0;
const SCRIM_KNEE: f32 = 0.32;
const SCRIM_KNEE_A: f32 = 0.58;
const SCRIM_FOOT_A: f32 = 0.88;
/// How long the HUD takes to fade in or out.
pub const HUD_FADE_S: f32 = 0.15;
/// How long the scrubber's knob takes to grow or settle as focus and a scrub come and go.
const BAR_EASE_S: f32 = 0.12;
/// A stall with the picture up shows the centre buffering plate after this long (a shorter one
/// is the clock's own spinner).
pub const STALL_PLATE_MS: u32 = 1000;
/// How long *Start over* is offered after a playback resumed a saved position.
pub const START_OVER_MS: u32 = 8000;
const SB_X: f32 = MARGIN_X;
const SB_W: f32 = SCR_W - 2.0 * MARGIN_X;
const SB_Y: f32 = SCR_H - 198.0;
const SB_H: f32 = 8.0;
/// The clocks' line under the bar.
const CLOCK_Y: f32 = SB_Y + 30.0;
const KICKER_Y: f32 = SCR_H - 312.0;
const TITLE_Y: f32 = SCR_H - 278.0;
/// A control disc's diameter and the air between two.
const BTN_S: f32 = 64.0;
const BTN_GAP: f32 = 22.0;
/// The control row's top: the media layer's `text_style::HUD_CONTROLS_TOP_Y`, which captions
/// rise clear of (the application's tests hold the two equal).
pub const CTRL_Y: f32 = SCR_H - 288.0;
/// The control row's right edge: the app's side margin, which the player menus share.
const CTRL_RIGHT: f32 = SCR_W - MARGIN_X;
/// The Up Next pair's type, and its caption's line above the row.
const UP_NEXT_SZ: i32 = theme::size::LABEL;
const UP_NEXT_CAPTION_Y: f32 = CTRL_Y - 40.0;
/// The clear air between the title block and the control row.
const TITLE_GAP: f32 = 40.0;
/// How far the bar's pointer target reaches above and below the 8 px bar.
const SB_HIT_PAD: f32 = 22.0;

/// The pointer's stops.
pub const ELEM_SCRUB: u32 = 0;
pub const ELEM_DISC: u32 = 10;
pub const ELEM_UP_NEXT: u32 = 20;
pub const ELEM_SKIP: u32 = 30;
pub const ELEM_EPISODES: u32 = 40;
/// The failure read-out's actions: *Choose another stream*, then *Try again*.
pub const ELEM_FAIL: u32 = 50;
const FAIL_ACTIONS: usize = 2;

/// The centre badge: its disc's diameter and its glyph's, centred a little above the middle of
/// the picture (the captions live at the bottom).
const BADGE_D: f32 = 144.0;
const BADGE_GLYPH: f32 = 60.0;
const BADGE_CY: f32 = SCR_H * 0.5 - 60.0;
/// The badge's ground and the dim under it, as scrim weights (translucent: the picture and the
/// captions read through).
const BADGE_GROUND_A: f32 = 0.55;
const PAUSE_DIM_A: f32 = 0.28;
/// How long GREEN is held before it opens the subtitle menu instead of toggling subtitles.
pub const GREEN_HOLD_MS: u32 = 500;
/// How long the subtitle toggle's note stands, and how long it waits for the playback to say what
/// the toggle landed on.
pub const SUB_NOTE_MS: u32 = 2500;
const SUB_NOTE_WAIT_MS: u32 = 1500;

/// The player's own keys, read before the shared classifier: the colour keys (which mean
/// something else on every other page), the number keys and CH ▲/▼, and ⏪/⏩ by their own codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerKey {
    /// RED: the audio menu.
    Red,
    /// GREEN: a press toggles subtitles, a hold ([`GREEN_HOLD_MS`]) opens the subtitle menu.
    Green,
    /// YELLOW: the Episodes panel (an episode); the show info for anything else.
    Yellow,
    /// BLUE: the show info.
    Blue,
    Hud(HudKey),
}

/// The player's reading of a raw `(sym, wcode)` press, `None` for the shared classifier's keys.
pub fn player_key(sym: u32, wcode: u32) -> Option<PlayerKey> {
    use crate::tabs::{WCODE_BLUE, WCODE_CH_DOWN, WCODE_CH_UP, WCODE_GREEN, WCODE_RED, WCODE_YELLOW};
    use sn_ui::consts::{WCODE_CH_DOWN_KEY, WCODE_CH_UP_KEY, WCODE_FASTFORWARD, WCODE_REWIND};
    Some(match wcode {
        WCODE_RED => PlayerKey::Red,
        WCODE_GREEN => PlayerKey::Green,
        WCODE_YELLOW => PlayerKey::Yellow,
        WCODE_BLUE => PlayerKey::Blue,
        WCODE_CH_UP | WCODE_CH_UP_KEY => PlayerKey::Hud(HudKey::ChUp),
        WCODE_CH_DOWN | WCODE_CH_DOWN_KEY => PlayerKey::Hud(HudKey::ChDown),
        _ if sym == WCODE_REWIND || wcode == WCODE_REWIND => PlayerKey::Hud(HudKey::Rewind),
        _ if sym == WCODE_FASTFORWARD || wcode == WCODE_FASTFORWARD => PlayerKey::Hud(HudKey::FastForward),
        // a number key arrives with its ASCII code in `sym` (`sn_ui::consts::is_bound`)
        _ if (u32::from(b'0')..=u32::from(b'9')).contains(&sym) => PlayerKey::Hud(HudKey::Digit((sym - u32::from(b'0')) as u8)),
        _ => return None,
    })
}

/// How long the badge takes to ease in or out, and the resume's play glyph to flash.
pub const BADGE_IN_S: f32 = 0.18;
pub const FLASH_S: f32 = 0.5;

/// The Skip button's (and the automatic skip's note's) top: above the control row and the Up
/// Next caption, right-aligned on the row's edge.
const SKIP_Y: f32 = CTRL_Y - BTN_S - 2.0 * BTN_GAP;

/// The Skip button's label for a segment.
pub fn skip_label(kind: SegKind) -> &'static str {
    match kind {
        SegKind::Recap => msg::app_player_skip_recap(),
        _ => msg::app_player_skip_intro(),
    }
}

/// The automatic skip's note.
pub fn skipped_label(kind: SegKind) -> &'static str {
    match kind {
        SegKind::Recap => msg::app_player_skipped_recap(),
        _ => msg::app_player_skipped_intro(),
    }
}

/// The Skip button's rect (the note's, with its check), right-aligned on the row's edge.
pub fn skip_rect(label: &str, icon: bool, measure: &dyn Measure) -> Rect {
    let c = CString::new(label).unwrap_or_default();
    let w = Button::pill_w_measured(&c, UP_NEXT_SZ, icon, false, measure);
    Rect::new(CTRL_RIGHT - w, SKIP_Y, w, BTN_S)
}

/// The Episodes pill's rect: before the discs, in the control row.
/// The Episodes button: a disc like the transport's, its icon only, left of the first disc.
pub fn episodes_rect(_measure: &dyn Measure) -> Rect {
    Rect::new(disc_rect(0).x - BTN_GAP - BTN_S, CTRL_Y, BTN_S, BTN_S)
}

fn disc_rect(i: u8) -> Rect {
    let x = CTRL_RIGHT - BTN_S - f32::from(hud::DISCS - 1 - i) * (BTN_S + BTN_GAP);
    Rect::new(x, CTRL_Y, BTN_S, BTN_S)
}

fn scrub_rect() -> Rect {
    Rect::new(SB_X, SB_Y - SB_HIT_PAD, SB_W, SB_H + 2.0 * SB_HIT_PAD)
}

/// The Up Next pair's rects (Watch credits, Next episode), right-anchored on the row's edge.
pub fn up_next_rects(measure: &dyn Measure) -> [Rect; 2] {
    let w = |s: &str| {
        let c = CString::new(s).unwrap_or_default();
        Button::pill_w_measured(&c, UP_NEXT_SZ, false, false, measure)
    };
    let next_w = w(msg::app_player_next_episode());
    let credits_w = w(msg::app_player_watch_credits());
    let next = Rect::new(CTRL_RIGHT - next_w, CTRL_Y, next_w, BTN_S);
    let credits = Rect::new(next.x - BTN_GAP - credits_w, CTRL_Y, credits_w, BTN_S);
    [credits, next]
}

/// Where along the bar `x` points, as a position of `dur`.
fn bar_ms(x: f32, dur: u64) -> u64 {
    (((x - SB_X) / SB_W).clamp(0.0, 1.0) as f64 * dur as f64) as u64
}


/// The scrub read-out's head: how far it moves and where to, `+2:30 → 1:14:05`.
pub fn scrub_head(target: u64, pos: u64) -> String {
    let delta = target as i64 - pos as i64;
    let sign = if delta < 0 { '\u{2212}' } else { '+' };
    format!("{sign}{} \u{2192} {}", clock(delta.unsigned_abs()), clock(target))
}

/// A playback clock: `1:23:45` past the hour, else `3:45`.
fn clock(ms: u64) -> String {
    sn_ui::fmt::clock(ms as i64)
}

/// The next episode of what plays, from the show's meta.
fn next_video(item: &PlayItem, meta: Option<&MetaState>) -> Option<VideoRef> {
    let Some(MetaState::Ready(m)) = meta else { return None };
    let v = sn_stremio::library::next_episode(&m.videos, &item.video_id)?;
    Some(video_ref(item, m, v))
}

/// Episode `v` of the show `m` that `item` is an episode of, as its Streams page names it.
fn video_ref(item: &PlayItem, m: &sn_stremio::types::Meta, v: &sn_stremio::types::Video) -> VideoRef {
    let address = crate::episodes::address(v.season.unwrap_or(0), v);
    let title = if v.title.trim().is_empty() { address } else { format!("{address} {}", v.title) };
    let show = if m.preview.name.is_empty() { item.name.clone() } else { m.preview.name.clone() };
    VideoRef { kind: item.kind.clone(), video_id: v.id.clone(), show, title, resume_ms: 0, item_id: item.id.clone(), from: String::new() }
}

/// How long the next episode waits on its addon's streams before its Streams page opens instead,
/// from when they were asked for (the addons' own stream timeout).
pub const PICK_WAIT_MS: u32 = 15_000;

/// The play of `next` on `stream`, the same release's stream `group` sent: from its saved
/// offset, else the start.
fn next_play(next: &VideoRef, group: &sn_stremio::streams::StreamGroup, stream: &sn_stremio::types::Stream, cx: &Cx<'_, Bundle>) -> Option<PlayRequest> {
    let Some(sn_stremio::types::Playable::Http { url, headers }) = stream.playable() else { return None };
    let saved = cx.views.library.items().iter().find(|i| i.id == next.item_id);
    Some(PlayRequest {
        url,
        headers,
        title: if next.title.is_empty() { next.show.clone() } else { format!("{} \u{2014} {}", next.show, next.title) },
        start_ms: saved.map_or(0, |i| i.saved_offset(&next.video_id)),
        addon: group.addon_name.clone(),
        item: Some(PlayItem {
            kind: next.kind.clone(),
            id: next.item_id.clone(),
            name: next.show.clone(),
            video_id: next.video_id.clone(),
            episode: next.title.clone(),
            release: Some(sn_stremio::stream_facts::Release::of(&group.addon_id, stream)),
        }),
        hints: sn_stremio::subtitles::FileHints::of(stream),
    })
}

/// Is what plays an episode of a work (so that it may have a next one)?
/// A video of a work rather than the work itself. Not read from the episode's title: a show's
/// meta may give its episodes none (anime often does).
fn is_episode(item: &PlayItem) -> bool {
    item.video_id != item.id
}

/// `x` moved toward `to` by `dt` over a ramp of `over` seconds.
fn ease(x: f32, to: f32, dt: f32, over: f32) -> f32 {
    let step = dt / over;
    if to > x { (x + step).min(to) } else { (x - step).max(to) }
}

/// The subtitle toggle's note (see `PlayerScreen::sub_note`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum SubNote {
    None,
    Waiting { was: Option<usize>, until: u32 },
    Shown { text: String, until: u32 },
}

/// The note saying what the subtitle toggle landed on: "Subtitles on: Deutsch" or "Subtitles off".
pub fn sub_note_text(media: &crate::registry::MediaFacts) -> String {
    match media.sub_sel {
        Some(i) if i < media.subs.len() => msg::app_player_subtitles_on(&tracks::track_label(&media.subs[i], i + 1, false)),
        Some(i) => match media.addon_subs.get(i - media.subs.len()) {
            Some(t) => msg::app_player_subtitles_on(&tracks::addon_sub_label(t)),
            None => msg::app_player_subtitles_turned_off().to_string(),
        },
        None => msg::app_player_subtitles_turned_off().to_string(),
    }
}

pub struct PlayerScreen {
    entry: EntryId,
    arg: PlayerArg,
    ctl: Controller,
    /// The meta this page holds open (the work's), for Up Next, the Episodes panel and the show
    /// info.
    meta: Option<MetaKey>,
    /// The last READY copy of that meta: the store can drop or reload it while this page plays
    /// (another page asking for the same show, the next episode's prefetch), and the Episodes
    /// button, the panel and Show info must not blink out with it.
    held: Option<sn_stremio::types::Meta>,
    /// A menu is over the page: the HUD stays up under it.
    covered: bool,
    /// Watch credits was pressed: Up Next is not offered again in this playback.
    declined: bool,
    stopped: bool,
    /// The page asked to leave (the end of the stream, BACK): nothing more is asked.
    leaving: bool,
    /// The next episode's streams this page holds open for its same release, and when they were
    /// asked for.
    prefetch: Option<(StreamKey, u32)>,
    /// What the same-release lookup settled on, for the episode named: the play, or `None` (the
    /// Streams page opens).
    picked: Option<(String, Option<Box<PlayRequest>>)>,
    /// An episode was asked for (the end, Next episode, a pick in the Episodes panel) while its
    /// streams were still on their way: each frame tries again until they settle.
    advancing: Option<VideoRef>,
    /// The centre pause badge's presence (0 gone, 1 shown) and the resume's play-glyph flash
    /// (1 just resumed, falling to 0).
    badge: f32,
    flash: f32,
    /// The film was paused at the last tick (its resume edge flashes the play glyph).
    was_paused: bool,
    /// GREEN is down since then (a hold opens the subtitle menu).
    green_since: Option<u32>,
    /// The disc whose menu is open over the page (its disc wears the "on" mark while it is).
    open_disc: Option<u8>,
    /// The subtitle toggle's note: the subtitle shown when it was asked and until when the note
    /// waits for the change (`Waiting`), then the note and until when it stands.
    sub_note: SubNote,
    /// What the last frame showed, so a change is a frame (see [`Self::fingerprint`]).
    shown: u64,
    /// The control row's pops: the discs, then the Episodes pill.
    pop: CtlPop<4>,
    /// The title lines, as the HUD draws them.
    title: String,
    kicker: String,
    /// The failure read-out's focused action (0 *Choose another stream*, 1 *Try again*).
    fail_focus: usize,
    /// The HUD's presence, 0 gone .. 1 shown: it fades ([`HUD_FADE_S`]) rather than cutting.
    hud_a: f32,
    /// The scrubber's focus and scrub emphasis, eased ([`BAR_EASE_S`]).
    bar_focus: f32,
    bar_scrub: f32,
    /// Since when the picture has stalled mid-playback (buffering with a picture up).
    stall_since: Option<u32>,
    /// *Start over* stands in the Skip slot until then (a resumed playback's first seconds);
    /// `Some(0)` once offered and gone.
    start_over: Option<u32>,
}

impl PlayerScreen {
    pub fn new(entry: EntryId, arg: PlayerArg) -> Self {
        let (title, kicker) = match &arg.item {
            Some(i) if is_episode(i) => (i.name.clone(), i.episode.clone()),
            _ => (arg.title.clone(), String::new()),
        };
        PlayerScreen {
            entry,
            title,
            kicker,
            arg,
            ctl: Controller::new(0),
            meta: None,
            held: None,
            covered: false,
            declined: false,
            stopped: false,
            leaving: false,
            prefetch: None,
            picked: None,
            advancing: None,
            badge: 0.0,
            flash: 0.0,
            was_paused: false,
            green_since: None,
            open_disc: None,
            sub_note: SubNote::None,
            shown: 0,
            pop: CtlPop::new(),
            fail_focus: 0,
            hud_a: 0.0,
            bar_focus: 0.0,
            bar_scrub: 0.0,
            stall_since: None,
            start_over: None,
        }
    }

    pub fn title(&self) -> &str {
        &self.arg.title
    }

    /// The HUD's presence (0 gone, 1 shown), as the tests read it.
    pub fn hud_alpha(&self) -> f32 {
        self.hud_a
    }

    /// The HUD's state, as the tests read it.
    pub fn controller(&self) -> &Controller {
        &self.ctl
    }

    fn ask(fx: &mut Effects<'_, Bundle>, cmd: PlayerCmd) {
        fx.push(Fx::App(AppFx::Player(cmd)));
    }

    fn stop(&mut self, fx: &mut Effects<'_, Bundle>) {
        if !std::mem::replace(&mut self.stopped, true) {
            Self::ask(fx, PlayerCmd::Stop);
        }
        if let Some(key) = self.meta.take() {
            fx.push(Fx::App(AppFx::Detail(DetailCmd::CloseMeta(key))));
        }
        if let Some((key, _)) = self.prefetch.take() {
            fx.push(Fx::App(AppFx::Detail(DetailCmd::CloseStreams(key))));
        }
    }

    /// **The next episode's same release**, as far as it is known: `None` while its addon's
    /// streams are on their way (asking for them the first time), else the play or `Some(None)`
    /// (the Streams page). Settled once, so a frame reads it without parsing the streams again.
    fn same_release(&mut self, next: &VideoRef, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Option<Option<Box<PlayRequest>>> {
        match &self.picked {
            Some((vid, p)) if *vid == next.video_id => return Some(p.clone()),
            Some(_) => self.picked = None,
            None => {}
        }
        let release = self.arg.item.as_ref().and_then(|i| i.release.as_ref());
        let Some(release) = release.filter(|_| cx.views.settings.get(Pref::NextEpisode) == NEXT_SAME) else {
            self.picked = Some((next.video_id.clone(), None));
            return Some(None);
        };
        // another episode than the one asked for before: let its streams go
        if self.prefetch.as_ref().is_some_and(|(k, _)| k.video_id != next.video_id) {
            if let Some((key, _)) = self.prefetch.take() {
                fx.push(Fx::App(AppFx::Detail(DetailCmd::CloseStreams(key))));
            }
        }
        let Some((key, since)) = &self.prefetch else {
            let key = StreamKey { kind: next.kind.clone(), video_id: next.video_id.clone() };
            fx.push(Fx::App(AppFx::Detail(DetailCmd::OpenStreams(key.clone()))));
            self.prefetch = Some((key, cx.tick.ms));
            return None;
        };
        let waited = cx.tick.ms.wrapping_sub(*since) >= PICK_WAIT_MS;
        let groups = match cx.views.detail.streams(key) {
            Some(g) => g,
            None if !waited => return None,
            None => &[],
        };
        let group = groups.iter().find(|g| g.addon_id == release.addon_id);
        let pick = match group.map(|g| (g, &g.state)) {
            Some((_, sn_stremio::streams::GroupState::Loading)) if !waited => return None,
            Some((g, sn_stremio::streams::GroupState::Ready(streams))) => sn_stremio::stream_facts::same_release(release, streams)
                .and_then(|i| next_play(next, g, &streams[i], cx))
                .map(Box::new),
            _ => None,
        };
        self.picked = Some((next.video_id.clone(), pick.clone()));
        Some(pick)
    }

    /// On to the next episode (the end of this one, or Next episode); back when there is none.
    fn advance(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let Some(next) = self.next(cx) else { return self.leave(None, fx) };
        self.go_to(next, cx, fx);
    }

    /// On to episode `to`: its same release when one was found, else its Streams page; while its
    /// addon's streams are on their way, each frame tries again (the playback goes on).
    fn go_to(&mut self, to: VideoRef, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        match self.same_release(&to, cx, fx) {
            None => self.advancing = Some(to),
            Some(pick) => {
                self.advancing = None;
                self.leave(Some(to), fx);
                if let Some(req) = pick {
                    // over the episode's Streams page, which BACK returns to
                    fx.push(Fx::App(AppFx::Play(*req)));
                }
            }
        }
    }

    /// The show's meta, once it is in.
    fn meta_ready<'s>(&'s self, cx: &'s Cx<'_, Bundle>) -> Option<&'s sn_stremio::types::Meta> {
        match cx.views.detail.meta(self.meta.as_ref()?) {
            Some(MetaState::Ready(m)) => Some(m),
            _ => self.held.as_ref(),
        }
    }

    /// Keep [`Self::held`] up to date with the store's ready meta.
    fn hold_meta(&mut self, cx: &Cx<'_, Bundle>) {
        let Some(key) = self.meta.as_ref() else { return };
        if let Some(MetaState::Ready(m)) = cx.views.detail.meta(key) {
            if self.held.as_ref() != Some(&**m) {
                self.held = Some((**m).clone());
            }
        }
    }

    /// Does the Episodes pill stand in the row: an episode whose show lists its episodes?
    fn episodes_on(&self, cx: &Cx<'_, Bundle>) -> bool {
        self.arg.item.as_ref().is_some_and(is_episode) && self.meta_ready(cx).is_some_and(|m| !m.videos.is_empty())
    }

    fn leave(&mut self, to: Option<VideoRef>, fx: &mut Effects<'_, Bundle>) {
        if std::mem::replace(&mut self.leaving, true) {
            return;
        }
        // the next episode's page is opened before the meta it was read from is let go
        let nav = match to {
            Some(v) => NavOp::Replace(AppArg::Streams(v)),
            None => NavOp::Pop,
        };
        self.stop(fx);
        fx.push(Fx::Nav(nav));
    }

    fn next(&self, cx: &Cx<'_, Bundle>) -> Option<VideoRef> {
        let item = self.arg.item.as_ref().filter(|i| is_episode(i))?;
        let key = self.meta.as_ref()?;
        next_video(item, cx.views.detail.meta(key))
    }

    /// The instant, as the HUD decides on it.
    fn now(&self, cx: &Cx<'_, Bundle>) -> Now {
        let v = cx.views.playback;
        let c = v.clock;
        let has_next = self.meta.is_some() && self.next(cx).is_some();
        let mode = cx.views.settings.get(Pref::SkipIntros);
        Now {
            ms: cx.tick.ms,
            pos: c.pos_ms,
            dur: c.dur_ms,
            paused: v.phase == PlayPhase::Paused,
            picture: c.picture,
            pinned: self.covered || v.phase == PlayPhase::Working || c.seeking || self.advancing.is_some(),
            offered: {
                let (lead, at_credits) = Pref::up_next_lead(cx.views.settings.get(Pref::UpNextAt));
                let credits = v.skips.credits.map(|s| s.start_ms).filter(|_| at_credits);
                hud::up_next_offered(has_next, self.declined, c.pos_ms, c.dur_ms, c.ended, credits, lead)
            },
            skip: if mode == SKIP_OFF || c.dur_ms == 0 { None } else { v.skips.at(c.pos_ms) },
            auto_skip: mode == SKIP_AUTO,
            episodes: self.episodes_on(cx),
            start_over: self.start_over_on(cx.tick.ms),
        }
    }

    /// Is *Start over* on offer at `now`?
    fn start_over_on(&self, now: u32) -> bool {
        self.start_over.is_some_and(|until| until != 0 && (until.wrapping_sub(now) as i32) > 0)
    }

    /// Has a mid-playback stall lasted long enough for the centre plate?
    fn stalled(&self, now: u32) -> bool {
        self.stall_since.is_some_and(|t| now.wrapping_sub(t) >= STALL_PLATE_MS)
    }

    /// Is the HUD drawn this frame? Never over a failure, which owns the frame.
    fn hud_drawn(&self, n: &Now, cx: &Cx<'_, Bundle>) -> bool {
        cx.views.playback.phase != PlayPhase::Failed && self.ctl.visible(n)
    }

    /// Everything a frame draws from, folded: a change is a reason to present.
    fn fingerprint(&self, n: &Now, cx: &Cx<'_, Bundle>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let v = cx.views.playback;
        let drawn = self.hud_drawn(n, cx);
        drawn.hash(&mut h);
        (v.phase as u8).hash(&mut h);
        v.caption.hash(&mut h);
        self.fail_focus.hash(&mut h);
        cx.views.epg.gen().hash(&mut h);
        n.start_over.hash(&mut h);
        self.stalled(n.ms).hash(&mut h);
        v.clock.seeking.hash(&mut h);
        if v.phase != PlayPhase::Failed {
            n.paused.hash(&mut h);
            (n.pos / 1000).hash(&mut h);
            n.episodes.hash(&mut h);
            self.ctl.skip_shown(n).hash(&mut h);
            self.ctl.note(n).hash(&mut h);
            self.ctl.hud.focus.hash(&mut h);
        }
        if drawn {
            (0..hud::DISCS).map(|i| self.disc_lit(i, &v.media)).collect::<Vec<_>>().hash(&mut h);
            (self.ctl.shown_pos(n) / 1000).hash(&mut h);
            (n.dur / 1000).hash(&mut h);
            self.ctl.hud.focus.hash(&mut h);
            self.ctl.scrub.live().hash(&mut h);
            self.ctl.undo.map(|u| u.armed(n.ms)).hash(&mut h);
            n.offered.hash(&mut h);
            ((v.clock.buffered_ms / 1000) as u32).hash(&mut h);
            self.ctl.scrub.hold_step(n.ms).hash(&mut h);
            self.found().hash(&mut h);
            self.advancing.as_ref().map(|v| &v.video_id).hash(&mut h);
        }
        h.finish()
    }

    fn act(&mut self, a: Option<Action>, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let Some(a) = a else { return };
        match a {
            Action::TogglePause => Self::ask(fx, PlayerCmd::TogglePause),
            Action::SetPaused(p) => Self::ask(fx, PlayerCmd::SetPaused(p)),
            Action::Seek(ms) => Self::ask(fx, PlayerCmd::Seek(ms)),
            Action::SkipTo(ms) => Self::ask(fx, PlayerCmd::SkipTo(ms)),
            Action::SeekAndPlay(ms) => {
                Self::ask(fx, PlayerCmd::Seek(ms));
                Self::ask(fx, PlayerCmd::SetPaused(false));
            }
            Action::OpenMenu(disc) => {
                let v = cx.views.playback;
                let kind = match disc {
                    hud::DISC_SUBTITLES => MenuKind::Subtitles,
                    hud::DISC_AUDIO => tracks::audio_menu(v),
                    _ => tracks::more_menu(v),
                };
                self.present(kind, Some(ELEM_DISC + u32::from(disc)), disc_rect(disc), fx);
                self.open_disc = Some(disc);
            }
            Action::ToggleSubtitles => self.toggle_subtitles(cx, fx),
            Action::UpNext(hud::UP_NEXT_PLAY) => {
                if self.next(cx).is_some() {
                    self.advance(cx, fx);
                }
            }
            Action::UpNext(_) => {
                self.declined = true;
                self.ctl.hud.focus = Focus::Scrub;
            }
            Action::StartOver => {
                self.start_over = Some(0);
                Self::ask(fx, PlayerCmd::Seek(0));
            }
            Action::OpenEpisodes if !self.episodes_on(cx) => {}
            Action::OpenEpisodes => {
                let (Some(meta), Some(item)) = (self.meta.clone(), self.arg.item.as_ref()) else { return };
                let kind = MenuKind::Player(PlayerPanel::Episodes { meta, video_id: item.video_id.clone() });
                self.present(kind, Some(ELEM_EPISODES), episodes_rect(cx.measure), fx);
            }
            Action::Leave => self.leave(None, fx),
        }
        fx.invalidate(Provenance::Input);
    }

    /// Does disc `i` wear its "on" mark: the Subtitles disc while subtitles show, and any disc
    /// while its own menu is open over the page (the same disc, or its colour key, closes it).
    pub fn disc_lit(&self, i: u8, media: &crate::registry::MediaFacts) -> bool {
        self.open_disc == Some(i) || (i == hud::DISC_SUBTITLES && media.sub_sel.is_some())
    }

    /// The next episode's same release was found (the Up Next line says so).
    fn found(&self) -> bool {
        matches!(self.picked, Some((_, Some(_))))
    }

    fn present(&self, kind: MenuKind, elem: Option<u32>, anchor: Rect, fx: &mut Effects<'_, Bundle>) {
        let reply = fx.from();
        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
            host: self.entry,
            reply,
            focus: elem.map(|elem| FocusKey { entry: self.entry, elem }),
            anchor: MenuArg::anchor_of(anchor),
            kind,
        }))));
    }

    /// A menu row was picked.
    fn chose(&mut self, target: ChoiceTarget, i: usize, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let more = disc_rect(hud::DISC_MORE);
        match target {
            ChoiceTarget::PlayerAudio if i < cx.views.playback.media.audio.len() => Self::ask(fx, PlayerCmd::Audio(i)),
            // the row after the tracks: the audio sync's steps, over the audio disc
            ChoiceTarget::PlayerAudio if i == cx.views.playback.media.audio.len() => {
                self.present(crate::dropdown::audio_sync_menu(cx.views.playback), None, disc_rect(hud::DISC_AUDIO), fx);
            }
            ChoiceTarget::PlayerAudioSync => {
                if let Some(ms) = crate::dropdown::audio_sync_pick(i, cx.views.playback.audio_delay_ms) {
                    Self::ask(fx, PlayerCmd::AudioDelay(ms));
                }
            }
            ChoiceTarget::PlayerSubtitleDelay => {
                if let Some(ms) = tracks::delay_pick(i, cx.views.playback.sub_delay_ms) {
                    Self::ask(fx, PlayerCmd::SubtitleDelay(ms));
                }
            }
            ChoiceTarget::PlayerMore => match tracks::MORE_ROWS.get(i) {
                Some(tracks::MoreRow::Info) => self.open_show_info(cx, fx),
                Some(tracks::MoreRow::Aspect) => self.present(tracks::aspect_menu(cx.views.playback), None, more, fx),
                Some(tracks::MoreRow::Stats) => Self::ask(fx, PlayerCmd::ToggleStats),
                None => {}
            },
            ChoiceTarget::PlayerAspect if i < tracks::aspect_labels().len() => Self::ask(fx, PlayerCmd::Aspect(i as u8)),
            ChoiceTarget::PlayerSubtitleSize => fx.push(Fx::App(AppFx::Settings(SettingsCmd::Set(Pref::SubtitleSize, i as u8)))),
            ChoiceTarget::PlayerSubtitlePosition => {
                fx.push(Fx::App(AppFx::Settings(SettingsCmd::Set(Pref::SubtitlePosition, i as u8))))
            }
            ChoiceTarget::PlayerSkipIntros => fx.push(Fx::App(AppFx::Settings(SettingsCmd::Set(Pref::SkipIntros, i as u8)))),
            ChoiceTarget::PlayerDvProfile7 => fx.push(Fx::App(AppFx::Settings(SettingsCmd::Set(Pref::DvProfile7, i as u8)))),
            ChoiceTarget::PlayerEpisode => {
                let to = match (self.arg.item.as_ref(), self.meta_ready(cx)) {
                    (Some(item), Some(m)) => m.videos.get(i).filter(|v| v.id != item.video_id).map(|v| video_ref(item, m, v)),
                    _ => None,
                };
                if let Some(to) = to {
                    self.go_to(to, cx, fx);
                }
            }
            _ => {}
        }
    }

    /// One of the player's own keys ([`player_key`]).
    fn player_key(&mut self, k: PlayerKey, edge: Edge, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        let v = cx.views.playback;
        if v.phase == PlayPhase::Failed {
            return Handled::Yes;
        }
        let hk = match k {
            // a live stream's channel keys are not seeks
            PlayerKey::Hud(HudKey::ChUp | HudKey::ChDown) if v.clock.dur_ms == 0 => return Handled::No,
            PlayerKey::Hud(hk) => hk,
            PlayerKey::Green => {
                match edge {
                    Edge::Down => self.green_since = Some(cx.tick.ms),
                    Edge::Repeat => self.green_held(cx, fx),
                    Edge::Up => {
                        if self.green_since.take().is_some() {
                            self.toggle_subtitles(cx, fx);
                        }
                    }
                }
                return Handled::Yes;
            }
            _ if edge != Edge::Down => return Handled::Yes,
            PlayerKey::Red => {
                self.act(Some(Action::OpenMenu(hud::DISC_AUDIO)), cx, fx);
                return Handled::Yes;
            }
            PlayerKey::Yellow if self.episodes_on(cx) => {
                self.act(Some(Action::OpenEpisodes), cx, fx);
                return Handled::Yes;
            }
            PlayerKey::Yellow | PlayerKey::Blue => {
                self.open_show_info(cx, fx);
                return Handled::Yes;
            }
        };
        self.key_hud(hk, edge, cx, fx)
    }

    /// GREEN held past [`GREEN_HOLD_MS`]: the subtitle menu, once.
    fn green_held(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        if self.green_since.is_some_and(|t| cx.tick.ms.wrapping_sub(t) >= GREEN_HOLD_MS) {
            self.green_since = None;
            self.act(Some(Action::OpenMenu(hud::DISC_SUBTITLES)), cx, fx);
        }
    }

    /// GREEN's press: subtitles on or off, and a note saying which once the playback has turned.
    fn toggle_subtitles(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        Self::ask(fx, PlayerCmd::ToggleSubtitles);
        self.sub_note = SubNote::Waiting { was: cx.views.playback.media.sub_sel, until: cx.tick.ms.wrapping_add(SUB_NOTE_WAIT_MS) };
        fx.invalidate(Provenance::Input);
    }

    /// The show info panel, over the `…` disc.
    fn open_show_info(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let kind = MenuKind::Player(PlayerPanel::ShowInfo {
            meta: self.meta.clone(),
            video_id: self.arg.item.as_ref().map(|i| i.video_id.clone()).unwrap_or_default(),
            title: self.title.clone(),
            tech: tracks::tech_line(&cx.views.playback.media, &self.arg.addon),
        });
        self.present(kind, None, disc_rect(hud::DISC_MORE), fx);
    }

    /// The note the subtitle toggle shows, while it stands.
    pub fn sub_note(&self, now: u32) -> Option<&str> {
        match &self.sub_note {
            SubNote::Shown { text, until } if (until.wrapping_sub(now) as i32) >= 0 => Some(text),
            _ => None,
        }
    }

    fn key(&mut self, key: UiKey, edge: Edge, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        let hk = match key {
            UiKey::Up => HudKey::Up,
            UiKey::Down => HudKey::Down,
            UiKey::Left { .. } => HudKey::Left,
            UiKey::Right { .. } => HudKey::Right,
            UiKey::Ok => HudKey::Ok,
            UiKey::Back => HudKey::Back,
            UiKey::Play => HudKey::Play,
            UiKey::Pause => HudKey::Pause,
            UiKey::PlayPause => HudKey::PlayPause,
            UiKey::Stop => HudKey::Stop,
            _ => return Handled::No,
        };
        self.key_hud(hk, edge, cx, fx)
    }

    fn key_hud(&mut self, hk: HudKey, edge: Edge, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        // A failure owns the frame: nothing under it is drawn, so nothing is driven but its
        // two actions (LEFT/RIGHT between them, OK presses one). BACK and STOP leave.
        if cx.views.playback.phase == PlayPhase::Failed {
            if edge == Edge::Down {
                match hk {
                    HudKey::Left => self.fail_focus = 0,
                    HudKey::Right => self.fail_focus = FAIL_ACTIONS - 1,
                    HudKey::Ok => self.fail_act(self.fail_focus, fx),
                    HudKey::Back | HudKey::Stop => self.leave(None, fx),
                    _ => {}
                }
                fx.invalidate(Provenance::Input);
            }
            return Handled::Yes;
        }
        let edge = match edge {
            Edge::Down => KeyEdge::Down,
            Edge::Repeat => KeyEdge::Repeat,
            Edge::Up => KeyEdge::Up,
        };
        let n = self.now(cx);
        self.ctl.chapters.clear();
        self.ctl.chapters.extend(cx.views.playback.media.chapters.iter().map(|c| c.start_ms));
        let a = self.ctl.key(hk, edge, &n);
        self.act(a, cx, fx);
        fx.invalidate(Provenance::Input);
        Handled::Yes
    }

    /// The failure read-out's action `i`: back to the stream list, or the same stream again.
    fn fail_act(&mut self, i: usize, fx: &mut Effects<'_, Bundle>) {
        if i == 0 {
            self.leave(None, fx);
        } else {
            self.fail_focus = 0;
            Self::ask(fx, PlayerCmd::Retry);
        }
        fx.invalidate(Provenance::Input);
    }

    fn focus_of(elem: u32) -> Option<Focus> {
        match elem {
            ELEM_SCRUB => Some(Focus::Scrub),
            ELEM_SKIP => Some(Focus::Skip),
            ELEM_EPISODES => Some(Focus::Episodes),
            e if (ELEM_DISC..ELEM_DISC + u32::from(hud::DISCS)).contains(&e) => Some(Focus::Disc((e - ELEM_DISC) as u8)),
            e if (ELEM_UP_NEXT..ELEM_UP_NEXT + u32::from(hud::UP_NEXT_ITEMS)).contains(&e) => {
                Some(Focus::UpNext((e - ELEM_UP_NEXT) as u8))
            }
            _ => None,
        }
    }

    fn pointer(&mut self, kind: &InputKind<u32>, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        if cx.views.playback.phase == PlayPhase::Failed {
            if let InputKind::Pointer { hit: Some(e), .. } = *kind {
                if (ELEM_FAIL..ELEM_FAIL + FAIL_ACTIONS as u32).contains(&e) {
                    self.fail_focus = (e - ELEM_FAIL) as usize;
                    fx.invalidate(Provenance::Input);
                }
            }
            return Handled::Yes;
        }
        let n = self.now(cx);
        let was = self.hud_drawn(&n, cx);
        match *kind {
            InputKind::Pointer { hit, .. } => self.ctl.hover(hit.filter(|_| was).and_then(Self::focus_of), &n),
            // a press on the bar seats a drag; a press on a control is its `Activate`
            InputKind::Click { x, hit: Some(ELEM_SCRUB), .. } if was && n.dur > 0 => self.ctl.drag(bar_ms(x, n.dur), &n),
            InputKind::Click { .. } => self.ctl.hover(None, &n),
            InputKind::Drag { x, .. } if self.ctl.scrub.dragging() => self.ctl.drag(bar_ms(x, n.dur), &n),
            _ => return Handled::No,
        }
        fx.invalidate(Provenance::Input);
        Handled::Yes
    }

    // ---- drawing ------------------------------------------------------------------------------

    /// A live channel's TV guide lines: what is on air (over the title) and what follows (over the
    /// control row); `None` each for anything else or what the guide does not know.
    fn guide_lines(&self, cx: &Cx<'_, Bundle>) -> (Option<String>, Option<String>) {
        let Some(item) = self.arg.item.as_ref().filter(|i| matches!(i.kind.as_str(), "tv" | "channel")) else { return (None, None) };
        let Some(nn) = cx.views.epg.now_next(&item.name) else { return (None, None) };
        (nn.now.map(|p| msg::app_live_now(&p.title)), nn.next.map(|p| msg::app_live_next(&p.title)))
    }

    fn draw_hud(&self, p: Painter, n: &Now, cx: &Cx<'_, Bundle>, measure: &dyn Measure) {
        let env = Env::inert();
        // front-loaded: dark enough under the title block and the Up Next line by a third of the
        // way down, then on to the foot
        let top = SCR_H - SCRIM_H;
        let knee = top + SCRIM_H * SCRIM_KNEE;
        p.rect(Rect::new(0.0, top, SCR_W, knee - top), 0.0, theme::scrim_black(0.0), theme::scrim_black(SCRIM_KNEE_A), 0.0);
        p.rect(Rect::new(0.0, knee, SCR_W, SCR_H - knee), 0.0, theme::scrim_black(SCRIM_KNEE_A), theme::scrim_black(SCRIM_FOOT_A), 0.0);
        let focus = self.ctl.hud.focus;
        let guide = self.guide_lines(cx);
        // the control row: the discs, or the Up Next pair standing in for them
        let row_left = if n.offered {
            let rects = up_next_rects(measure);
            let labels = [msg::app_player_watch_credits(), msg::app_player_next_episode()];
            for (i, (r, label)) in rects.iter().zip(labels).enumerate() {
                let c = CString::new(label).unwrap_or_default();
                Button::new(c.as_ptr(), UP_NEXT_SZ, *r)
                    .focused(focus == Focus::UpNext(i as u8))
                    .ground(ControlGround::Unkeyed)
                    .scale(self.pop.scale(i))
                    .draw(&env, p);
            }
            if let Some(next) = self.next(cx) {
                let mut line = format!("{} \u{b7} {}", msg::app_player_up_next(), next.title);
                if self.found() {
                    line = format!("{line} \u{b7} {}", msg::app_settings_next_same());
                }
                let fitted = measure.fit_line(&line, CTRL_RIGHT - rects[0].x, theme::size::CAPTION, false);
                p.text(fitted.as_ptr(), CTRL_RIGHT, UP_NEXT_CAPTION_Y, theme::size::CAPTION, theme::TEXT_SECONDARY, 2, 0);
            }
            rects[0].x
        } else {
            for i in 0..hud::DISCS {
                TransportButton::new(i32::from(i), disc_rect(i))
                    .focused(focus == Focus::Disc(i))
                    .lit(self.disc_lit(i, &cx.views.playback.media))
                    .ground(ControlGround::Unkeyed)
                    .scale(self.pop.scale(usize::from(i)))
                    .draw(&env, p);
            }
            let left = if n.episodes {
                let r = episodes_rect(measure);
                TransportButton::new(sn_ui::widgets::TRANSPORT_EPISODES, r)
                    .focused(focus == Focus::Episodes)
                    .ground(ControlGround::Unkeyed)
                    .scale(self.pop.scale(usize::from(hud::DISCS)))
                    .draw(&env, p);
                r.x
            } else {
                disc_rect(0).x
            };
            // an episode picked in the Episodes panel, waiting on its addon's streams; a live
            // channel's next programme in the same place
            let line = match &self.advancing {
                Some(to) => Some(format!("{} \u{b7} {}", msg::app_player_up_next(), to.title)),
                None => guide.1.clone(),
            };
            if let Some(line) = line {
                let fitted = measure.fit_line(&line, CTRL_RIGHT - left, theme::size::CAPTION, false);
                p.text(fitted.as_ptr(), CTRL_RIGHT, UP_NEXT_CAPTION_Y, theme::size::CAPTION, theme::TEXT_SECONDARY, 2, 0);
            }
            left
        };
        // the title block, clear of the row
        let budget = (row_left - TITLE_GAP - SB_X).max(0.0);
        let kicker = if self.kicker.is_empty() { guide.0.unwrap_or_default() } else { self.kicker.clone() };
        if !kicker.is_empty() {
            let k = measure.fit_line(&kicker, budget, theme::size::CAPTION, true);
            p.text(k.as_ptr(), SB_X, KICKER_Y, theme::size::CAPTION, theme::TEXT_PRIMARY, 0, 1);
        }
        let t = measure.fit_line(&self.title, budget, HUD_TITLE_SZ, true);
        p.text(t.as_ptr(), SB_X, TITLE_Y, HUD_TITLE_SZ, theme::TEXT_PRIMARY, 0, 1);
        // no remote-key hints on the HUD (owner's verdict): the colour keys are documented in
        // docs/remote-keys.md and Settings > Remote keys
        self.draw_bar(p, n, cx, measure);
    }

    /// The scrubber ([`sn_ui::widgets::Scrubber`]: the rail, the buffered range, the played part,
    /// the chapter cuts, the playhead and, while scrubbing, the time bubble with the hold's step),
    /// then the clocks under it — elapsed on the left with the state mark after it, remaining on
    /// the right — and the undo chip between them.
    fn draw_bar(&self, p: Painter, n: &Now, cx: &Cx<'_, Bundle>, measure: &dyn Measure) {
        let white = theme::TEXT_PRIMARY;
        let pos = self.ctl.shown_pos(n);
        let dur = n.dur;
        let frac = |ms: u64| if dur > 0 { (ms as f64 / dur as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let clock_view = cx.views.playback.clock;
        let buffered = (clock_view.buffered_ms > 0 && dur > 0).then(|| frac(clock_view.buffered_ms));
        let chapters: Vec<f32> = cx.views.playback.media.chapters.iter().map(|c| frac(c.start_ms)).collect();
        // the bubble: the target, and a held key's step (`+0:30`) while one runs
        let bubble = self.ctl.scrub.preview.map(|t| match self.ctl.scrub.hold_step(n.ms) {
            Some(step) => format!("{} \u{b7} {}{}", clock(t), if t >= n.pos { '+' } else { '\u{2212}' }, clock(step)),
            None => clock(t),
        });
        sn_ui::widgets::Scrubber::new(Rect::new(SB_X, SB_Y + SB_H * 0.5, SB_W, SB_H), frac(pos))
            .buffered(buffered)
            .emphasis(self.bar_focus, self.bar_scrub)
            .bubble(bubble.as_deref())
            .chapters(&chapters)
            .draw(p, measure);
        // the clocks: elapsed at the bar's left end, remaining at its right; a digit-blind template
        // keeps a clock's box still while its digits tick
        let sz = theme::size::CAPTION;
        let template = |s: &str| s.chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect::<String>();
        let elapsed = clock(pos);
        let ew = measure.width_str(&template(&elapsed), sz, true);
        if let Ok(c) = CString::new(elapsed) {
            p.text(c.as_ptr(), SB_X, CLOCK_Y, sz, white, 0, 1);
        }
        let el_r = SB_X + ew;
        let mut busy_r = el_r;
        // the state mark past the clock: pause while paused, a spinner while a seek resolves
        const MARK: f32 = 30.0;
        let icy = CLOCK_Y + measure.line_h(sz) * 0.5;
        if clock_view.seeking {
            // placeholder-exempt: the transport's inline busy mark, reported by the pipeline, not a content stand-in
            Spinner::new(el_r + 14.0 + MARK * 0.5, icy, Spinner::R_INLINE).phase(n.ms).tint(white).draw(&Env::inert(), p);
            busy_r = el_r + 14.0 + MARK;
        } else if n.paused && !self.ctl.scrub.live() {
            sn_ui::icons::draw(p, Icon::Pause, Rect::new(el_r + 6.0, icy - MARK * 0.5, MARK, MARK), white);
            busy_r = el_r + 6.0 + MARK;
        }
        let mut rem_l = SB_X + SB_W;
        if dur > 0 {
            let rem = format!("-{}", clock(dur.saturating_sub(pos)));
            let rw = measure.width_str(&template(&rem), sz, true);
            rem_l = SB_X + SB_W - rw;
            if let Ok(c) = CString::new(rem) {
                p.text(c.as_ptr(), SB_X + SB_W, CLOCK_Y, sz, theme::TEXT_SECONDARY, 2, 0);
            }
        }
        // the undo window: a ghost tick where the scrub started, and the chip saying BACK goes there
        if let Some(u) = self.ctl.undo.filter(|u| u.armed(n.ms) && dur > 0) {
            let ox = SB_X + SB_W * frac(u.origin);
            let cy = SB_Y + SB_H * 0.5;
            let ghost = theme::with_a(theme::TINT_WHITE, 0.55);
            p.rect(Rect::new(ox - 1.0, cy - 8.0, 2.0, 16.0), 1.0, ghost, ghost, 0.0);
            let chip = msg::app_player_undo(&clock(u.origin));
            let w = measure.width_str(&chip, theme::size::CAPTION, false);
            let x = (ox - w * 0.5).clamp(busy_r + 20.0, (rem_l - 20.0 - w).max(busy_r + 20.0));
            if x + w + 20.0 <= rem_l {
                if let Ok(c) = CString::new(chip) {
                    p.text(c.as_ptr(), x, CLOCK_Y, theme::size::CAPTION, theme::TEXT_SECONDARY, 0, 1);
                }
            }
        }
    }

    /// The centre of the picture: a live scrub's target (large, its chapter under it, and how far
    /// it moves), else the pause badge while it shows (or the resume's play flash).
    fn draw_centre(&self, p: Painter, n: &Now, cx: &Cx<'_, Bundle>, measure: &dyn Measure, hud: bool) {
        let media = &cx.views.playback.media;
        if let Some(target) = self.ctl.scrub.preview.filter(|_| hud) {
            let speed = self.ctl.scrub.speed();
            let head = scrub_head(target, n.pos);
            let mut sub: Vec<String> = Vec::new();
            if let Some((_, rate)) = speed {
                sub.push(format!("\u{d7}{rate}"));
            }
            if let Some(ch) = media.chapter_at(target) {
                sub.push(measure.fit_line(ch, SCR_W * 0.5, theme::size::BODY, false).to_string_lossy().into_owned());
            }
            let glyph = speed.map(|(fwd, _)| if fwd { Icon::FastForward } else { Icon::Rewind });
            self.draw_plate(p, glyph, &head, theme::size::DISPLAY, &sub.join(" \u{b7} "), 1.0, 1.0, measure);
            return;
        }
        if self.badge > 0.0 {
            let pos = if n.dur > 0 { format!("{} / {}", clock(n.pos), clock(n.dur)) } else { clock(n.pos) };
            let a = self.badge;
            self.draw_plate(p, Some(Icon::Pause), msg::app_player_paused(), theme::size::TITLE, &pos, a, 0.92 + 0.08 * a, measure);
        } else if self.flash > 0.0 {
            self.draw_plate(p, Some(Icon::Play), "", theme::size::TITLE, "", self.flash, 1.0 + 0.12 * (1.0 - self.flash), measure);
        }
    }

    /// One centre read-out: a rounded translucent badge (with `glyph`), `head` under it at
    /// `head_sz` and `sub` under that, at opacity `a` and scale `scale` about its centre.
    #[allow(clippy::too_many_arguments)]
    fn draw_plate(&self, p: Painter, glyph: Option<Icon>, head: &str, head_sz: i32, sub: &str, a: f32, scale: f32, measure: &dyn Measure) {
        let p = p.alpha(a);
        let cx = SCR_W * 0.5;
        let ground = theme::scrim_black(BADGE_GROUND_A);
        let mut y = BADGE_CY;
        if let Some(g) = glyph {
            let d = BADGE_D * scale;
            p.rect(Rect::new(cx - d * 0.5, y - d * 0.5, d, d), d * 0.5, ground, ground, 0.0);
            let gs = BADGE_GLYPH * scale;
            sn_ui::icons::draw(p, g, Rect::new(cx - gs * 0.5, y - gs * 0.5, gs, gs), theme::TEXT_PRIMARY);
            y += d * 0.5 + theme::space::MD;
        } else {
            // no disc: the head stands on a pill of its own, centred where the disc would be
            y -= measure.line_h(head_sz) * 0.5;
        }
        let head_h = measure.line_h(head_sz);
        let sub_h = if sub.is_empty() { 0.0 } else { measure.line_h(theme::size::BODY) };
        if head.is_empty() {
            return;
        }
        let w = measure.width_str(head, head_sz, true).max(measure.width_str(sub, theme::size::BODY, false)) + 2.0 * theme::space::LG;
        let h = head_h + sub_h + 2.0 * theme::space::SM;
        p.rect(Rect::new(cx - w * 0.5, y, w, h), theme::space::MD, ground, ground, 0.0);
        let hc = CString::new(head).unwrap_or_default();
        Label::new(hc.as_ptr(), head_sz, theme::TEXT_PRIMARY).bold().h(HAlign::Center).v(VAlign::Middle).draw(p, Rect::new(cx - w * 0.5, y + theme::space::SM, w, head_h));
        if !sub.is_empty() {
            let sc = CString::new(sub).unwrap_or_default();
            Label::new(sc.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY)
                .h(HAlign::Center)
                .v(VAlign::Middle)
                .draw(p, Rect::new(cx - w * 0.5, y + theme::space::SM + head_h, w, sub_h));
        }
    }

    /// The wait before the first picture (Connecting…, Buffering…): a progress ring in the
    /// picture's centre — lit as far as the start buffer has filled when the media layer knows
    /// ([`crate::registry::PlayClock::buffer_pct`], the share read inside it), a comet running round
    /// it either way — with the caption under it. The ring keeps the frames coming while it shows.
    fn draw_working(p: Painter, cx: &Cx<'_, Bundle>, measure: &dyn Measure) {
        let view = cx.views.playback;
        sn_ui::placeholder::note(p, sn_ui::placeholder::Reason::WorkingReadout, "player");
        let (x, y) = (SCR_W * 0.5, BADGE_CY);
        let pct = view.clock.buffer_pct;
        let ring = sn_ui::widgets::ProgressRing::new(x, y, sn_ui::widgets::ProgressRing::R_PAGE)
            .phase(cx.tick.ms)
            .progress(pct.map(|v| f32::from(v) / 100.0))
            .tint(theme::TEXT_PRIMARY);
        ring.draw(&Env::inert(), p);
        if let Some(v) = pct {
            let c = CString::new(format!("{v}%")).unwrap_or_default();
            let r = sn_ui::widgets::ProgressRing::R_PAGE;
            Label::new(c.as_ptr(), theme::size::CAPTION, theme::TEXT_PRIMARY).bold().h(HAlign::Center).v(VAlign::Middle).draw(p, Rect::new(x - r, y - r, 2.0 * r, 2.0 * r));
        }
        let top = y + sn_ui::widgets::ProgressRing::R_PAGE + theme::space::LG;
        let h = measure.line_h(theme::size::HEADLINE);
        Label::new(view.caption.as_ptr(), theme::size::HEADLINE, theme::TEXT_SECONDARY)
            .h(HAlign::Center)
            .v(VAlign::Middle)
            .draw(p, Rect::new(MARGIN_X, top, SCR_W - 2.0 * MARGIN_X, h));
    }

    /// **A stall with the picture up**, past [`STALL_PLATE_MS`]: the centre plate — a comet ring on
    /// the badge's translucent disc, the pipeline's caption (*Buffering…*) under it — so a frozen
    /// picture says why, where the clock's own spinner is too small to.
    fn draw_stall(p: Painter, cx: &Cx<'_, Bundle>, measure: &dyn Measure) {
        let view = cx.views.playback;
        let ground = theme::scrim_black(BADGE_GROUND_A);
        let (x, y) = (SCR_W * 0.5, BADGE_CY);
        p.rect(Rect::new(x - BADGE_D * 0.5, y - BADGE_D * 0.5, BADGE_D, BADGE_D), BADGE_D * 0.5, ground, ground, 0.0);
        sn_ui::widgets::ProgressRing::new(x, y, sn_ui::widgets::ProgressRing::R_PAGE).phase(cx.tick.ms).progress(None).tint(theme::TEXT_PRIMARY).draw(&Env::inert(), p);
        let caption = view.caption.to_str().unwrap_or("");
        if caption.is_empty() {
            return;
        }
        let h = measure.line_h(theme::size::BODY) + 2.0 * theme::space::SM;
        let w = measure.width_str(caption, theme::size::BODY, true) + 2.0 * theme::space::LG;
        let top = y + BADGE_D * 0.5 + theme::space::MD;
        let pill = Rect::new(x - w * 0.5, top, w, h);
        p.rect(pill, theme::space::MD, ground, ground, 0.0);
        Label::new(view.caption.as_ptr(), theme::size::BODY, theme::TEXT_PRIMARY).bold().h(HAlign::Center).v(VAlign::Middle).draw(p, pill);
    }

    /// The read-out this phase calls for: the failure (the wait before the first picture is
    /// [`Self::draw_working`]'s ring).
    fn overlay(&self, cx: &Cx<'_, Bundle>) -> Option<StatusOverlay<'static>> {
        let view = cx.views.playback;
        match view.phase {
            PlayPhase::Failed => {
                let mut o = StatusOverlay::new(Rect::FULL, view.caption, StatusKind::Failed)
                    .page(Icon::Alert)
                    .row(&[msg::app_player_choose_stream_c(), msg::app_player_try_again_c()]);
                o.focus = Some(self.fail_focus);
                if !view.reason.is_empty() {
                    o = o.reason(view.reason);
                }
                Some(o)
            }
            _ => None,
        }
    }

    /// The controls a pointer can reach this frame, and where.
    fn stops(n: &Now, measure: &dyn Measure) -> Vec<(u32, Rect)> {
        let mut out = vec![(ELEM_SCRUB, scrub_rect())];
        if n.offered {
            for (i, r) in up_next_rects(measure).into_iter().enumerate() {
                out.push((ELEM_UP_NEXT + i as u32, r));
            }
        } else {
            for i in 0..hud::DISCS {
                out.push((ELEM_DISC + u32::from(i), disc_rect(i)));
            }
            if n.episodes {
                out.push((ELEM_EPISODES, episodes_rect(measure)));
            }
        }
        out
    }
}

impl LogicalState for PlayerScreen {
    fn write(&self, c: &mut Canon) {
        let focus = match self.ctl.hud.focus {
            Focus::Scrub => 0,
            Focus::Disc(i) => 0x10 + u32::from(i),
            Focus::UpNext(i) => 0x20 + u32::from(i),
            Focus::Skip => 0x30,
            Focus::Episodes => 0x40,
        };
        c.u32(self.entry.0).bool(self.stopped).bool(self.covered).bool(self.declined).u32(focus);
        c.option(self.ctl.scrub.preview, |c, p| {
            c.u64(p);
        });
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("player focus={:?} scrub={:?} stopped={}", self.ctl.hud.focus, self.ctl.scrub.preview, self.stopped));
    }
}

impl Machine<Bundle> for PlayerScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        self.hold_meta(cx);
        match ev {
            ScreenEvent::Mount => {
                self.ctl = Controller::new(cx.tick.ms);
                // the work's meta: an episode's for Up Next and its Episodes, any's for Show info
                if let Some(item) = self.arg.item.as_ref().filter(|i| !i.id.is_empty()) {
                    let key = MetaKey { kind: item.kind.clone(), id: item.id.clone() };
                    fx.push(Fx::App(AppFx::Detail(DetailCmd::OpenMeta(key.clone()))));
                    self.meta = Some(key);
                }
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                let n = self.now(cx);
                let a = self.ctl.tick(&n);
                self.act(a, cx, fx);
                let drawn = self.hud_drawn(&n, cx);
                let row_focus = match self.ctl.hud.focus {
                    Focus::Disc(i) | Focus::UpNext(i) if drawn => Some(usize::from(i)),
                    Focus::Episodes if drawn => Some(usize::from(hud::DISCS)),
                    _ => None,
                };
                self.pop.step(row_focus, t.dt());
                // the HUD fades in and out; the scrubber's knob grows with focus and a scrub
                let eased = (self.hud_a, self.bar_focus, self.bar_scrub);
                self.hud_a = ease(self.hud_a, if drawn { 1.0 } else { 0.0 }, t.dt(), HUD_FADE_S);
                self.bar_focus = ease(self.bar_focus, if drawn && self.ctl.hud.focus == Focus::Scrub { 1.0 } else { 0.0 }, t.dt(), BAR_EASE_S);
                self.bar_scrub = ease(self.bar_scrub, if self.ctl.scrub.live() { 1.0 } else { 0.0 }, t.dt(), BAR_EASE_S);
                if eased != (self.hud_a, self.bar_focus, self.bar_scrub) {
                    fx.note(PresentEvent::Motion);
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                // a stall with the picture up: its start, for the centre plate after a beat
                let stalling = cx.views.playback.phase == PlayPhase::Working && cx.views.playback.clock.picture;
                self.stall_since = if stalling { self.stall_since.or(Some(cx.tick.ms)) } else { None };
                if stalling {
                    fx.note(PresentEvent::Motion);
                }
                // a resumed playback offers Start over once its first picture is up
                if self.start_over.is_none() && self.arg.resumed_ms > 0 && n.picture {
                    self.start_over = Some(cx.tick.ms.wrapping_add(START_OVER_MS).max(1));
                }
                // the centre badge eases in while paused (with a picture), out on resume, with a
                // flash of the play glyph
                let phase = cx.views.playback.phase;
                let paused = n.paused && n.picture;
                if self.was_paused && phase == PlayPhase::Playing {
                    self.flash = 1.0;
                }
                self.was_paused = paused;
                let (badge, flash) = (self.badge, self.flash);
                self.badge = ease(self.badge, if paused { 1.0 } else { 0.0 }, t.dt(), BADGE_IN_S);
                self.flash = if paused { 0.0 } else { ease(self.flash, 0.0, t.dt(), FLASH_S) };
                if self.badge != badge || self.flash != flash {
                    fx.note(PresentEvent::Motion);
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                // the offer asks for the next episode's same release, so it is ready at the end
                if n.offered && self.picked.is_none() {
                    if let Some(next) = self.next(cx) {
                        let _ = self.same_release(&next, cx, fx);
                    }
                }
                // an episode asked for while its streams were on their way; else the stream played
                // to its end: the next episode, or back
                if let Some(to) = self.advancing.clone().filter(|_| !self.leaving) {
                    self.go_to(to, cx, fx);
                } else if cx.views.playback.clock.ended && !self.leaving {
                    // Auto-play next episode off: the end returns to where it was played from
                    if cx.views.settings.get(Pref::AutoPlay) == 0 {
                        self.leave(None, fx);
                    } else {
                        self.advance(cx, fx);
                    }
                }
                let shown = self.fingerprint(&n, cx);
                if std::mem::replace(&mut self.shown, shown) != shown {
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                // a moving preview, a spinner and the working read-out turn from the frame clock
                let working = cx.views.playback.phase == PlayPhase::Working && !cx.views.playback.clock.picture;
                // GREEN held with no repeat to say so
                self.green_held(cx, fx);
                // the subtitle toggle's note, once the playback says what it landed on
                if let SubNote::Waiting { was, until } = self.sub_note {
                    let media = &cx.views.playback.media;
                    if media.sub_sel != was || (cx.tick.ms.wrapping_sub(until) as i32) >= 0 {
                        self.sub_note = SubNote::Shown { text: sub_note_text(media), until: cx.tick.ms.wrapping_add(SUB_NOTE_MS) };
                        fx.invalidate(Provenance::Landing(fx.from()));
                    }
                } else if matches!(self.sub_note, SubNote::Shown { .. }) && self.sub_note(cx.tick.ms).is_none() {
                    self.sub_note = SubNote::None;
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                let speeding = self.ctl.scrub.speed().is_some();
                if (drawn && (self.ctl.scrub.holding() || speeding || cx.views.playback.clock.seeking || self.ctl.disc_held())) || working {
                    fx.note(PresentEvent::Motion);
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(o, _) => {
                if *o == StoreId::Playback.ord() || *o == StoreId::Detail.ord() || *o == StoreId::Epg.ord() {
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                Handled::No
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key, sym, wcode, edge, .. }, .. }) => match player_key(*sym, *wcode) {
                Some(k) => self.player_key(k, *edge, cx, fx),
                None => self.key(classify_input(*key, *sym, *wcode), *edge, cx, fx),
            },
            ScreenEvent::Input(InputEvent { kind, .. }) => self.pointer(kind, cx, fx),
            ScreenEvent::Activate(elem) if cx.views.playback.phase == PlayPhase::Failed => {
                if (ELEM_FAIL..ELEM_FAIL + FAIL_ACTIONS as u32).contains(elem) {
                    self.fail_act((elem - ELEM_FAIL) as usize, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                let n = self.now(cx);
                // the Skip button also stands alone, over a hidden HUD
                let reachable = |f: &Focus| {
                    self.hud_drawn(&n, cx) || (*f == Focus::Skip && cx.views.playback.phase != PlayPhase::Failed && self.ctl.slot_shown(&n))
                };
                if let Some(f) = Self::focus_of(*elem).filter(reachable) {
                    let a = self.ctl.click(f, &n);
                    self.act(a, cx, fx);
                }
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::Chose(target, i)) => {
                self.chose(*target, *i, cx, fx);
                Handled::Yes
            }
            ScreenEvent::Cover => {
                self.covered = true;
                Handled::Yes
            }
            ScreenEvent::Uncover => {
                self.covered = false;
                self.open_disc = None;
                self.ctl.hud.wake(cx.tick.ms);
                Handled::Yes
            }
            ScreenEvent::WillLeave(Leave::ForGood) | ScreenEvent::Unmount => {
                self.stop(fx);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

impl Focusable<Bundle> for PlayerScreen {
    fn groups(&self, _cx: &Cx<'_, Bundle>, _out: &mut Vec<GroupSpec>) {}
    fn group_of(&self, _key: &u32, _cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        None
    }
    fn neighbour(&self, _key: FocusKey<u32>, _dir: Dir, _cx: &Cx<'_, Bundle>) -> Step<u32> {
        Step::Edge
    }
    fn place(&self, _key: &u32, _cx: &Cx<'_, Bundle>, _at: At) -> Option<Placed> {
        None
    }
    fn reconcile(&self, want: FocusKey<u32>, _cx: &Cx<'_, Bundle>) -> FocusKey<u32> {
        want
    }
    fn seat(&self, _g: GroupId, _from: Placed, _cx: &Cx<'_, Bundle>) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem: ELEM_SCRUB }
    }
}

impl Screen<Bundle> for PlayerScreen {
    fn name(&self) -> &'static str {
        word::PLAYER
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        // a hole the plane shows through, never a sheet over it
        sn_gfx::gfx::frame_clear_through();
        let n = self.now(f.cx);
        let drawn = self.hud_drawn(&n, f.cx);
        let p = f.painter.alpha(f.page_alpha);
        let failed = f.cx.views.playback.phase == PlayPhase::Failed;
        // the pause dim goes UNDER the captions, so they stay crisp over it
        if self.badge > 0.0 && !failed {
            p.rect(Rect::FULL, 0.0, theme::scrim_black(PAUSE_DIM_A * self.badge), theme::scrim_black(PAUSE_DIM_A * self.badge), 0.0);
        }
        if let Some(paint) = SUBTITLES.get() {
            paint(drawn, f.measure);
        }
        if !failed {
            if self.stalled(n.ms) && self.ctl.scrub.preview.is_none() {
                Self::draw_stall(p, f.cx, f.measure);
            } else {
                self.draw_centre(p, &n, f.cx, f.measure, drawn);
            }
        }
        if !failed && self.hud_a > 0.0 {
            self.draw_hud(p.alpha(self.hud_a), &n, f.cx, f.measure);
        }
        if drawn {
            if f.records_stops() {
                for (elem, rect) in Self::stops(&n, f.measure) {
                    f.stop(
                        p,
                        Stop {
                            key: FocusKey { entry: self.entry, elem },
                            rect,
                            rest_rect: rect,
                            clip: Rect::FULL,
                            hover: Hover::Ignore,
                            activate: Activate::Direct,
                        },
                    );
                }
            }
        }
        if f.cx.views.playback.phase != PlayPhase::Failed {
            if let Some(s) = self.ctl.skip_shown(&n) {
                let label = skip_label(s.kind);
                let rect = skip_rect(label, false, f.measure);
                let c = CString::new(label).unwrap_or_default();
                Button::new(c.as_ptr(), UP_NEXT_SZ, rect)
                    .focused(self.ctl.hud.focus == Focus::Skip)
                    .ground(ControlGround::Unkeyed)
                    .draw(&Env::inert(), p);
                if f.records_stops() {
                    let key = FocusKey { entry: self.entry, elem: ELEM_SKIP };
                    f.stop(p, Stop { key, rect, rest_rect: rect, clip: Rect::FULL, hover: Hover::Ignore, activate: Activate::Direct });
                }
            } else if n.start_over {
                let label = msg::app_player_start_over();
                let rect = skip_rect(label, true, f.measure);
                let c = CString::new(label).unwrap_or_default();
                Button::new(c.as_ptr(), UP_NEXT_SZ, rect)
                    .icon(Icon::Restart)
                    .focused(self.ctl.hud.focus == Focus::Skip)
                    .ground(ControlGround::Unkeyed)
                    .draw(&Env::inert(), p);
                if f.records_stops() {
                    let key = FocusKey { entry: self.entry, elem: ELEM_SKIP };
                    f.stop(p, Stop { key, rect, rest_rect: rect, clip: Rect::FULL, hover: Hover::Ignore, activate: Activate::Direct });
                }
            } else if let Some(text) = self.sub_note(n.ms) {
                let rect = skip_rect(text, false, f.measure);
                let c = CString::new(text).unwrap_or_default();
                Button::new(c.as_ptr(), UP_NEXT_SZ, rect).icon(Icon::Cc).ground(ControlGround::Unkeyed).draw(&Env::inert(), p);
            } else if let Some(kind) = self.ctl.note(&n) {
                let label = skipped_label(kind);
                let c = CString::new(label).unwrap_or_default();
                Button::new(c.as_ptr(), UP_NEXT_SZ, skip_rect(label, true, f.measure))
                    .icon(Icon::Check)
                    .ground(ControlGround::Unkeyed)
                    .draw(&Env::inert(), p);
            }
        }
        if f.cx.views.playback.phase == PlayPhase::Working && !f.cx.views.playback.clock.picture {
            Self::draw_working(p, f.cx, f.measure);
        } else if let Some(overlay) = self.overlay(f.cx) {
            overlay.draw_measured(&Env::inert(), p, f.measure);
            if f.records_stops() {
                for (i, rect) in overlay.row_frames_measured(f.measure).into_iter().enumerate() {
                    let Some(rect) = rect else { continue };
                    let key = FocusKey { entry: self.entry, elem: ELEM_FAIL + i as u32 };
                    f.stop(p, Stop { key, rect, rest_rect: rect, clip: Rect::FULL, hover: Hover::Ignore, activate: Activate::Direct });
                }
            }
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::VideoPlane
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests;
