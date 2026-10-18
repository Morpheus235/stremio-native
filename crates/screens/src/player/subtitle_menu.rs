//! **The subtitle menu** — every subtitle option in one popover over the player, on three tabs
//! along its top ([`Tab`]): UP off the top of a column reaches the strip, LEFT/RIGHT walk it and
//! the tab follows the cursor, DOWN enters the tab.
//!
//! **Tracks** — Stremio's two columns:
//!
//! - **Left, the groups** ([`groups`]): *Off*; *Embedded*, the file's own tracks (when it has
//!   any); one row per language the file's tracks and the addons' subtitles are in — the viewer's
//!   subtitle languages first, in their order, then the rest by name — each with its count; and
//!   *Other* for addon subtitles that name no language.
//! - **Right, the variants** of the group the left column is on ([`variants`]): the file's tracks
//!   in that language first, then each addon's, by addon, named by the addon with its release and
//!   its marks (Forced, SDH).
//! - **Where it opens** ([`opening`]): on the group of the subtitle in use (a file track's is
//!   *Embedded*); with none in use, on *Embedded* when the file has tracks, else on the first of
//!   the viewer's languages on offer — the original subtitles are one OK away.
//!
//! UP/DOWN walk a column, RIGHT (or OK on a group) crosses to its variants, LEFT comes back, OK
//! on a variant shows it and closes, OK on *Off* turns subtitles off.
//!
//! **Timing** ([`timing_form`]) — the delay in force, then a second and a quarter-second earlier
//! or later ([`DELAY_STEPS`]) and *Reset*; a step applies at once and the menu stays open (the
//! player clamps the delay to what it can show).
//!
//! **Style** ([`style_form`]) — size, position, text colour, outline and background: THE subtitle
//! preferences Settings shows too ([`STYLE_PREFS`]); LEFT/RIGHT step a row's value, OK goes round,
//! and each step is set at once, so the caption on the picture changes under the menu.
//!
//! BACK (or a GREEN press, the key that opened it) closes. Every row and tab is a pointer stop
//! too. The rows are re-read off the playback every tick, so an addon's answer lands in the open
//! menu; rows are keyed by identity (a file track's index, an addon subtitle's
//! [`AddonSubFacts::key`], a language's code), so a landing never moves the focus or the check,
//! and a pick asks the playback by that identity (`PlayerCmd::AddonSubtitle`).

use std::borrow::Cow;
use std::convert::Infallible;

use sn_base::tracks::{language_code, language_name};
use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind, Key, LogicalState, Machine, NavOp,
};
use sn_platform::i18n::msg;
use sn_ui::consts::{MARGIN_X, SCR_W};
use sn_ui::form::{Form, FormId, FormSection, FormTable, RowKey, RowKind};
use sn_ui::screen::{
    Activate, At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, FocusTarget, Focusable, GroupKind, GroupSpec, Hover,
    Placed, RenderStrategy, Scrim, Screen, ScreenEvent, Seat, Step, Stop,
};
use sn_ui::table::{Row, Section};
use sn_ui::{theme, Rect, View};

use super::tracks::{delay_label, track_label};
use crate::registry::{word, AddonSubFacts, AppFx, Bundle, MediaFacts, MenuArg, PlayerCmd, SettingsCmd, Views};
use crate::stores::settings::{Pref, SettingsStore};

const PANEL_RAD: f32 = theme::radius::PANEL;
/// The panel's keep-out from the top of the screen.
const EDGE: f32 = theme::space::XL;

// ---- the model (pure) --------------------------------------------------------------------------

/// A row of the left column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Group {
    Off,
    /// The file's own tracks, every language.
    Embedded,
    /// One language, by its code (`sn_base::tracks::language_code`, at most three letters).
    Lang(String),
    /// Addon subtitles that name no language.
    Other,
}

/// A row of the right column: what a pick shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// The `n`-th of the file's tracks.
    Embedded(usize),
    /// The addon subtitle with this key.
    Addon(u32),
}

/// One left row: the group, its name and how many subtitles it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupRow {
    pub group: Group,
    pub label: String,
    pub count: usize,
}

/// The language a tag names as a group's code; `None` for no language (`und`, untagged). An
/// unknown code keeps three letters, which is what its row key is made from.
fn code(tag: &str) -> Option<String> {
    language_name(tag)?;
    Some(language_code(tag).chars().filter(char::is_ascii_alphabetic).take(3).collect::<String>()).filter(|c| !c.is_empty())
}

/// The subtitle in use.
pub fn current(media: &MediaFacts) -> Option<Variant> {
    let i = media.sub_sel?;
    if i < media.subs.len() {
        return Some(Variant::Embedded(i));
    }
    media.addon_subs.get(i - media.subs.len()).map(|s| Variant::Addon(s.key))
}

fn addon<'a>(media: &'a MediaFacts, key: u32) -> Option<&'a AddonSubFacts> {
    media.addon_subs.iter().find(|s| s.key == key)
}

/// Is `v` among `group`'s variants?
fn holds(media: &MediaFacts, group: &Group, v: Variant) -> bool {
    match (group, v) {
        (Group::Embedded, Variant::Embedded(i)) => i < media.subs.len(),
        (Group::Lang(c), Variant::Embedded(i)) => media.subs.get(i).and_then(|t| code(&t.language)).as_ref() == Some(c),
        (Group::Lang(c), Variant::Addon(k)) => addon(media, k).and_then(|s| code(&s.language)).as_ref() == Some(c),
        (Group::Other, Variant::Addon(k)) => addon(media, k).is_some_and(|s| code(&s.language).is_none()),
        _ => false,
    }
}

/// The right column for `group`: the file's tracks first, in file order, then the addons',
/// addon by addon in the order the addons first answered, each addon's in its own order.
pub fn variants(media: &MediaFacts, group: &Group) -> Vec<Variant> {
    let mut out: Vec<Variant> = (0..media.subs.len()).map(Variant::Embedded).filter(|v| holds(media, group, *v)).collect();
    let mut addons: Vec<&str> = Vec::new();
    for s in &media.addon_subs {
        if !addons.contains(&s.addon.as_str()) {
            addons.push(&s.addon);
        }
    }
    for a in addons {
        out.extend(media.addon_subs.iter().filter(|s| s.addon == a).map(|s| Variant::Addon(s.key)).filter(|v| holds(media, group, *v)));
    }
    out
}

/// The left column: Off, Embedded (when the file has tracks), the languages (`prefer`'s first,
/// in its order, then by name), Other (when an addon names no language), the delay.
pub fn groups(media: &MediaFacts, prefer: &[String]) -> Vec<GroupRow> {
    let mut langs: Vec<(String, String)> = Vec::new();
    let tags = media.subs.iter().map(|t| t.language.as_str()).chain(media.addon_subs.iter().map(|s| s.language.as_str()));
    for tag in tags {
        if let Some(c) = code(tag) {
            if !langs.iter().any(|(have, _)| *have == c) {
                langs.push((c, language_name(tag).unwrap_or_default()));
            }
        }
    }
    let rank = |c: &str| prefer.iter().position(|p| code(p).as_deref() == Some(c)).unwrap_or(prefer.len());
    langs.sort_by(|(a, an), (b, bn)| rank(a).cmp(&rank(b)).then_with(|| an.to_lowercase().cmp(&bn.to_lowercase())));
    let row = |group: Group, label: String| GroupRow { count: variants(media, &group).len(), group, label };
    let mut out = vec![GroupRow { group: Group::Off, label: msg::app_player_subtitles_off().to_string(), count: 0 }];
    if !media.subs.is_empty() {
        out.push(row(Group::Embedded, msg::app_player_subtitles_embedded().to_string()));
    }
    out.extend(langs.into_iter().map(|(c, name)| row(Group::Lang(c), name)));
    if media.addon_subs.iter().any(|s| code(&s.language).is_none()) {
        out.push(row(Group::Other, msg::app_player_subtitles_other().to_string()));
    }
    out
}

/// The group the menu opens on: the one holding the subtitle in use (a file track's is
/// Embedded); none in use, Embedded when the file has tracks, else the first language on offer
/// that the viewer reads, else the first language, else Off.
pub fn opening(media: &MediaFacts, prefer: &[String]) -> Group {
    let rows = groups(media, prefer);
    match current(media) {
        Some(Variant::Embedded(_)) => return Group::Embedded,
        Some(v) => {
            if let Some(r) = rows.iter().find(|r| holds(media, &r.group, v)) {
                return r.group.clone();
            }
        }
        None if !media.subs.is_empty() => return Group::Embedded,
        None => {}
    }
    let preferred = |r: &&GroupRow| matches!(&r.group, Group::Lang(c) if prefer.iter().any(|p| code(p).as_deref() == Some(c.as_str())));
    let first_lang = |r: &&GroupRow| matches!(r.group, Group::Lang(_));
    rows.iter().find(preferred).or_else(|| rows.iter().find(first_lang)).map_or(Group::Off, |r| r.group.clone())
}

/// Does the group have a right column (a pick lives there)?
fn has_variants(g: &Group) -> bool {
    matches!(g, Group::Embedded | Group::Lang(_) | Group::Other)
}

/// A variant's marks: `Forced · SDH`. App text.
fn marks(s: &AddonSubFacts) -> String {
    let mut m: Vec<&str> = Vec::new();
    if s.forced {
        m.push(msg::app_player_subtitle_forced());
    }
    if s.hearing_impaired {
        m.push(msg::app_player_subtitle_sdh());
    }
    m.join(" \u{b7} ")
}

// ---- the rows ----------------------------------------------------------------------------------

/// Left keys are below this; right keys from it.
const LEFT_BAND: u32 = 0x5000;
const RIGHT_BAND: u32 = 0x1_0000;

impl FormId for Group {
    fn key(&self) -> RowKey {
        RowKey(match self {
            Group::Off => 1,
            Group::Embedded => 2,
            Group::Other => 3,
            // three letters, base 27: one key per code (`code` keeps at most three)
            Group::Lang(c) => 0x100 + c.bytes().fold(0, |k, b| k * 27 + u32::from(b.to_ascii_uppercase().saturating_sub(b'A') % 26 + 1)),
        })
    }
}

impl FormId for Variant {
    fn key(&self) -> RowKey {
        RowKey(match self {
            Variant::Embedded(i) => LEFT_BAND + (*i as u32).min(0xFFF),
            // ponytail: keys past 0x9FFF in one session share the last slot; a session lands a
            // few dozen
            Variant::Addon(k) => LEFT_BAND + 0x1000 + (*k).min(0x9FFF),
        })
    }
}

type LeftForm = Form<Group, Group, Infallible>;
type RightForm = Form<Variant, Variant, Infallible>;

/// The left column's rows. Off is checked while subtitles are off, the group of the subtitle in
/// use otherwise.
pub fn left_form(media: &MediaFacts, prefer: &[String]) -> LeftForm {
    let checked = match current(media) {
        None => Some(Group::Off),
        Some(Variant::Embedded(_)) => Some(Group::Embedded),
        Some(v) => groups(media, prefer).into_iter().map(|r| r.group).find(|g| holds(media, g, v)),
    };
    let mut picks = FormSection::from_head(Section::new(msg::app_player_subtitle_languages()));
    for r in groups(media, prefer) {
        let mut row = Row::new(r.label.clone());
        if matches!(r.group, Group::Lang(_)) {
            // a language's own name, or an unknown code as the file gives it
            row = row.server_label();
        }
        if has_variants(&r.group) {
            row = row.value(r.count.to_string());
        }
        let is = checked.clone();
        picks = picks.choice(r.group.clone(), r.group, row, move |id| Some(id) == is.as_ref());
    }
    Form::new().section(picks)
}

// ---- the tabs, Timing and Style -----------------------------------------------------------------

/// The menu's tabs: the tracks (the two columns), the delay, the caption's look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Tracks,
    Timing,
    Style,
}

pub const TABS: [Tab; 3] = [Tab::Tracks, Tab::Timing, Tab::Style];

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Tracks => msg::app_player_tab_tracks(),
            Tab::Timing => msg::app_player_tab_timing(),
            Tab::Style => msg::app_player_tab_style(),
        }
    }
}

/// The tab strip's element keys (`TAB_KEY + n`), and the Timing / Style rows' band, both clear of
/// the two columns'.
const TAB_KEY: u32 = 0x3_0000;
const OPT_BAND: u32 = 0x4_0000;
/// The tab strip's band at the panel's top.
const TAB_BAND: f32 = sn_ui::widgets::TAB_PILL_H + 2.0 * theme::space::SM;
/// The tab pills' type.
const TAB_SZ: i32 = theme::size::LABEL;

/// The Timing steps (ms): a second and a quarter-second either way, the player's own clamp keeping
/// the delay inside the range it can show (`sn_media::player::subtitle_offset_range_ms`).
pub const DELAY_STEPS: [i64; 4] = [-1000, -super::tracks::DELAY_STEP_MS, super::tracks::DELAY_STEP_MS, 1000];

/// The caption's look the Style tab sets: THE subtitle preferences (Settings keeps the same ones).
pub const STYLE_PREFS: [Pref; 5] = [Pref::SubtitleSize, Pref::SubtitlePosition, Pref::SubtitleColor, Pref::SubtitleOutline, Pref::SubtitleBackground];

/// A row of the Timing or the Style tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opt {
    /// Move the delay by this many ms.
    Step(i64),
    /// No delay.
    Reset,
    /// One preference of the caption's look: LEFT/RIGHT step its rungs, OK the next one.
    Style(Pref),
}

impl FormId for Opt {
    fn key(&self) -> RowKey {
        RowKey(OPT_BAND + match self {
            Opt::Step(ms) => 1 + DELAY_STEPS.iter().position(|s| s == ms).unwrap_or(0) as u32,
            Opt::Reset => 8,
            Opt::Style(p) => 0x10 + *p as u32,
        })
    }
}

type OptForm = Form<Opt, Opt, Infallible>;

/// The Timing tab: the delay in force as its header, then the steps and Reset.
pub fn timing_form(delay_ms: i64) -> OptForm {
    let mut s = FormSection::new(format!("{} \u{b7} {}", msg::app_player_subtitle_delay(), delay_label(delay_ms)));
    for step in DELAY_STEPS {
        let label = if step < 0 { msg::app_player_subtitle_earlier() } else { msg::app_player_subtitle_later() };
        s = s.item(Opt::Step(step), RowKind::Button, Opt::Step(step), Row::new(label).value(delay_label(step)));
    }
    s = s.item(Opt::Reset, RowKind::Button, Opt::Reset, Row::new(msg::app_player_subtitle_delay_reset()).value(delay_label(0)));
    Form::new().section(s)
}

/// The Style tab: each preference with its rung between the arrows LEFT/RIGHT move it by.
pub fn style_form(settings: &SettingsStore) -> OptForm {
    let mut s = FormSection::new(msg::app_player_tab_style());
    for p in STYLE_PREFS {
        let value = format!("\u{2039} {} \u{203a}", p.rung_label(settings.get(p)));
        s = s.item(Opt::Style(p), RowKind::Button, Opt::Style(p), Row::new(p.label()).value(value));
    }
    Form::new().section(s.note(msg::app_player_style_note()))
}

/// The rung a step of `delta` (±1) takes preference `p` to from `at`: clamped to its ladder; OK
/// (`wrap`) goes round from the last to the first.
pub fn style_step(p: Pref, at: u8, delta: i8, wrap: bool) -> u8 {
    let n = p.rungs();
    match (delta, wrap) {
        (d, true) if d > 0 => (at + 1) % n,
        (d, _) if d > 0 => (at + 1).min(n - 1),
        _ => at.saturating_sub(1),
    }
}

/// The right column for `group`, the subtitle in use checked.
pub fn right_form(media: &MediaFacts, group: &Group, title: &str) -> RightForm {
    let now = current(media);
    let mut s = FormSection::from_head(Section::new(title.to_string()).server_header());
    for v in variants(media, group) {
        let row = match v {
            Variant::Embedded(i) => {
                let row = Row::new(track_label(&media.subs[i], i + 1, false)).server_label();
                // among a language's subtitles the file's own say so; the Embedded group is all of them
                if *group == Group::Embedded { row } else { row.value(msg::app_player_subtitles_embedded()) }
            }
            Variant::Addon(k) => {
                let Some(a) = addon(media, k) else { continue };
                let mut row = Row::new(a.addon.clone()).server_label();
                let marks = marks(a);
                if !marks.is_empty() {
                    row = row.value(marks);
                }
                if !a.release.is_empty() {
                    row = row.detail(a.release.clone()).server_detail();
                }
                row
            }
        };
        s = s.choice(v, v, row, move |id| Some(*id) == now);
    }
    Form::new().section(s)
}

// ---- the screen --------------------------------------------------------------------------------

/// What the rows were built from; a change rebuilds them.
#[derive(Clone, PartialEq)]
struct Built {
    media: MediaFacts,
    prefer: Vec<String>,
    delay_ms: i64,
    /// The Style tab's rungs, in [`STYLE_PREFS`] order.
    look: [u8; 5],
}

impl Built {
    fn of(views: &Views<'_>) -> Self {
        Built {
            media: views.playback.media.clone(),
            prefer: views.settings.track_prefs().subtitles,
            delay_ms: views.playback.sub_delay_ms,
            look: STYLE_PREFS.map(|p| views.settings.get(p)),
        }
    }
}

/// Which part of the menu an element is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Tab(usize),
    Left,
    Right,
    Opt,
}

fn part_of(elem: u32) -> Part {
    match elem {
        e if (TAB_KEY..TAB_KEY + TABS.len() as u32).contains(&e) => Part::Tab((e - TAB_KEY) as usize),
        e if e >= OPT_BAND => Part::Opt,
        e if e < LEFT_BAND => Part::Left,
        _ => Part::Right,
    }
}

const LEFT_GROUP: GroupId = GroupId(0);
const RIGHT_GROUP: GroupId = GroupId(1);
const TAB_GROUP: GroupId = GroupId(2);
const OPT_GROUP: GroupId = GroupId(3);

pub struct SubtitleMenu {
    entry: EntryId,
    arg: MenuArg,
    /// The tab shown: the tracks' two columns, or the Timing / Style rows.
    tab: Tab,
    left: FormTable<Group, Group, Infallible>,
    right: FormTable<Variant, Variant, Infallible>,
    /// The Timing or the Style tab's rows.
    opts: FormTable<Opt, Opt, Infallible>,
    /// The group the right column shows: the left column's focus, kept while focus is across.
    shown: Group,
    built: Built,
    /// The right column's tallest content over every group, and the taller of the Timing and
    /// Style tabs, so the panel holds its height while the left column walks the languages and
    /// while the tabs change.
    right_h: f32,
    opts_h: f32,
}

impl SubtitleMenu {
    pub fn new(entry: EntryId, arg: MenuArg, views: &Views<'_>) -> Self {
        let built = Built::of(views);
        let mut left = FormTable::new(LEFT_BAND);
        let mut right = FormTable::new(RIGHT_BAND);
        let mut opts = FormTable::new(KEY_CEIL);
        left.table.compact = true;
        right.table.compact = true;
        opts.table.compact = true;
        right.table.list_focused = false;
        let shown = opening(&built.media, &built.prefer);
        left.open(left_form(&built.media, &built.prefer), Some(&shown));
        let opts_h = [timing_form(built.delay_ms), style_form(views.settings)]
            .into_iter()
            .map(|f| {
                let mut t = FormTable::<Opt, Opt, Infallible>::new(KEY_CEIL);
                t.table.compact = true;
                t.set(f, None);
                t.table.measured_height()
            })
            .fold(0.0, f32::max);
        let mut menu = SubtitleMenu { entry, arg, tab: Tab::Tracks, left, right, opts, shown, built, right_h: 0.0, opts_h };
        menu.fill_right(true);
        menu
    }

    /// The left column's table (the tests read its rows).
    pub fn left(&self) -> &sn_ui::table::TableView {
        &self.left.table
    }

    /// The right column's table.
    pub fn right(&self) -> &sn_ui::table::TableView {
        &self.right.table
    }

    /// The Timing or Style tab's table.
    pub fn opts(&self) -> &sn_ui::table::TableView {
        &self.opts.table
    }

    /// The tab shown.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The group the right column shows.
    pub fn shown(&self) -> &Group {
        &self.shown
    }

    fn title(&self) -> String {
        groups(&self.built.media, &self.built.prefer).into_iter().find(|r| r.group == self.shown).map(|r| r.label).unwrap_or_default()
    }

    /// (Re)build the right column for [`Self::shown`]: `fresh` opens on the subtitle in use (else
    /// the first), otherwise the focused variant is kept by identity.
    fn fill_right(&mut self, fresh: bool) {
        let form = right_form(&self.built.media, &self.shown, &self.title());
        let now = current(&self.built.media).filter(|v| holds(&self.built.media, &self.shown, *v));
        if fresh {
            self.right.open(form, now.as_ref());
        } else {
            let keep = self.right.selected_id().copied();
            self.right.set_sliding(form, keep.as_ref().or(now.as_ref()));
        }
        let mut tallest = self.right.table.measured_height();
        for r in groups(&self.built.media, &self.built.prefer).iter().filter(|r| has_variants(&r.group)) {
            let mut t = FormTable::<Variant, Variant, Infallible>::new(RIGHT_BAND);
            t.table.compact = true;
            t.set(right_form(&self.built.media, &r.group, &r.label), None);
            tallest = tallest.max(t.table.measured_height());
        }
        self.right_h = tallest;
    }

    /// The Timing or the Style rows for the tab shown, the focused row kept by identity.
    fn fill_opts(&mut self, settings: &SettingsStore, fresh: bool) {
        let form = match self.tab {
            Tab::Timing => timing_form(self.built.delay_ms),
            _ => style_form(settings),
        };
        if fresh {
            self.opts.open(form, None);
        } else {
            let keep = self.opts.selected_id().copied();
            self.opts.set_sliding(form, keep.as_ref());
        }
    }

    /// Show tab `t`.
    fn set_tab(&mut self, t: Tab, settings: &SettingsStore) {
        if t == self.tab {
            return;
        }
        self.tab = t;
        if t != Tab::Tracks {
            self.fill_opts(settings, true);
        }
    }

    /// The panel and its two columns under the tab strip: the groups at their token width on the
    /// left, the variants at theirs on the right, a hairline between; right-aligned above the
    /// control row like every player menu. The Timing and Style rows take both columns' width.
    fn frames(&self) -> (Rect, Rect, Rect) {
        let lw = theme::layout::SUBTITLE_MENU_GROUPS_W;
        let rw = theme::layout::SUBTITLE_MENU_VARIANTS_W;
        let bottom = theme::layout::PLAYER_MENU_BOTTOM;
        let content = self.left.table.measured_height().max(self.right_h).max(self.opts_h);
        let h = (content + TAB_BAND).clamp(120.0, bottom - EDGE);
        let panel = Rect::new(SCR_W - MARGIN_X - lw - rw, bottom - h, lw + rw, h);
        let ch = h - TAB_BAND;
        (panel, Rect::new(panel.x, panel.y + TAB_BAND, lw, ch), Rect::new(panel.x + lw, panel.y + TAB_BAND, rw, ch))
    }

    /// The Timing / Style rows' frame: both columns.
    fn opts_frame(&self) -> Rect {
        let (_, l, r) = self.frames();
        Rect::new(l.x, l.y, l.w + r.w, l.h)
    }

    /// The tab pills, left to right along the strip.
    fn tab_rects(&self, measure: &dyn sn_machine::machine::Measure) -> Vec<Rect> {
        let (panel, _, _) = self.frames();
        let mut x = panel.x + theme::space::MD;
        let y = panel.y + theme::space::SM;
        TABS.iter()
            .map(|t| {
                let w = sn_ui::widgets::TabPill::width_measured(t.label(), TAB_SZ, measure);
                let r = Rect::new(x, y, w, sn_ui::widgets::TAB_PILL_H);
                x += w + theme::space::SM;
                r
            })
            .collect()
    }

    /// Where the row keyed `elem` is drawn, in either column or the tab's rows.
    pub fn row_rect(&self, elem: u32) -> Option<Rect> {
        let (_, l, r) = self.frames();
        match part_of(elem) {
            Part::Left => self.left.table.row_frame(l, self.left.index_of_key(RowKey(elem))? as i32),
            Part::Right => self.right.table.row_frame(r, self.right.index_of_key(RowKey(elem))? as i32),
            Part::Opt => self.opts.table.row_frame(self.opts_frame(), self.opts.index_of_key(RowKey(elem))? as i32),
            Part::Tab(_) => None,
        }
    }

    fn focus_key(&self, elem: u32) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem }
    }

    fn tab_key(&self) -> u32 {
        TAB_KEY + TABS.iter().position(|t| *t == self.tab).unwrap_or(0) as u32
    }

    /// The right column's seat: the focused variant, else the checked one, else the first.
    fn right_seat(&self) -> Option<u32> {
        self.right.selected_key().or_else(|| self.right.opening_key()).map(|k| k.0)
    }

    fn left_seat(&self) -> u32 {
        self.left.index_of(&self.shown).and_then(|i| self.left.key_at(i)).or_else(|| self.left.opening_key()).map_or(1, |k| k.0)
    }

    fn opts_seat(&self) -> Option<u32> {
        self.opts.selected_key().or_else(|| self.opts.opening_key()).map(|k| k.0)
    }

    /// Where DOWN from the tab strip lands: the tab's own content.
    fn content_seat(&self) -> Option<u32> {
        match self.tab {
            Tab::Tracks => Some(self.left_seat()),
            _ => self.opts_seat(),
        }
    }

    /// Re-read the playback and the settings; rebuild what changed, keeping every focus by identity.
    fn sync(&mut self, views: &Views<'_>) {
        let now = Built::of(views);
        if now == self.built {
            return;
        }
        let tracks = now.media != self.built.media || now.prefer != self.built.prefer;
        self.built = now;
        if tracks {
            let keep = self.left.selected_id().cloned();
            self.left.set_sliding(left_form(&self.built.media, &self.built.prefer), keep.as_ref());
            self.fill_right(false);
        }
        if self.tab != Tab::Tracks {
            self.fill_opts(views.settings, false);
        }
    }

    /// The left column's focus moved onto `g`: the right column follows it.
    fn follow(&mut self, g: Group) {
        if has_variants(&g) && g != self.shown {
            self.shown = g;
            self.fill_right(true);
        }
    }

    fn ask(&self, fx: &mut Effects<'_, Bundle>, cmd: PlayerCmd) {
        fx.push(Fx::App(AppFx::Player(cmd)));
        fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
    }

    /// A Style row stepped by `delta` (`wrap`: OK's step, round from the last rung): the
    /// preference set at once, so the caption on the picture changes under the menu.
    fn step_style(&self, p: Pref, delta: i8, wrap: bool, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let at = cx.views.settings.get(p);
        let to = style_step(p, at, delta, wrap);
        if to != at {
            fx.push(Fx::App(AppFx::Settings(SettingsCmd::Set(p, to))));
            fx.invalidate(sn_machine::present::Provenance::Input);
        }
    }

    fn activate(&mut self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        match part_of(elem) {
            Part::Tab(i) => {
                self.set_tab(TABS[i], cx.views.settings);
                fx.invalidate(sn_machine::present::Provenance::Input);
            }
            Part::Opt => match self.opts.index_of_key(RowKey(elem)).and_then(|i| self.opts.id_at(i)).copied() {
                // the delay moves and the menu stays: steps are fine-tuning
                Some(Opt::Step(ms)) => fx.push(Fx::App(AppFx::Player(PlayerCmd::SubtitleDelay(self.built.delay_ms + ms)))),
                Some(Opt::Reset) => fx.push(Fx::App(AppFx::Player(PlayerCmd::SubtitleDelay(0)))),
                Some(Opt::Style(p)) => self.step_style(p, 1, true, cx, fx),
                None => {}
            },
            Part::Left => {
                let Some(g) = self.left.index_of_key(RowKey(elem)).and_then(|i| self.left.id_at(i)).cloned() else { return };
                match g {
                    Group::Off => self.ask(fx, PlayerCmd::Subtitle(None)),
                    g => {
                        self.follow(g);
                        if let Some(k) = self.right_seat() {
                            crate::registry::reseat(FocusTarget::Elem(self.focus_key(k)), fx);
                        }
                    }
                }
            }
            Part::Right => match self.right.index_of_key(RowKey(elem)).and_then(|i| self.right.id_at(i)).copied() {
                Some(Variant::Embedded(i)) => self.ask(fx, PlayerCmd::Subtitle(Some(i))),
                Some(Variant::Addon(k)) => self.ask(fx, PlayerCmd::AddonSubtitle(k)),
                None => {}
            },
        }
    }

    /// Every stop the menu draws: the tabs, then the shown tab's rows.
    fn rows(&self) -> Vec<u32> {
        let mut out: Vec<u32> = (0..TABS.len() as u32).map(|i| TAB_KEY + i).collect();
        if self.tab == Tab::Tracks {
            out.extend((0..self.left.table.n_rows() as usize).filter_map(|i| self.left.key_at(i).map(|k| k.0)));
            out.extend((0..self.right.table.n_rows() as usize).filter_map(|i| self.right.key_at(i).map(|k| k.0)));
        } else {
            out.extend((0..self.opts.table.n_rows() as usize).filter_map(|i| self.opts.key_at(i).map(|k| k.0)));
        }
        out
    }

    /// Does `elem` stand in this menu now (a stale key of a tab not shown does not)?
    fn holds_elem(&self, elem: u32) -> bool {
        match part_of(elem) {
            Part::Tab(_) => true,
            Part::Left => self.tab == Tab::Tracks && self.left.index_of_key(RowKey(elem)).is_some(),
            Part::Right => self.tab == Tab::Tracks && self.right.index_of_key(RowKey(elem)).is_some(),
            Part::Opt => self.tab != Tab::Tracks && self.opts.index_of_key(RowKey(elem)).is_some(),
        }
    }
}

/// The Timing / Style table's key band: every key it makes is below this.
const KEY_CEIL: u32 = OPT_BAND + 0x100;

impl Machine<Bundle> for SubtitleMenu {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Mount => sn_ui::hold_hint::mark_learned(),
            ScreenEvent::Tick(t) => {
                self.sync(&cx.views);
                let focus = cx.focus.current.filter(|k| k.entry == self.entry).map(|k| k.elem);
                if let Some(e) = focus {
                    let at = part_of(e);
                    self.left.table.list_focused = at == Part::Left;
                    self.right.table.list_focused = at == Part::Right;
                    self.opts.table.list_focused = at == Part::Opt;
                    match at {
                        Part::Left => {
                            if let Some(i) = self.left.index_of_key(RowKey(e)) {
                                self.left.table.sel = i as i32;
                            }
                        }
                        Part::Right => {
                            if let Some(i) = self.right.index_of_key(RowKey(e)) {
                                self.right.table.sel = i as i32;
                            }
                        }
                        Part::Opt => {
                            if let Some(i) = self.opts.index_of_key(RowKey(e)) {
                                self.opts.table.sel = i as i32;
                            }
                        }
                        Part::Tab(_) => {}
                    }
                }
                let (_, l, r) = self.frames();
                self.left.table.update(t.dt(), l.h);
                self.right.table.update(t.dt(), r.h);
                self.opts.table.update(t.dt(), l.h);
            }
            ScreenEvent::FocusMoved { to, .. } => {
                match part_of(to.elem) {
                    // a tab strip switches as the cursor moves along it
                    Part::Tab(i) => self.set_tab(TABS[i], cx.views.settings),
                    Part::Left => {
                        if let Some(i) = self.left.index_of_key(RowKey(to.elem)) {
                            self.left.table.sel = i as i32;
                            if let Some(g) = self.left.id_at(i).cloned() {
                                self.follow(g);
                            }
                        }
                    }
                    Part::Right => {
                        if let Some(i) = self.right.index_of_key(RowKey(to.elem)) {
                            self.right.table.sel = i as i32;
                        }
                    }
                    Part::Opt => {
                        if let Some(i) = self.opts.index_of_key(RowKey(to.elem)) {
                            self.opts.table.sel = i as i32;
                        }
                    }
                }
                let at = part_of(to.elem);
                self.left.table.list_focused = at == Part::Left;
                self.right.table.list_focused = at == Part::Right;
                self.opts.table.list_focused = at == Part::Opt;
                fx.invalidate(sn_machine::present::Provenance::Input);
            }
            ScreenEvent::Activate(elem) => self.activate(*elem, cx, fx),
            ScreenEvent::PressCommit(_) => {
                if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry) {
                    self.activate(k.elem, cx, fx);
                }
            }
            ScreenEvent::Input(input) => {
                if let InputKind::Key { key: Key::Back, edge: Edge::Down, .. } = input.kind {
                    fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                    return Handled::Yes;
                }
                // a toggle on its disc's key: a GREEN press closes the menu a held GREEN opened (the
                // hold's own repeats and release arrive here and are spent: only a new press closes)
                if let InputKind::Key { wcode: crate::tabs::WCODE_GREEN, edge, .. } = input.kind {
                    if edge == Edge::Down {
                        fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                    }
                    return Handled::Yes;
                }
                // LEFT/RIGHT on a Style row step its preference (the row's own edge re-delivers them)
                if let InputKind::Key { key: key @ (Key::Left | Key::Right), edge: Edge::Down | Edge::Repeat, .. } = input.kind {
                    let row = cx.focus.current.filter(|k| k.entry == self.entry && part_of(k.elem) == Part::Opt);
                    if let Some(Opt::Style(p)) = row.and_then(|k| self.opts.index_of_key(RowKey(k.elem))).and_then(|i| self.opts.id_at(i)).copied() {
                        self.step_style(p, if key == Key::Right { 1 } else { -1 }, false, cx, fx);
                        return Handled::Yes;
                    }
                }
                return Handled::No;
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }
}

impl Focusable<Bundle> for SubtitleMenu {
    fn groups(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<GroupSpec>) {
        let (panel, l, r) = self.frames();
        let column = |id: GroupId, extent: Rect, len: usize, edge: [EdgeRule; 4]| GroupSpec {
            id,
            kind: GroupKind::Column,
            seat: Seat::Remembered,
            reachable: AxisMask::BOTH,
            edge,
            extent,
            len,
            elem: ElemKind::Bare,
        };
        out.push(GroupSpec {
            id: TAB_GROUP,
            kind: GroupKind::Row { wrap: false },
            seat: Seat::Remembered,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Stop; 4],
            extent: Rect::new(panel.x, panel.y, panel.w, TAB_BAND),
            len: TABS.len(),
            elem: ElemKind::Control,
        });
        let _ = cx;
        if self.tab == Tab::Tracks {
            out.push(column(LEFT_GROUP, l, self.left.focusable_len(), [EdgeRule::Stop; 4]));
            if self.right.focusable_len() > 0 {
                out.push(column(RIGHT_GROUP, r, self.right.focusable_len(), [EdgeRule::Stop; 4]));
            }
        } else if self.opts.focusable_len() > 0 {
            // LEFT/RIGHT come back to the page: a Style row steps its preference with them
            out.push(column(OPT_GROUP, self.opts_frame(), self.opts.focusable_len(), [EdgeRule::Stop, EdgeRule::Stop, EdgeRule::Screen, EdgeRule::Screen]));
        }
    }
    fn group_of(&self, elem: &u32, _: &Cx<'_, Bundle>) -> Option<GroupId> {
        if !self.holds_elem(*elem) {
            return None;
        }
        Some(match part_of(*elem) {
            Part::Tab(_) => TAB_GROUP,
            Part::Left => LEFT_GROUP,
            Part::Right => RIGHT_GROUP,
            Part::Opt => OPT_GROUP,
        })
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, _: &Cx<'_, Bundle>) -> Step<u32> {
        let at = part_of(key.elem);
        let to = match (at, dir) {
            (Part::Tab(i), Dir::Left) => i.checked_sub(1).map(|j| TAB_KEY + j as u32),
            (Part::Tab(i), Dir::Right) => (i + 1 < TABS.len()).then(|| TAB_KEY + i as u32 + 1),
            (Part::Tab(_), Dir::Down) => self.content_seat(),
            (Part::Tab(_), Dir::Up) => None,
            (Part::Opt, Dir::Up) => self.opts.step_key(RowKey(key.elem), -1).map(|k| k.0).or(Some(self.tab_key())),
            (Part::Opt, Dir::Down) => self.opts.step_key(RowKey(key.elem), 1).map(|k| k.0),
            (Part::Opt, _) => None,
            (Part::Left | Part::Right, Dir::Up | Dir::Down) => {
                let delta = if dir == Dir::Up { -1 } else { 1 };
                let step = if at == Part::Left { self.left.step_key(RowKey(key.elem), delta) } else { self.right.step_key(RowKey(key.elem), delta) };
                // UP off a column's top is the tab strip
                step.map(|k| k.0).or((dir == Dir::Up).then(|| self.tab_key()))
            }
            // across only from a group the right column is showing
            (Part::Left, Dir::Right) if self.left.index_of_key(RowKey(key.elem)).and_then(|i| self.left.id_at(i)) == Some(&self.shown) => self.right_seat(),
            (Part::Right, Dir::Left) => Some(self.left_seat()),
            _ => None,
        };
        to.map_or(Step::Edge, |elem| Step::Move(self.focus_key(elem)))
    }
    fn place(&self, elem: &u32, cx: &Cx<'_, Bundle>, _: At) -> Option<Placed> {
        if !self.holds_elem(*elem) {
            return None;
        }
        let (panel, l, r) = self.frames();
        let (rect, index, clip) = match part_of(*elem) {
            Part::Tab(i) => (*self.tab_rects(cx.measure).get(i)?, i, panel),
            Part::Left => (self.row_rect(*elem)?, self.left.index_of_key(RowKey(*elem))?, l),
            Part::Right => (self.row_rect(*elem)?, self.right.index_of_key(RowKey(*elem))?, r),
            Part::Opt => (self.row_rect(*elem)?, self.opts.index_of_key(RowKey(*elem))?, self.opts_frame()),
        };
        Some(Placed { rect, rest_rect: rect, clip, index: Some(index as u32) })
    }
    fn reconcile(&self, want: FocusKey<u32>, _: &Cx<'_, Bundle>) -> FocusKey<u32> {
        if self.holds_elem(want.elem) {
            want
        } else {
            self.focus_key(self.content_seat().unwrap_or_else(|| self.tab_key()))
        }
    }
    fn seat(&self, g: GroupId, _: Placed, _: &Cx<'_, Bundle>) -> FocusKey<u32> {
        match g {
            TAB_GROUP => self.focus_key(self.tab_key()),
            RIGHT_GROUP => self.focus_key(self.right_seat().unwrap_or_else(|| self.left_seat())),
            OPT_GROUP => self.focus_key(self.opts_seat().unwrap_or_else(|| self.tab_key())),
            _ => self.focus_key(self.left.selected_key().map_or_else(|| self.left_seat(), |k| k.0)),
        }
    }
}

impl Screen<Bundle> for SubtitleMenu {
    fn name(&self) -> &'static str {
        word::MENU
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
    }
    fn opener(&self) -> Option<(EntryId, Option<FocusKey<u32>>)> {
        Some((self.arg.host, self.arg.focus))
    }
    fn scrim(&self) -> Scrim {
        Scrim::dim(theme::underlay::DIM_COMPACT)
    }
    fn prepare(&mut self, _: &mut sn_ui::frame::Budget, _: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        let p = f.painter.alpha(f.page_alpha);
        let (panel, l, r) = self.frames();
        sn_ui::widgets::panel_ground(p, panel, PANEL_RAD, f.underlay);
        let focus = f.focus.current.filter(|k| k.entry == self.entry).map(|k| k.elem);
        for (i, (t, rect)) in TABS.iter().zip(self.tab_rects(f.measure)).enumerate() {
            let label = std::ffi::CString::new(t.label()).unwrap_or_default();
            sn_ui::widgets::TabPill::new(label.as_ptr(), TAB_SZ, rect)
                .plated()
                .segment(*t == self.tab)
                .focused(focus == Some(TAB_KEY + i as u32))
                .draw(&sn_ui::Env::inert(), p);
        }
        if self.tab == Tab::Tracks {
            self.left.table.draw(p, l, f.measure);
            self.right.table.draw(p, r, f.measure);
            // the column split: the table's own section hairline, stood upright
            p.rect(Rect::new(r.x - 1.0, l.y + theme::space::SM, 2.0, l.h - theme::space::SM - PANEL_RAD), 0.0, theme::HAIRLINE, theme::HAIRLINE, 0.0);
        } else {
            self.opts.table.draw(p, self.opts_frame(), f.measure);
        }
        for elem in self.rows() {
            if let Some(placed) = self.place(&elem, f.cx, At::Drawn) {
                f.stop(
                    p,
                    Stop {
                        key: self.focus_key(elem),
                        rect: placed.rect,
                        rest_rect: placed.rest_rect,
                        clip: placed.clip,
                        hover: Hover::Focus,
                        activate: Activate::Immediate,
                    },
                );
            }
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

impl LogicalState for SubtitleMenu {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.left.table.sel as u32).u32(self.right.table.sel as u32).u32(self.shown.key().0).u32(self.tab_key());
        self.left.table.write_motion(c);
        self.right.table.write_motion(c);
        self.opts.table.write_motion(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("subtitle menu groups={} variants={}", self.left.table.n_rows(), self.right.table.n_rows()));
    }
}

#[cfg(test)]
#[path = "subtitle_menu_tests.rs"]
mod tests;
