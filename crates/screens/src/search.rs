//! **Search** — a top-bar destination: the query field, the addon filter, and one result row per
//! searchable catalog of every addon switched on (`sn_stremio::search::plan`, any type: a live-TV
//! addon's `tv` catalog is searched exactly like Cinemeta's `movie` and `series` ones).
//!
//! PlxNative's Search page, over Stremio's protocol, on Home's grid (its margins, its headings,
//! its row spacing, its reveal under the top bar):
//!
//! - **The field** is a box you cannot miss (owner feedback, 2026-10-17, which replaced the
//!   ink-only contract): a film ground and a quiet rim at rest, the magnifier leading, the
//!   placeholder in `SEARCH_PLACEHOLDER_INK`; focused, the accent rim with its halo, a slightly
//!   larger box and a blinking caret ([`Page::draw_field`]). OK raises the television's keyboard
//!   (`Delivery::Keyboard`), typed text arrives as `InputKind::Text`, the panel's edit keys (◀ ▶,
//!   delete, Clear all) edit the draft, OK or DOWN commits it and BACK puts the keyboard away.
//!   Typing is debounced by the search store (`stores::search::DEBOUNCE_MS`).
//! - **Suggestions** (`SearchStore::suggestions`): while the field is being typed into, a vertical
//!   LIST directly under it (YouTube TV's and Stremio's), one row per suggestion — the matching
//!   recents (a clock mark), then titles this session's searches found (the magnifier) — the typed
//!   part of each brighter ([`match_span`]). DOWN from the field enters it, UP/DOWN walk it, OK on
//!   a row fills the field and searches it; the list goes once the text is too short to search,
//!   once OK on the field runs the search, and once focus leaves the field and the list.
//! - **The filter row**: the *Addons* dropdown (`crate::dropdown`) saying which addons are searched
//!   ("All addons", one addon's name, "3 of 7 addons"; `AddonFilter`, persisted), whose popover
//!   toggles each addon on OK and stays open (*Select all*, *Select none* first), and at the row's
//!   right end the *View* toggle — posters or a list (`crate::item_list`, a setting per page).
//! - **The result rows**, in the order they found something (a row that lands never pushes the
//!   rows above it down), then the rows still asking (the shared working read-out). A row whose
//!   addon holds more than the row shows has a linked heading — OK on it opens the whole catalog
//!   as a paginated grid (`AppArg::Catalog`), seeded with what the row already holds. The rows
//!   that found nothing or failed are not rows: ONE closing line names them, the failures first
//!   ("Failed: … (HTTP 502) · No results from: …", [`summary_line`]; led by *No results for
//!   “query”* once every row answered with nothing), with one Try again for the failed ones.
//! - **Recent searches**, always reachable: a pill row (each term, then Clear) under the filter row
//!   while there is no query, and after the last result row while there is one. OK on a term fills
//!   the field and searches it.
//!
//! The focus path is the page's order: field → suggestions → filter row → rows → recents, UP/DOWN
//! between sections.
//! OK on a card (or a list row) opens its Detail page; a held OK its item menu. UP from the field
//! reaches the bar. BACK from anywhere below the field returns to the field (the page scrolls
//! home); BACK on the field goes to the bar.
//!
//! A Detail page's cast or director link pushes this page over it (`AppArg::SearchFor`,
//! [`SearchScreen::for_query`]) with the name in the field, searched at once; BACK from anywhere on
//! it (the keyboard down) returns to the Detail page.
//!
//! **Cost.** Nothing here is glass or a page-sized translucent layer: the recents are pills, not a
//! stack of 820-pixel capsules, the closing line is text, and a list row's thumbnail is asked for
//! only while the row is on the screen.

use std::borrow::Cow;
use std::ffi::CString;

use sn_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind, InputOwner, InstanceId, Key,
    LogicalState, Machine, NavOp, TextEdit,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::search::{searchable_addons, RowState, SearchRow, ROW_PREVIEW};
use sn_stremio::FetchError;
use sn_ui::cards::{CardEvent, Kind, RowStyle, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{MARGIN_X, SCR_W};
use sn_ui::label::{Label, VAlign};
use sn_ui::linked_heading::LinkedHeading;
use sn_ui::screen::{
    At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, FocusTarget, Focusable, GroupKind, GroupSpec, Link, RenderStrategy,
    Screen, ScreenEvent, Seat,
};
use sn_ui::text_buffer::TextBuffer;
use sn_ui::widgets::{Button, StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect, Spring, View};

use crate::cards::{item_ref, ItemCards};
use crate::item_list::{self, ListRow};
use crate::dropdown;
use crate::registry::{reseat, word, AppArg, AppFx, AppMsg, Bundle, CatalogRef, ChoiceTarget, MenuArg, SearchCmd, SettingsCmd};
use crate::stores::grid::GridKey;
use crate::stores::search::{Phase, SearchStore};
use crate::stores::settings::ViewPage;
use crate::stores::StoreId;
use crate::tabs::Tab;

const NS: u32 = sn_ui::geom::NAMESPACE_FLOOR;
const FIELD: u32 = NS + 0x20;
const CLEAR: u32 = NS + 0x21;
const TOGGLE: u32 = NS + 0x22;
const RETRY_FAILED: u32 = NS + 0x23;
/// The addon filter's dropdown.
const ADDONS: u32 = NS + 0x100;
const RECENT_BASE: u32 = NS + 0x200;
const MORE_BASE: u32 = NS + 0x400;
const SUGGEST_BASE: u32 = NS + 0x500;
/// Result rows shown (the plan is one per searchable catalog; more than this is a misbehaving
/// install, and the extra rows are cut rather than overflowing a key band).
pub const MAX_ROWS: usize = 0xFF;
const CARD_BASE: u32 = 0x0100_0000;

const FIELD_GROUP: GroupId = GroupId(0x5345_4100);
const CHIPS_GROUP: GroupId = GroupId(0x5345_4101);
const RECENTS_GROUP: GroupId = GroupId(0x5345_4102);
const SUMMARY_GROUP: GroupId = GroupId(0x5345_4103);
const STATUS_GROUP: GroupId = GroupId(0x5345_4104);
const SUGGEST_GROUP: GroupId = GroupId(0x5345_4105);
/// The suggestion list's band for `n` rows.
fn suggest_band(n: usize) -> f32 {
    n as f32 * SUGGEST_ROW_H + theme::space::MD
}

/// Where the typed text `q` sits in the suggestion `s`, case blind: its byte span in `s`, the first
/// occurrence (`None`: not in it, or `q` blank). Char by char, so a case change that alters a
/// character's byte length never splits a code point.
pub fn match_span(s: &str, q: &str) -> Option<(usize, usize)> {
    let q: Vec<char> = q.trim().chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return None;
    }
    for (start, _) in s.char_indices() {
        let mut want = q.iter();
        let mut end = start;
        let mut pending: Vec<char> = Vec::new();
        for (i, c) in s[start..].char_indices() {
            pending.extend(c.to_lowercase());
            end = start + i + c.len_utf8();
            if pending.len() >= q.len() {
                break;
            }
        }
        if pending.len() >= q.len() && pending.iter().take(q.len()).all(|c| want.next() == Some(c)) {
            return Some((start, end));
        }
    }
    None
}
fn row_group(r: usize) -> GroupId {
    GroupId(0x5345_4200 + r as u32)
}
fn note_group(r: usize) -> GroupId {
    GroupId(0x5345_4400 + r as u32)
}
fn more_group(r: usize) -> GroupId {
    GroupId(0x5345_4600 + r as u32)
}
fn head_group(r: usize) -> GroupId {
    GroupId(0x5345_4800 + r as u32)
}
fn item_group(r: usize, i: usize) -> GroupId {
    GroupId(0x5346_0000 + ((r as u32 & 0xFF) << 10) + (i as u32 & 0x3FF))
}

/// The field: under the bar, the whole safe width.
pub const FIELD_Y: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
pub const FIELD_H: f32 = 96.0;
const FIELD_BAND: f32 = FIELD_Y + FIELD_H + theme::space::SM;
/// The typed text's rung inside the field box.
pub const FIELD_TEXT: i32 = theme::size::DISPLAY;
/// The magnifier at the field's leading edge, and the field's inner padding.
const FIELD_ICON: f32 = 40.0;
const FIELD_PAD: f32 = theme::space::MD;
/// How much the focused field grows about its centre (the box, never the type: text stays on its
/// rung), and the focused rim's width over the resting one.
const FIELD_FOCUS_GROW: f32 = 0.012;
const FIELD_RIM_W: f32 = 2.0;
const FIELD_RIM_FOCUS_W: f32 = 4.0;
const FIELD_GLOW_BLUR: f32 = 30.0;
/// The suggestion list under the field: one row per suggestion, a leading mark (the magnifier, or
/// a clock for a recent search) and the text with the typed part brighter.
pub const SUGGEST_ROW_H: f32 = 72.0;
pub const SUGGEST_W: f32 = 1040.0;
const SUGGEST_TEXT: i32 = theme::size::HEADLINE;
const SUGGEST_ICON: f32 = 30.0;
const CHIP_H: f32 = sn_ui::widgets::TAB_PILL_H;
const CHIP_GAP: f32 = theme::space::SM;
const CHIPS_BAND: f32 = CHIP_H + theme::space::LG;
/// Home's heading band and heading inset.
const HEAD_TO_ROW: f32 = 64.0;
const HEADING_TOP: f32 = 10.0;
/// Where a section scrolled back into view stops: under the top bar, as on Home.
const CONTENT_TOP: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
/// A row's band while it is still asking: a shelf of shimmering skeleton posters, the height its
/// cards will take, so the row that lands takes their place without moving the rows under it.
const NOTE_BAND: f32 = POSTER.h + theme::space::MD;
/// The recents: a heading, then one row of pills.
const RECENTS_BAND: f32 = HEAD_TO_ROW + CHIP_H + theme::space::LG;
const PILL_SIZE: i32 = theme::size::BODY;
/// The closing line's band (its Try again pill, when there is one, sits under it).
const SUMMARY_LINE: f32 = 56.0;
const HINT_BAND: f32 = 96.0;
const CARET_W: f32 = 4.0;
const CARET_GAP: f32 = 6.0;
const BLINK_MS: u32 = 530;
static POSTER: RowStyle = RowStyle::HOME;

/// The page's sections. A row's key carries the query generation it answers, so a new query is a
/// new set of shelves (its own scroll and pop), never a reused one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    Field,
    /// The suggestions for the text being typed.
    Suggest,
    Chips,
    /// The recent searches' pill row (and Clear).
    Recents,
    Hint,
    Status,
    /// The closing line: the rows that found nothing or failed.
    Summary,
    /// A row's linked heading (it has more than it shows).
    More(u32, usize),
    /// A list-mode row's plain heading (it has nothing more).
    Head(u32, usize),
    /// A row's cards.
    Row(u32, usize),
    /// A row still asking.
    Note(u32, usize),
    /// List mode: item `i` of row `r`.
    Item(u32, usize, usize),
}

/// What a result row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowView {
    Loading,
    Empty,
    Failed(String),
    Results { count: usize, more: bool },
}

/// A row's view from its state.
pub fn row_view(state: &RowState) -> RowView {
    match state {
        RowState::Loading => RowView::Loading,
        RowState::Empty => RowView::Empty,
        RowState::Failed(e) => RowView::Failed(short_reason(e)),
        RowState::Results { count, more } => RowView::Results { count: *count, more: *more },
    }
}

/// A failure's cause, short, localised and scrubbed ([`crate::stores::fetch_reason`]).
pub fn short_reason(e: &FetchError) -> String {
    crate::stores::fetch_reason(e)
}

/// The closing line over the rows that are not shown as rows: `(name, state)` per row, in the
/// plan's order — the failures first (Try again stands for them), then the catalogs that found
/// nothing; once every row has answered and none found anything, led by the verdict *No results
/// for “query”*. `None` when every row found something or is still asking.
pub fn summary_line(rows: &[(String, RowState)], query: &str) -> Option<String> {
    let empty: Vec<&str> = rows.iter().filter(|(_, s)| *s == RowState::Empty).map(|(n, _)| n.as_str()).collect();
    let failed: Vec<String> = rows
        .iter()
        .filter_map(|(n, s)| match s {
            RowState::Failed(e) => Some(format!("{n} ({})", short_reason(e))),
            _ => None,
        })
        .collect();
    let mut parts = Vec::new();
    let settled = rows.iter().all(|(_, s)| !matches!(s, RowState::Loading | RowState::Results { .. }));
    if settled && !rows.is_empty() && !query.trim().is_empty() {
        parts.push(msg::app_search_no_results(query.trim()));
    }
    if !failed.is_empty() {
        parts.push(msg::app_search_summary_failed(&failed.join(", ")));
    }
    if !empty.is_empty() {
        parts.push(msg::app_search_summary_empty(&empty.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// The order the result rows are shown in: the rows with results in the order they FOUND them
/// (`landed`, so a row landing late never pushes a row above it down), then the rows still
/// asking, in the plan's order. The rest are the closing line's.
pub fn display_order(landed: &[usize], views: &[RowView]) -> Vec<usize> {
    let results = landed.iter().copied().filter(|r| matches!(views.get(*r), Some(RowView::Results { .. })));
    let loading = (0..views.len()).filter(|r| views[*r] == RowView::Loading);
    results.chain(loading).collect()
}

struct Page {
    entry: EntryId,
    /// The rows' query generation as this page counts it (moves when the store's query does).
    qgen: u32,
    query: String,
    caret: usize,
    editing: bool,
    blink_ms: u32,
    hot: Spring,
    /// Whether there are addons to filter (the filter row stands), and how many recent searches
    /// the recents row holds, as of the last sync (`plain_len` has no `Cx`).
    filter: bool,
    recents: usize,
    /// The rows show as a list (the page's setting, as of the last sync).
    list: bool,
    /// The rows with results, in the order they landed ([`display_order`]).
    landed: Vec<usize>,
    /// The suggestions shown under the field (empty: no row), and a counter that moves with them.
    suggest: Vec<String>,
    suggest_rev: u32,
    /// The recents' pills' (and Clear's, last) widths as of a search-store generation, likewise.
    recent_widths: std::cell::RefCell<(u32, Vec<f32>)>,
}

impl Page {
    fn store<'a>(cx: &Cx<'a, Bundle>) -> &'a SearchStore {
        cx.views.search
    }

    fn rows<'a>(cx: &Cx<'a, Bundle>) -> &'a [SearchRow] {
        let rows = Self::store(cx).rows();
        &rows[..rows.len().min(MAX_ROWS)]
    }

    fn view_of(cx: &Cx<'_, Bundle>, r: usize) -> RowView {
        Self::store(cx).row_state(r).map_or(RowView::Loading, |s| row_view(&s))
    }

    fn views(cx: &Cx<'_, Bundle>) -> Vec<RowView> {
        (0..Self::rows(cx).len()).map(|r| Self::view_of(cx, r)).collect()
    }

    /// The rows shown as rows, in the order they are shown.
    fn shown(&self, cx: &Cx<'_, Bundle>) -> Vec<(usize, RowView)> {
        let views = Self::views(cx);
        display_order(&self.landed, &views).into_iter().map(|r| (r, views[r].clone())).collect()
    }

    #[cfg(test)]
    fn shown_for_test(&self, app: &crate::test_support::TestApp) -> Vec<usize> {
        let views: Vec<RowView> = (0..app.rig.search.rows().len()).map(|r| app.rig.search.row_state(r).map_or(RowView::Loading, |s| row_view(&s))).collect();
        display_order(&self.landed, &views)
    }

    fn idle(cx: &Cx<'_, Bundle>) -> bool {
        Self::store(cx).phase() == Phase::Idle
    }

    fn filterable(cx: &Cx<'_, Bundle>) -> bool {
        !searchable_addons(cx.views.addons()).is_empty()
    }

    /// The filter row at `y`: the Addons dropdown, then the View toggle.
    // ponytail: measured per ask (two controls)
    fn filter_rects(cx: &Cx<'_, Bundle>, y: f32) -> Vec<Rect> {
        let w = dropdown::face_w(&dropdown::addon_summary(&cx.views), cx.measure);
        dropdown::row(&[w], Some(item_list::toggle_width(cx.measure)), y)
    }

    fn recents<'a>(cx: &Cx<'a, Bundle>) -> &'a [String] {
        Self::store(cx).recents()
    }

    fn recent_label(term: &str) -> CString {
        CString::new(term.replace('\0', "")).unwrap_or_default()
    }

    /// Every recent pill's rect (Clear last) at `y`, the row shifted so the focused pill is on
    /// screen.
    fn recent_rects(&self, cx: &Cx<'_, Bundle>, y: f32) -> Vec<Rect> {
        let mut memo = self.recent_widths.borrow_mut();
        let gen = Self::store(cx).gen();
        if memo.0 != gen || memo.1.is_empty() {
            let mut widths: Vec<f32> =
                Self::recents(cx).iter().map(|t| Button::pill_w_measured(&Self::recent_label(t), PILL_SIZE, false, false, cx.measure)).collect();
            widths.push(Button::pill_w_measured(msg::app_search_clear_c(), PILL_SIZE, false, false, cx.measure));
            *memo = (gen, widths);
        }
        let widths = &memo.1;
        let focused = cx.focus.current.filter(|k| k.entry == self.entry).and_then(|k| match k.elem {
            CLEAR => Some(widths.len() - 1),
            e if (RECENT_BASE..RECENT_BASE + widths.len() as u32 - 1).contains(&e) => Some((e - RECENT_BASE) as usize),
            _ => None,
        });
        sn_ui::value_chip::row_xs(widths, focused, CHIP_GAP).into_iter().zip(widths.iter()).map(|(x, w)| Rect::new(x, y, *w, CHIP_H)).collect()
    }

    /// Suggestion `n`'s row in a list whose section starts at `y`.
    fn suggest_rect(n: usize, y: f32) -> Rect {
        Rect::new(MARGIN_X, y + n as f32 * SUGGEST_ROW_H, SUGGEST_W.min(SCR_W - 2.0 * MARGIN_X), SUGGEST_ROW_H)
    }

    /// Is `term` one of the viewer's recent searches (the list marks it with a clock)?
    fn is_recent(cx: &Cx<'_, Bundle>, term: &str) -> bool {
        let t = term.to_lowercase();
        Self::recents(cx).iter().any(|r| r.to_lowercase() == t)
    }

    /// One suggestion row: a focus plate (the accent, as a list row's focus is everywhere), the
    /// leading mark, then the text with the typed part in bright bold ink and the rest quieter.
    fn draw_suggestion(&self, f: &mut DrawFrame<'_, '_, Bundle>, i: usize, r: Rect) {
        let p = f.painter.alpha(f.page_alpha);
        let Some(term) = self.suggest.get(i) else { return };
        let focused = self.focused(f, SUGGEST_BASE + i as u32);
        if focused {
            p.rect(r, r.h * 0.5, theme::ACCENT, theme::ACCENT, 0.0);
        }
        let (lead_ink, hit_ink, rest_ink) = if focused {
            (theme::ACCENT_INK, theme::ACCENT_INK, theme::ACCENT_INK)
        } else {
            (theme::TEXT_TERTIARY, theme::TEXT_PRIMARY, theme::TEXT_SECONDARY)
        };
        let icon = if Self::is_recent(f.cx, term) { sn_ui::icons::Icon::Clock } else { sn_ui::icons::Icon::Search };
        let ix = r.x + theme::space::MD;
        sn_ui::icons::draw(p, icon, Rect::new(ix, r.cy() - SUGGEST_ICON * 0.5, SUGGEST_ICON, SUGGEST_ICON), lead_ink);
        let tx = ix + SUGGEST_ICON + theme::space::SM;
        let _clip = f.clip(p, Rect::new(tx, r.y, (r.x + r.w - theme::space::MD - tx).max(0.0), r.h));
        let (a, b) = match_span(term, &self.query).unwrap_or((0, 0));
        let mut x = tx;
        for (run, ink, bold) in [(&term[..a], rest_ink, false), (&term[a..b], hit_ink, true), (&term[b..], rest_ink, false)] {
            if run.is_empty() {
                continue;
            }
            let c = CString::new(run.replace('\0', "")).unwrap_or_default();
            let y = sn_gfx::text::text_vcenter_y(SUGGEST_TEXT, i32::from(bold), r.cy());
            p.text(c.as_ptr(), x, y, SUGGEST_TEXT, ink, 0, i32::from(bold));
            x += f.measure.width(&c, SUGGEST_TEXT, bold);
        }
    }

    fn base(&self, r: usize) -> u32 {
        CARD_BASE + ((self.qgen & 0x3F) << 18) + ((r as u32 & 0xFF) << 10)
    }

    fn heading(row: &SearchRow) -> (&str, String) {
        let kind = crate::registry::kind_label(&row.pager.catalog.kind).map_or_else(|| row.pager.catalog.kind.clone(), str::to_string);
        (row.pager.catalog.display_name(), msg::app_home_row_source(&kind, &row.addon_name))
    }

    /// The name a row goes by in the closing line: its addon and its type.
    fn short_name(row: &SearchRow) -> String {
        let kind = crate::registry::kind_label(&row.pager.catalog.kind).map_or_else(|| row.pager.catalog.kind.clone(), str::to_string);
        format!("{} ({kind})", row.addon_name)
    }

    /// The closing line, and whether any row in it failed (it then carries Try again).
    fn summary(cx: &Cx<'_, Bundle>) -> Option<(String, bool)> {
        let store = Self::store(cx);
        let rows: Vec<(String, RowState)> =
            Self::rows(cx).iter().enumerate().filter_map(|(r, row)| Some((Self::short_name(row), store.row_state(r)?))).collect();
        let failed = rows.iter().any(|(_, s)| matches!(s, RowState::Failed(_)));
        summary_line(&rows, store.query()).map(|line| (line, failed))
    }

    fn retry_rect(cx: &Cx<'_, Bundle>, section_y: f32) -> Rect {
        let w = Button::pill_w_measured(msg::app_home_retry_c(), PILL_SIZE, false, false, cx.measure);
        Rect::new(MARGIN_X, section_y + SUMMARY_LINE, w, CHIP_H)
    }

    fn status_readout(cx: &Cx<'_, Bundle>) -> Option<StatusOverlay<'static>> {
        let frame = Rect::new(0.0, FIELD_BAND + CHIPS_BAND, SCR_W, sn_ui::consts::SCR_H - FIELD_BAND - CHIPS_BAND);
        match Self::store(cx).phase() {
            Phase::NoCatalogs => Some(StatusOverlay::new(frame, msg::app_search_no_catalogs_c(), StatusKind::Empty).reason(msg::app_search_no_catalogs_reason_c())),
            _ => None,
        }
    }

    fn focused(&self, f: &DrawFrame<'_, '_, Bundle>, elem: u32) -> bool {
        f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == elem)
    }
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(Self::store(cx).gen()) << 32)
            ^ (u64::from(cx.views.catalog.gen()) << 8)
            ^ u64::from(self.qgen)
            ^ (u64::from(self.list) << 63)
            ^ ((self.landed.len() as u64) << 56)
            ^ (u64::from(self.suggest_rev) << 40)
            ^ (u64::from(cx.views.addons.gen()) << 20)
    }

    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        let mut rank = 0;
        let mut push = |out: &mut Vec<SectionSpec<Sec>>, spec: SectionSpec<Sec>| {
            out.push(spec.ranked(rank));
            rank += 1;
        };
        push(out, SectionSpec::new(Sec::Field, Kind::Custom { height: FIELD_BAND, focusable: true }, FIELD_GROUP));
        if !self.suggest.is_empty() {
            push(out, SectionSpec::new(Sec::Suggest, Kind::Custom { height: suggest_band(self.suggest.len()), focusable: true }, SUGGEST_GROUP));
        }
        if Self::filterable(cx) {
            push(out, SectionSpec::new(Sec::Chips, Kind::Custom { height: CHIPS_BAND, focusable: true }, CHIPS_GROUP));
        }
        let recents = !Self::recents(cx).is_empty();
        if Self::idle(cx) {
            if recents {
                push(out, SectionSpec::new(Sec::Recents, Kind::Custom { height: RECENTS_BAND, focusable: true }, RECENTS_GROUP));
            } else {
                push(out, SectionSpec::new(Sec::Hint, Kind::Custom { height: HINT_BAND, focusable: false }, RECENTS_GROUP));
            }
            return;
        }
        if Page::status_readout(cx).is_some() {
            push(out, SectionSpec::new(Sec::Status, Kind::Overlay { rect: Rect::FULL, focusable: false }, STATUS_GROUP));
            return;
        }
        let q = self.qgen;
        for (r, view) in self.shown(cx) {
            match view {
                RowView::Results { more, .. } if self.list => {
                    if more {
                        push(out, SectionSpec::new(Sec::More(q, r), Kind::Custom { height: HEAD_TO_ROW, focusable: true }, more_group(r)));
                    } else {
                        push(out, SectionSpec::new(Sec::Head(q, r), Kind::Custom { height: HEAD_TO_ROW, focusable: false }, head_group(r)));
                    }
                    let n = Self::rows(cx).get(r).map_or(0, |row| row.preview().len());
                    for i in 0..n {
                        push(out, SectionSpec::new(Sec::Item(q, r, i), Kind::Custom { height: item_list::ROW_H, focusable: true }, item_group(r, i)));
                    }
                }
                RowView::Results { more: true, .. } => {
                    push(out, SectionSpec::new(Sec::More(q, r), Kind::Custom { height: HEAD_TO_ROW, focusable: true }, more_group(r)));
                    push(out, SectionSpec::new(Sec::Row(q, r), Kind::Shelf { style: &POSTER, heading: 0.0 }, row_group(r)));
                }
                RowView::Results { .. } => {
                    push(out, SectionSpec::new(Sec::Row(q, r), Kind::Shelf { style: &POSTER, heading: HEAD_TO_ROW }, row_group(r)));
                }
                _ => {
                    push(out, SectionSpec::new(Sec::Note(q, r), Kind::Custom { height: HEAD_TO_ROW + NOTE_BAND, focusable: false }, note_group(r)));
                }
            }
        }
        if let Some((_, failed)) = Self::summary(cx) {
            let height = SUMMARY_LINE + if failed { CHIP_H + theme::space::LG } else { theme::space::MD };
            push(out, SectionSpec::new(Sec::Summary, Kind::Custom { height, focusable: failed }, SUMMARY_GROUP));
        }
        if recents {
            push(out, SectionSpec::new(Sec::Recents, Kind::Custom { height: RECENTS_BAND, focusable: true }, RECENTS_GROUP));
        }
    }

    fn fallback(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        for (r, _) in self.shown(cx) {
            out.push(Sec::Row(self.qgen, r));
            out.push(Sec::Item(self.qgen, r, 0));
        }
        out.extend([Sec::Summary, Sec::Recents, Sec::Chips, Sec::Suggest, Sec::Field]);
    }

    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        let Sec::Row(q, r) = k else { return None };
        if q != self.qgen {
            return None;
        }
        let row = Self::rows(cx).get(r)?;
        Some(ItemCards { items: row.preview(), base: self.base(r), more: false, watched: None })
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Field => Some(FIELD),
            Sec::Suggest => Some(SUGGEST_BASE),
            Sec::Chips => Some(ADDONS),
            Sec::Recents => Some(RECENT_BASE),
            Sec::Summary => Some(RETRY_FAILED),
            Sec::More(_, r) => Some(MORE_BASE + r as u32),
            Sec::Item(_, r, i) => Some(self.base(r) + i as u32),
            _ => None,
        }
    }

    fn plain_len(&self, k: Sec) -> usize {
        // the multi-element sections are sized at `sections` time from the store; the Stack asks
        // with no `Cx`, so the page keeps their counts (see `SearchScreen::sync`)
        match k {
            Sec::Chips => 2 * usize::from(self.filter),
            Sec::Recents => self.recents + 1,
            Sec::Suggest => self.suggest.len(),
            _ => self.elem_of(k).is_some() as usize,
        }
    }

    fn plain_elem(&self, k: Sec, n: usize) -> Option<u32> {
        match k {
            Sec::Chips => [ADDONS, TOGGLE].get(n).copied().filter(|_| self.filter),
            Sec::Recents if n < self.recents => Some(RECENT_BASE + n as u32),
            Sec::Recents => (n == self.recents).then_some(CLEAR),
            Sec::Suggest => (n < self.suggest.len()).then_some(SUGGEST_BASE + n as u32),
            _ if n == 0 => self.elem_of(k),
            _ => None,
        }
    }

    fn plain_step(&self, k: Sec, at: usize, dir: Dir) -> Option<usize> {
        let n = self.plain_len(k);
        match (k, dir) {
            (Sec::Chips | Sec::Recents, Dir::Left) => at.checked_sub(1),
            (Sec::Chips | Sec::Recents, Dir::Right) => (at + 1 < n).then_some(at + 1),
            (Sec::Suggest, Dir::Up) => at.checked_sub(1),
            (Sec::Suggest, Dir::Down) => (at + 1 < n).then_some(at + 1),
            _ => None,
        }
    }

    fn element_rect(&self, cx: &Cx<'_, Bundle>, k: Sec, n: usize, section: Rect) -> Rect {
        match k {
            Sec::Field => Rect::new(MARGIN_X, section.y + FIELD_Y, SCR_W - 2.0 * MARGIN_X, FIELD_H),
            Sec::Chips => Self::filter_rects(cx, section.y).get(n).copied().unwrap_or(Rect::new(MARGIN_X, section.y, 0.0, CHIP_H)),
            Sec::Recents => {
                let y = section.y + HEAD_TO_ROW;
                self.recent_rects(cx, y).get(n).copied().unwrap_or(Rect::new(MARGIN_X, y, 0.0, CHIP_H))
            }
            Sec::Summary => Self::retry_rect(cx, section.y),
            Sec::Suggest => Self::suggest_rect(n.min(self.suggest.len().saturating_sub(1)), section.y),
            Sec::More(_, r) => {
                let (title, note) = Self::rows(cx).get(r).map(Self::heading).unwrap_or(("", String::new()));
                let more = format!("{note} \u{b7} {}", sn_ui::text_view::more_mark().to_string_lossy());
                let h = LinkedHeading::heading(title, &more).bounded(SCR_W - 2.0 * MARGIN_X);
                let m = h.measure(cx.measure);
                h.face_rect(MARGIN_X, section.y + HEADING_TOP, 1.0, &m)
            }
            Sec::Item(..) => item_list::face_rect(section),
            _ => section,
        }
    }

    fn plain_group(&self, _cx: &Cx<'_, Bundle>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        let (kind, len) = match k {
            Sec::Item(..) => return item_list::group(id, extent),
            Sec::Chips => (GroupKind::Row { wrap: false }, 2 * usize::from(self.filter)),
            Sec::Recents => (GroupKind::Row { wrap: false }, self.recents + 1),
            Sec::Suggest => (GroupKind::Column, self.suggest.len()),
            _ => (GroupKind::Free, 1),
        };
        GroupSpec {
            id,
            kind,
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop],
            extent,
            len,
            elem: ElemKind::Control,
        }
    }

    /// A section scrolled back into view stops under the top bar (Home's margin).
    fn reveal_margin(&self, _k: Sec) -> f32 {
        CONTENT_TOP
    }

    /// A list row (and a linked heading) scrolled back into view stops under the top bar too.
    fn reveal_under_margin(&self, k: Sec) -> bool {
        matches!(k, Sec::Item(..) | Sec::More(..) | Sec::Head(..) | Sec::Summary | Sec::Recents | Sec::Suggest | Sec::Chips)
    }

    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, lift: f32) {
        let Sec::Row(_, row) = k else { return };
        let Some(row) = Self::rows(f.cx).get(row) else { return };
        let (title, note) = Self::heading(row);
        sn_ui::cards::draw_heading(f.painter.alpha(f.page_alpha), title, &note, MARGIN_X, r.y + HEADING_TOP - lift, SCR_W - 2.0 * MARGIN_X, f.measure);
    }

    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        // a section wholly off the screen draws nothing (a long list holds hundreds of rows)
        if r.y >= sn_ui::consts::SCR_H || r.y + r.h <= 0.0 {
            if !matches!(k, Sec::Status) {
                return;
            }
        }
        let p = f.painter.alpha(f.page_alpha);
        match k {
            Sec::Field => sn_base::diag::spans::span("s.field", || self.draw_field(f, Rect::new(MARGIN_X, r.y + FIELD_Y, SCR_W - 2.0 * MARGIN_X, FIELD_H))),
            Sec::Chips => {
                let rects = Self::filter_rects(f.cx, r.y);
                dropdown::draw_face(p, rects[0], &dropdown::addon_summary(&f.cx.views), self.focused(f, ADDONS), f.measure);
                item_list::draw_toggle(p, rects[1], self.list, self.focused(f, TOGGLE));
            }
            Sec::Recents => {
                sn_ui::cards::draw_heading(p, msg::app_search_recents(), "", MARGIN_X, r.y + HEADING_TOP, SCR_W - 2.0 * MARGIN_X, f.measure);
                let rects = self.recent_rects(f.cx, r.y + HEAD_TO_ROW);
                let recents = Self::recents(f.cx);
                for (i, rect) in rects.iter().enumerate() {
                    let (label, elem) = match recents.get(i) {
                        Some(term) => (Self::recent_label(term), RECENT_BASE + i as u32),
                        None => (msg::app_search_clear_c().to_owned(), CLEAR),
                    };
                    Button::new(label.as_ptr(), PILL_SIZE, *rect).focused(self.focused(f, elem)).draw(&Env::inert(), p);
                }
            }
            Sec::Suggest => {
                for i in 0..self.suggest.len() {
                    self.draw_suggestion(f, i, Self::suggest_rect(i, r.y));
                }
            }
            Sec::Hint => {
                Label::new(msg::app_search_hint_c().as_ptr(), theme::size::BODY, theme::TEXT_TERTIARY)
                    .v(VAlign::Middle)
                    .draw(p, Rect::new(MARGIN_X, r.y, SCR_W - 2.0 * MARGIN_X, HINT_BAND));
            }
            Sec::Status => {
                if let Some(o) = Page::status_readout(f.cx) {
                    o.draw_measured(&Env::inert(), p, f.measure);
                }
            }
            Sec::Summary => {
                let Some((line, failed)) = Self::summary(f.cx) else { return };
                let w = SCR_W - 2.0 * MARGIN_X;
                let line = sn_gfx::text::elide_by(&line, w, false, |t| f.measure.width_str(t, theme::size::BODY, false));
                let c = CString::new(line.replace('\0', "")).unwrap_or_default();
                Label::new(c.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY).v(VAlign::Middle).draw(p, Rect::new(MARGIN_X, r.y, w, SUMMARY_LINE));
                if failed {
                    Button::new(msg::app_home_retry_c().as_ptr(), PILL_SIZE, Self::retry_rect(f.cx, r.y))
                        .focused(self.focused(f, RETRY_FAILED))
                        .draw(&Env::inert(), p);
                }
            }
            Sec::More(_, row) => {
                let Some(sr) = Self::rows(f.cx).get(row) else { return };
                let (title, note) = Self::heading(sr);
                let more = format!("{note} \u{b7} {}", sn_ui::text_view::more_mark().to_string_lossy());
                let h = LinkedHeading::heading(title, &more).bounded(SCR_W - 2.0 * MARGIN_X);
                let m = h.measure(f.measure);
                let t = if self.focused(f, MORE_BASE + row as u32) { 1.0 } else { 0.0 };
                h.draw(p, MARGIN_X, r.y + HEADING_TOP, t, &m, f.measure);
            }
            Sec::Head(_, row) => {
                let Some(sr) = Self::rows(f.cx).get(row) else { return };
                let (title, note) = Self::heading(sr);
                sn_ui::cards::draw_heading(p, title, &note, MARGIN_X, r.y + HEADING_TOP, SCR_W - 2.0 * MARGIN_X, f.measure);
            }
            Sec::Note(_, row) => {
                // still asking: the heading says so, and a row of skeleton posters shimmers where
                // its cards will land (the search ticks the frame clock while any row asks)
                let Some(sr) = Self::rows(f.cx).get(row) else { return };
                let (title, note) = Self::heading(sr);
                let note = format!("{note} \u{b7} {}", msg::app_search_row_loading());
                sn_ui::cards::draw_heading(p, title, &note, MARGIN_X, r.y + HEADING_TOP, SCR_W - 2.0 * MARGIN_X, f.measure);
                let pitch = POSTER.w + POSTER.gap;
                let y = r.y + HEAD_TO_ROW;
                let mut x = POSTER.margin_x;
                while x < SCR_W {
                    let cell = Rect::new(x, y, POSTER.w, POSTER.h);
                    sn_ui::placeholder::skeleton(p, sn_ui::placeholder::Reason::CardSkeleton, "", cell, POSTER.radius);
                    x += pitch;
                }
            }
            Sec::Item(_, row, i) => {
                let Some(sr) = Self::rows(f.cx).get(row) else { return };
                let Some(item) = sr.preview().get(i) else { return };
                let focused = self.focused(f, self.base(row) + i as u32);
                item_list::draw(f, r, &ListRow::of_meta(item, Some(&sr.addon_name)), focused);
            }
            Sec::Row(..) => {}
        }
    }
}

impl Page {
    /// The query field: a film ground and a quiet rim at rest; focused, the accent rim, its halo
    /// and a slightly larger box (`hot`, a spring) and an accent caret (steady while waiting for OK,
    /// so the page still idles; blinking while the keyboard is up). The magnifier leads; the
    /// typed run (or the placeholder) slides left once it outgrows the box so the caret stays in it.
    fn draw_field(&self, f: &mut DrawFrame<'_, '_, Bundle>, rect: Rect) {
        let p = f.painter.alpha(f.page_alpha);
        let blank = self.query.trim().is_empty();
        let hot = self.hot.pos.clamp(0.0, 1.0);
        let boxr = rect.scaled(1.0 + FIELD_FOCUS_GROW * hot);
        let rad = boxr.h * 0.5;
        if hot > 0.01 {
            let g = theme::SEARCH_FIELD_GLOW;
            p.shadow_outside(boxr, rad, FIELD_GLOW_BLUR * hot, 0.0, theme::with_a(g, g[3] * hot));
        }
        let fill = theme::cross(theme::SEARCH_FIELD_FILL, theme::SEARCH_FIELD_FILL_FOCUS, hot);
        p.rect(boxr, rad, fill, fill, 0.0);
        let rim = theme::cross(theme::SEARCH_FIELD_RIM, theme::SEARCH_FIELD_RIM_FOCUS, hot);
        p.rring(boxr, rad, FIELD_RIM_W + (FIELD_RIM_FOCUS_W - FIELD_RIM_W) * hot, rim);
        let mark = theme::cross(theme::TEXT_SECONDARY, theme::TEXT_PRIMARY, hot);
        let ix = boxr.x + FIELD_PAD + rad * 0.25;
        sn_ui::icons::draw(p, sn_ui::icons::Icon::Search, Rect::new(ix, boxr.cy() - FIELD_ICON * 0.5, FIELD_ICON, FIELD_ICON), mark);
        let tx = ix + FIELD_ICON + theme::space::SM;
        let text = Rect::new(tx, boxr.y, (boxr.x + boxr.w - FIELD_PAD - rad * 0.25 - tx).max(0.0), boxr.h);
        let shown = if blank { msg::app_search_placeholder() } else { self.query.as_str() };
        let run = CString::new(shown.replace('\0', "")).unwrap_or_default();
        let run_w = f.measure.width(&run, FIELD_TEXT, !blank);
        let head = &self.query[..self.caret.min(self.query.len())];
        let head_w = if blank { 0.0 } else { f.measure.width_str(head, FIELD_TEXT, true) };
        let ink = if blank {
            theme::SEARCH_PLACEHOLDER_INK
        } else {
            theme::cross(theme::TEXT_SECONDARY, if self.editing { theme::FIELD_EDITING_INK } else { theme::FIELD_WAITING_INK }, hot)
        };
        let caret = self.editing || hot > 0.5;
        let (run_dx, caret_dx) = run_layout(run_w, head_w, text.w, caret);
        let (cap_top, cap_base) = sn_gfx::text::text_cap_band(FIELD_TEXT, 1);
        let _clip = f.clip(p, Rect::new(text.x, text.y, text.w, text.h));
        let mut label = Label::new(run.as_ptr(), FIELD_TEXT, ink);
        if !blank {
            label = label.bold();
        }
        label.draw(p, Rect::new(text.x + run_dx, text.y, text.w, text.h));
        if caret && (!self.editing || (self.blink_ms / BLINK_MS) % 2 == 0) {
            let text_y = sn_gfx::text::text_vcenter_y(FIELD_TEXT, 1, text.cy());
            let ink = if self.editing { theme::TEXT_PRIMARY } else { theme::SEARCH_FIELD_RIM_FOCUS };
            p.rect(Rect::new(text.x + caret_dx, text_y + cap_top, CARET_W, cap_base - cap_top), 0.0, ink, ink, 0.0);
        }
    }
}

/// Where the run and the caret sit in a field `width` wide: the run slides left so the caret
/// stays on the box's right edge once the text outgrows it (PlxNative's `run_layout`).
pub fn run_layout(run: f32, caret: f32, width: f32, editing: bool) -> (f32, f32) {
    let available = (width - if editing { CARET_W + CARET_GAP } else { 0.0 }).max(0.0);
    let overflow = (caret - available).max(0.0).min((run - available).max(0.0));
    (-overflow, (caret - overflow + CARET_GAP).min((width - CARET_W).max(0.0)))
}

pub struct SearchScreen {
    entry: EntryId,
    instance: InstanceId,
    page: Page,
    stack: Stack<Sec>,
    draft: TextBuffer,
    /// The store generation the rows were last read at.
    read: Option<u32>,
    /// The store's query the page's `qgen` counts.
    seen_query: String,
    /// The suggestions row is open: typing put it up, and focus has not left the field and it.
    suggesting: bool,
    /// The store generation and open state the suggestions were last read at.
    suggest_read: Option<(u32, bool)>,
    /// Pushed by a link: the query it searches on mount, and BACK pops back to the page.
    pushed: Option<String>,
}

impl SearchScreen {
    pub fn new(entry: EntryId, instance: InstanceId, store: &SearchStore) -> Self {
        let typed = store.typed().to_string();
        SearchScreen {
            entry,
            instance,
            page: Page {
                entry,
                qgen: 0,
                query: typed.clone(),
                caret: typed.len(),
                editing: false,
                blink_ms: 0,
                hot: Spring::at(0.0),
                filter: false,
                recents: 0,
                list: false,
                landed: Vec::new(),
                suggest: Vec::new(),
                suggest_rev: 0,
                recent_widths: std::cell::RefCell::new((u32::MAX, Vec::new())),
            },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Search),
            draft: TextBuffer::new(typed.clone(), typed.len()),
            read: None,
            seen_query: store.query().to_string(),
            suggesting: false,
            suggest_read: None,
            pushed: None,
        }
    }

    /// Search pushed by a cast or director link: `query` in the field, searched on mount.
    pub fn for_query(entry: EntryId, instance: InstanceId, store: &SearchStore, query: &str) -> Self {
        let mut page = Self::new(entry, instance, store);
        page.draft = TextBuffer::new(query.to_string(), query.len());
        page.sync_draft();
        page.pushed = Some(query.to_string());
        page
    }

    /// Is the television keyboard up for the field?
    pub fn editing(&self) -> bool {
        self.page.editing
    }

    pub fn query(&self) -> &str {
        self.draft.text()
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    fn key(&self, elem: u32) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem }
    }

    fn store(&self, cmd: SearchCmd, fx: &mut Effects<'_, Bundle>) {
        fx.push(Fx::App(AppFx::Search(cmd)));
    }

    fn keyboard(&mut self, up: bool, commit: bool, fx: &mut Effects<'_, Bundle>) {
        if self.page.editing == up {
            return;
        }
        self.page.editing = up;
        self.page.blink_ms = 0;
        if up {
            self.suggesting = true;
            self.draft = TextBuffer::new(self.draft.text().to_string(), self.draft.text().len());
            self.stack.scroll_to(0.0);
        } else if commit {
            self.commit(fx);
        }
        fx.push(Fx::Deliver(sn_machine::machine::MachineId::Instance(self.instance), Delivery::Keyboard { up }));
        fx.invalidate(Provenance::Input);
    }

    /// Search the draft now and keep it on the recents.
    fn commit(&mut self, fx: &mut Effects<'_, Bundle>) {
        let q = self.draft.text().trim().to_string();
        self.store(SearchCmd::Submit(q.clone()), fx);
        self.store(SearchCmd::Remember(q), fx);
    }

    fn edit(&mut self, edit: &TextEdit, fx: &mut Effects<'_, Bundle>) {
        let before = self.draft.text().to_string();
        self.draft.edit(edit);
        if self.draft.text() != before {
            self.store(SearchCmd::Type(self.draft.text().to_string()), fx);
            self.stack.scroll_to(0.0);
        }
        self.page.blink_ms = 0;
        self.suggesting = true;
        self.sync_draft();
        fx.invalidate(Provenance::Input);
    }

    /// Search `term` now, from a recent or a suggestion pill: it fills the field and is kept.
    fn pick(&mut self, term: String, fx: &mut Effects<'_, Bundle>) {
        self.draft = TextBuffer::new(term.clone(), term.len());
        self.sync_draft();
        self.suggesting = false;
        self.store(SearchCmd::Submit(term.clone()), fx);
        self.store(SearchCmd::Remember(term), fx);
        reseat(FocusTarget::Elem(self.key(FIELD)), fx);
        self.stack.scroll_to(0.0);
    }

    fn sync_draft(&mut self) {
        self.page.query = self.draft.text().to_string();
        self.page.caret = self.draft.caret();
    }

    /// The group under the field: what DOWN out of the field (committing the draft) moves to —
    /// the filter row, or with none the first of the content.
    fn under_field(&self, cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        if !self.page.suggest.is_empty() {
            return Some(SUGGEST_GROUP);
        }
        if Page::filterable(cx) {
            return Some(CHIPS_GROUP);
        }
        if Page::idle(cx) {
            return (!Page::recents(cx).is_empty()).then_some(RECENTS_GROUP);
        }
        self.page
            .shown(cx)
            .into_iter()
            .find_map(|(r, view)| match view {
                RowView::Results { more: true, .. } => Some(more_group(r)),
                RowView::Results { .. } if self.page.list => Some(item_group(r, 0)),
                RowView::Results { .. } => Some(row_group(r)),
                _ => None,
            })
            .or_else(|| Page::summary(cx).filter(|(_, failed)| *failed).map(|_| SUMMARY_GROUP))
            .or_else(|| (!Page::recents(cx).is_empty()).then_some(RECENTS_GROUP))
    }

    /// The row's full grid: its addon and catalog under the same query.
    fn open_more(&self, r: usize, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let Some(row) = Page::rows(cx).get(r) else { return };
        let (title, _) = Page::heading(row);
        let key = GridKey {
            addon_id: row.addon_id.clone(),
            kind: row.pager.catalog.kind.clone(),
            catalog_id: row.pager.catalog.id.clone(),
            genre: row.pager.genre.clone(),
            search: row.pager.search.clone(),
        };
        fx.push(Fx::Nav(NavOp::Push(AppArg::Catalog(CatalogRef { key, title: format!("{} \u{2014} {title}", row.addon_name) }))));
    }

    fn item_at<'a>(&self, cx: &Cx<'a, Bundle>, elem: u32) -> Option<&'a sn_stremio::types::MetaPreview> {
        Page::rows(cx).iter().enumerate().find_map(|(r, row)| row.preview().get(elem.checked_sub(self.page.base(r))? as usize))
    }

    fn activate(&mut self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        if crate::tabs::on_strip(elem) {
            self.keyboard(false, false, fx);
            crate::tabs::on_pill(elem, Tab::Search, Some(FIELD_GROUP), cx, fx);
            return;
        }
        match elem {
            FIELD => self.keyboard(!self.page.editing, true, fx),
            CLEAR => {
                self.store(SearchCmd::ClearRecents, fx);
                reseat(FocusTarget::Elem(self.key(FIELD)), fx);
            }
            TOGGLE => fx.push(Fx::App(AppFx::Settings(SettingsCmd::List(ViewPage::Search, !self.page.list)))),
            RETRY_FAILED => {
                for r in 0..Page::rows(cx).len() {
                    if matches!(Page::view_of(cx, r), RowView::Failed(_)) {
                        self.store(SearchCmd::Retry(r), fx);
                    }
                }
                reseat(FocusTarget::Elem(self.key(FIELD)), fx);
            }
            ADDONS => {
                let rect = match self.stack.focused_plain(&self.page, cx) {
                    Some((Sec::Chips, r)) if cx.focus.current.is_some_and(|k| k.elem == ADDONS) => r,
                    _ => {
                        let y = FIELD_BAND + if self.page.suggest.is_empty() { 0.0 } else { suggest_band(self.page.suggest.len()) } - self.stack.scroll();
                        Page::filter_rects(cx, y)[0]
                    }
                };
                dropdown::open(self.entry, cx, fx, rect, dropdown::addon_menu(&cx.views));
            }
            e if (RECENT_BASE..RECENT_BASE + 0x100).contains(&e) => {
                if let Some(term) = Page::recents(cx).get((e - RECENT_BASE) as usize).cloned() {
                    self.pick(term, fx);
                }
            }
            e if (SUGGEST_BASE..SUGGEST_BASE + 0x100).contains(&e) => {
                if let Some(term) = self.page.suggest.get((e - SUGGEST_BASE) as usize).cloned() {
                    self.pick(term, fx);
                }
            }
            e if (MORE_BASE..MORE_BASE + 0x100).contains(&e) => self.open_more((e - MORE_BASE) as usize, cx, fx),
            e => {
                if let Some(item) = self.item_at(cx, e) {
                    fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(item, crate::tabs::Tab::Search.label())))));
                }
            }
        }
        fx.invalidate(Provenance::Input);
    }

    /// The store moved: a new query is a new set of rows, and a row that found something joins
    /// the shown ones after those that found something before it.
    fn sync(&mut self, cx: &Cx<'_, Bundle>) {
        let store = Page::store(cx);
        self.page.filter = Page::filterable(cx);
        self.page.recents = store.recents().len().min(0xFF);
        self.page.list = cx.views.settings.list(ViewPage::Search);
        if self.suggest_read != Some((store.gen(), self.suggesting)) {
            self.suggest_read = Some((store.gen(), self.suggesting));
            let next = if self.suggesting { store.suggestions() } else { Vec::new() };
            if next != self.page.suggest {
                self.page.suggest = next;
                self.page.suggest_rev = self.page.suggest_rev.wrapping_add(1);
            }
        }
        if self.read == Some(store.gen()) {
            return;
        }
        self.read = Some(store.gen());
        if store.query() != self.seen_query {
            self.seen_query = store.query().to_string();
            self.page.qgen = self.page.qgen.wrapping_add(1);
            self.page.landed.clear();
        }
        let views = Page::views(cx);
        self.page.landed.retain(|r| matches!(views.get(*r), Some(RowView::Results { .. })));
        for (r, v) in views.iter().enumerate() {
            if matches!(v, RowView::Results { .. }) && !self.page.landed.contains(&r) {
                self.page.landed.push(r);
            }
        }
    }
}

impl LogicalState for SearchScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).str(self.draft.text()).u64(self.draft.caret() as u64).bool(self.page.editing).u32(self.page.qgen);
        self.stack.write(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("search editing={} rows-gen={}", self.page.editing, self.page.qgen));
    }
}

impl Machine<Bundle> for SearchScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        if matches!(ev, ScreenEvent::Mount | ScreenEvent::Tick(_) | ScreenEvent::StoreChanged(..)) {
            self.sync(cx);
        }
        // the keyboard owns the keys while it is up: nothing reaches the stack's cards
        let typing = self.page.editing && matches!(ev, ScreenEvent::Input(_));
        let got = if typing { None } else { self.stack.on(&self.page, ev, cx, fx) };
        match ev {
            ScreenEvent::Mount => {
                if let Some(q) = &self.pushed {
                    self.store(SearchCmd::Submit(q.clone()), fx);
                }
                reseat(FocusTarget::Elem(self.key(FIELD)), fx);
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                let hot = self.page.editing || cx.focus.current == Some(self.key(FIELD));
                self.page.hot.step(if hot { 1.0 } else { 0.0 }, 300.0, t.dt());
                // the caret blinks only while typing: a focused field waiting for OK shows it steady,
                // so a page left on the field idles
                if self.page.editing {
                    let before = self.page.blink_ms / BLINK_MS;
                    self.page.blink_ms = self.page.blink_ms.wrapping_add(t.dt_us / 1000);
                    if self.page.blink_ms / BLINK_MS != before {
                        fx.invalidate(Provenance::Input);
                    }
                }
                if (0..Page::rows(cx).len()).any(|r| Page::view_of(cx, r) == RowView::Loading) && !Page::idle(cx) {
                    fx.note(sn_machine::present::PresentEvent::Motion);
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _)
                if [StoreId::Search, StoreId::Catalog, StoreId::Addons, StoreId::Settings].iter().any(|s| *ord == s.ord()) =>
            {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::Input(input) => match &input.kind {
                InputKind::SystemKeyboard(up) => {
                    if *up && !self.page.editing && cx.focus.current != Some(self.key(FIELD)) {
                        reseat(FocusTarget::Elem(self.key(FIELD)), fx);
                    }
                    self.page.editing = *up;
                    self.suggesting |= *up;
                    self.page.blink_ms = 0;
                    fx.invalidate(Provenance::Input);
                    Handled::Yes
                }
                InputKind::Text(edit) if self.page.editing || matches!(cx.owner, InputOwner::System(_)) => {
                    self.edit(edit, fx);
                    Handled::Yes
                }
                InputKind::Key { key, sym, edge, .. } if *edge != Edge::Up => {
                    if self.page.editing {
                        let edit = match (*key, *sym) {
                            (Key::Left, _) => Some(TextEdit::Left),
                            (Key::Right, _) => Some(TextEdit::Right),
                            (_, sn_ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
                            (_, sn_ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear),
                            _ => None,
                        };
                        if let Some(edit) = edit {
                            self.edit(&edit, fx);
                            return Handled::Yes;
                        }
                        if *edge == Edge::Down {
                            match key {
                                Key::Back => self.keyboard(false, false, fx),
                                Key::Ok => {
                                    // a search runs: the suggestions have done their job
                                    self.keyboard(false, true, fx);
                                    self.suggesting = false;
                                    self.sync(cx);
                                }
                                Key::Down => {
                                    self.keyboard(false, true, fx);
                                    if let Some(group) = self.under_field(cx) {
                                        reseat(FocusTarget::ContainerGroup(group), fx);
                                    }
                                }
                                Key::Up => {
                                    self.keyboard(false, true, fx);
                                    reseat(FocusTarget::Elem(self.key(Tab::Search.elem())), fx);
                                }
                                _ => {}
                            }
                        }
                        return Handled::Yes;
                    }
                    if *key == Key::Back && *edge == Edge::Down && self.pushed.is_some() {
                        return Handled::No;
                    }
                    if *key == Key::Back && *edge == Edge::Down {
                        // BACK climbs the page first: from anything below the field, to the field
                        let below = cx.focus.current.filter(|k| k.entry == self.entry && k.elem != FIELD && !crate::tabs::on_strip(k.elem));
                        if below.is_some() {
                            reseat(FocusTarget::Elem(self.key(FIELD)), fx);
                            self.stack.scroll_to(0.0);
                            fx.invalidate(Provenance::Input);
                            return Handled::Yes;
                        }
                        return crate::tabs::on_back(self.entry, Tab::Search, cx, fx);
                    }
                    Handled::No
                }
                _ => Handled::No,
            },
            ScreenEvent::PressCommit(_) => {
                match got {
                    Some(StackEvent::Card(_, CardEvent::Activate(elem))) => self.activate(elem, cx, fx),
                    _ => {
                        if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry) {
                            self.activate(k.elem, cx, fx);
                        }
                    }
                }
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.activate(*elem, cx, fx);
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::Chose(ChoiceTarget::SearchAddons, n)) => {
                // the popover stays open and re-reads its rows (`dropdown::live`)
                for id in dropdown::addon_toggles(&cx.views, *n) {
                    self.store(SearchCmd::ToggleAddon(id), fx);
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                // a card reports its hold; a list row is the same item, held through its plain stop
                let held = match got {
                    Some(StackEvent::Card(_, CardEvent::Hold(elem))) => Some((elem, self.stack.focused_card(&self.page, cx, At::Drawn).map(|(_, at)| at.rect))),
                    _ => self.stack.focused_plain(&self.page, cx).and_then(|(k, rect)| match k {
                        Sec::Item(..) => cx.focus.current.map(|f| (f.elem, Some(rect))),
                        _ => None,
                    }),
                };
                if let Some((elem, at)) = held {
                    if let Some(item) = self.item_at(cx, elem) {
                        let anchor = at.unwrap_or_else(crate::menu::fallback_anchor);
                        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
                            host: self.entry,
                            reply: fx.from(),
                            focus: cx.focus.current,
                            anchor: MenuArg::anchor_of(anchor),
                            kind: crate::cards::item_menu(item, crate::tabs::Tab::Search.label()),
                        }))));
                    }
                }
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                if to.elem != FIELD {
                    self.keyboard(false, true, fx);
                }
                let suggestion = (SUGGEST_BASE..SUGGEST_BASE + 0x100).contains(&to.elem);
                if to.elem != FIELD && !suggestion && self.suggesting {
                    self.suggesting = false;
                    self.sync(cx);
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::WillLeave(_) | ScreenEvent::Cover | ScreenEvent::Unmount => {
                self.keyboard(false, false, fx);
                Handled::Yes
            }
            ScreenEvent::Enter(_) | ScreenEvent::Uncover => {
                fx.invalidate(Provenance::Nav);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

sn_ui::focusable_via_view!(SearchScreen, Bundle, view);

impl Screen<Bundle> for SearchScreen {
    fn name(&self) -> &'static str {
        word::SEARCH
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
    }
    fn links(&self, out: &mut Vec<Link>) {
        crate::tabs::links(FIELD_GROUP, out);
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        sn_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        self.stack.view(&self.page).paint(f);
        self.stack.draw_hold_hint(f.painter.alpha(f.page_alpha), f.measure);
    }
    fn redraw_focused(&self, f: &mut DrawFrame<'_, '_, Bundle>, focus: Option<FocusKey<u32>>) {
        self.stack.view(&self.page).redraw_focused(f, focus);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// How many of a row's items the row shows (the rest are its MORE).
pub const PREVIEW: usize = ROW_PREVIEW;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_slides_so_the_caret_stays_in_the_box() {
        assert_eq!(run_layout(100.0, 100.0, 500.0, true), (0.0, 106.0));
        let (dx, caret) = run_layout(900.0, 900.0, 500.0, true);
        assert!(dx < 0.0 && caret <= 500.0 - CARET_W);
        assert_eq!(run_layout(900.0, 0.0, 500.0, false).0, 0.0, "with no caret the run starts at the edge");
    }

    #[test]
    fn rows_show_in_the_order_they_found_something_then_the_ones_still_asking() {
        let views = [
            RowView::Results { count: 3, more: false },
            RowView::Loading,
            RowView::Results { count: 1, more: false },
            RowView::Empty,
            RowView::Failed("x".into()),
            RowView::Loading,
        ];
        assert_eq!(display_order(&[2, 0], &views), [2, 0, 1, 5], "landing order, then the plan's order; empty and failed are the closing line's");
        assert_eq!(display_order(&[4, 2], &views), [2, 1, 5], "a landed row that is not a result any more is not shown");
    }

    #[test]
    fn the_closing_line_names_the_empty_and_the_failed_rows_with_a_scrubbed_cause() {
        let _g = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        let rows = [
            ("A (Movies)".to_string(), RowState::Results { count: 3, more: false }),
            ("B (Series)".to_string(), RowState::Empty),
            ("C (TV)".to_string(), RowState::Failed(FetchError::Transport("Operation timed out after 15000 ms: https://c.example/x".into()))),
            ("D (Movies)".to_string(), RowState::Failed(FetchError::Status(502))),
            ("E (Movies)".to_string(), RowState::Failed(FetchError::Transport("Could not resolve host: e.example".into()))),
            ("F (Movies)".to_string(), RowState::Loading),
        ];
        let line = summary_line(&rows, "bat").unwrap();
        // the failures first: they are what the viewer can do something about
        assert_eq!(line, "Failed: C (TV) (timed out), D (Movies) (HTTP 502), E (Movies) (no connection) \u{b7} No results from: B (Series)");
        assert!(!line.contains("example"), "no host or URL reaches the screen");
        assert_eq!(summary_line(&rows[..1], "bat"), None);
        assert_eq!(summary_line(&rows[..2], "bat").as_deref(), Some("No results from: B (Series)"));
        // every row settled and none found anything: the verdict leads
        let none = [rows[1].clone(), rows[3].clone()];
        assert_eq!(
            summary_line(&none, "bat").as_deref(),
            Some("No results for \u{201c}bat\u{201d} \u{b7} Failed: D (Movies) (HTTP 502) \u{b7} No results from: B (Series)")
        );
        assert!(!summary_line(&[rows[1].clone(), rows[5].clone()], "bat").unwrap().contains("bat"), "no verdict while a row still asks");
    }

    /// The app-owned runs of the closing line and the recents' pills fit their slots unelided in
    /// every shipped language (the names they carry are addon text and may elide).
    #[test]
    fn the_new_search_strings_fit_in_every_shipped_language() {
        let m = sn_base::fontcov::advances::ShippedMeasure;
        let w = SCR_W - 2.0 * MARGIN_X;
        for language in sn_platform::i18n::SHIPPED {
            let _g = sn_platform::i18n::language_on_this_thread_for_test(language);
            for line in [msg::app_search_summary_empty(""), msg::app_search_summary_failed(""), msg::app_search_no_results("")] {
                use sn_machine::machine::Measure as _;
                assert!(m.width_str(&line, theme::size::BODY, false) <= w / 3.0, "{}: {line:?}", language.tag());
            }
            let clear = Button::pill_w_measured(msg::app_search_clear_c(), PILL_SIZE, false, false, &m);
            assert!(clear <= w / 4.0, "{}: Clear is {clear}px", language.tag());
            let heading = { use sn_machine::machine::Measure as _; m.width_str(msg::app_search_recents(), theme::size::HEADLINE, true) };
            assert!(heading <= w / 2.0, "{}: the recents heading is {heading}px", language.tag());
            // the placeholder fits the field's text run, beside the magnifier, at the field's rung
            let rad = FIELD_H * 0.5;
            let run = w - 2.0 * (FIELD_PAD + rad * 0.25) - FIELD_ICON - theme::space::SM;
            let ph = { use sn_machine::machine::Measure as _; m.width_str(msg::app_search_placeholder(), FIELD_TEXT, false) };
            assert!(ph <= run, "{}: the placeholder is {ph}px of {run}", language.tag());
        }
    }

    #[test]
    fn the_typed_part_of_a_suggestion_is_found_case_blind() {
        assert_eq!(match_span("Batman Begins", "bat"), Some((0, 3)));
        assert_eq!(match_span("The Batman", " BAT "), Some((4, 7)), "trimmed, any case, inside the title");
        assert_eq!(match_span("Amélie", "élie"), Some((2, 7)), "a span ends on a code point boundary");
        assert_eq!(match_span("Heat", "cold"), None);
        assert_eq!(match_span("Heat", "  "), None, "nothing typed, nothing marked");
    }

    mod screen {
        use super::super::*;
        use crate::registry::AppArg;
        use crate::stores::catalog::CatalogStore;
        use crate::stores::search::SearchLanding;
        use crate::test_support::TestApp;
        use sn_machine::machine::MachineId;
        use sn_stremio::manifest::{Addon, Manifest};
        use sn_stremio::types::{CatalogPage, MetaPreview};

        const CINEMETA: &str = r#"{"id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta","resources":["catalog","meta"],
            "types":["movie","series"],"idPrefixes":["tt"],
            "catalogs":[{"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]},
                        {"type":"series","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]}]}"#;

        fn page(prefix: &str, n: usize) -> CatalogPage {
            CatalogPage {
                metas: (0..n)
                    .map(|i| MetaPreview { id: format!("{prefix}{i}"), kind: "movie".into(), name: format!("{prefix} title {i}"), ..Default::default() })
                    .collect(),
            }
        }

        /// Search open over Cinemeta, the viewer's recents seeded.
        fn app(recents: &[&str]) -> TestApp {
            let addon = Addon::new("https://a.example/cinemeta/manifest.json", Manifest::parse(CINEMETA.as_bytes()).unwrap());
            let mut app = TestApp::boot(CatalogStore::from_addons(vec![addon]));
            app.rig.search = SearchStore::new(recents.iter().map(|s| s.to_string()).collect());
            app.notify(StoreId::Catalog, 1);
            app.d.request(MachineId::Nav, NavOp::SelectTab(AppArg::Search));
            app.frames(5);
            assert_eq!(app.top_name(), "search");
            app
        }

        fn focus(app: &TestApp) -> Option<u32> {
            app.d.focus().map(|k| k.elem)
        }

        fn screen(app: &TestApp) -> &SearchScreen {
            app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<SearchScreen>()).expect("the search screen")
        }

        /// "bat" asked, and each row's first page landed as given, in that order.
        fn search(app: &mut TestApp, landings: Vec<(usize, Result<CatalogPage, FetchError>)>) -> u32 {
            let addons = app.rig.catalog.addons().to_vec();
            let jobs = app.rig.search.submit("bat", &addons, app.rig.addons.filter());
            for (row, result) in landings {
                assert!(app.rig.search.land(SearchLanding { qgen: jobs[0].qgen, row, result }));
                app.frames(2);
            }
            app.frames(3);
            jobs[0].qgen
        }

        /// Every element DOWN reaches from the field, in order, until it stops moving.
        fn walk_down(app: &mut TestApp) -> Vec<u32> {
            let mut seen = vec![focus(app).unwrap()];
            for _ in 0..12 {
                app.press(Key::Down);
                let now = focus(app).unwrap();
                if Some(&now) == seen.last() {
                    break;
                }
                seen.push(now);
            }
            seen
        }

        #[test]
        fn idle_the_recents_are_a_pill_row_under_the_filter_and_clear_closes_it() {
            let _g = sn_base::testlock::serial();
            let mut app = app(&["news", "batman"]);
            assert_eq!(focus(&app), Some(FIELD));
            assert_eq!(walk_down(&mut app), [FIELD, ADDONS, RECENT_BASE], "field, the addon filter, then the recents");
            app.press(Key::Right);
            app.press(Key::Right);
            assert_eq!(focus(&app), Some(CLEAR), "one row: the terms, then Clear");
            app.press(Key::Ok);
            assert!(app.rig.search.recents().is_empty());
            assert_eq!(focus(&app), Some(FIELD));
        }

        #[test]
        fn with_results_the_recents_close_the_page_after_the_closing_line() {
            let _g = sn_base::testlock::serial();
            let mut app = app(&["news"]);
            search(&mut app, vec![(1, Ok(page("s", 30))), (0, Err(FetchError::Status(502)))]);
            let s = screen(&app);
            assert_eq!(s.page.landed, [1], "the series row found something");
            let path = walk_down(&mut app);
            let base = screen(&app).page.base(1);
            assert!((base..base + 30).contains(&path[3]), "a card of the row: {path:?}");
            assert_eq!(
                [path[0], path[1], path[2], path[4], path[5]],
                [FIELD, ADDONS, MORE_BASE + 1, RETRY_FAILED, RECENT_BASE],
                "field, the addon filter, the row, the closing line's Try again, the recents"
            );
            assert_eq!(path.len(), 6);
            app.take_effects();
            app.press(Key::Up);
            app.press(Key::Ok);
            assert!(app.take_effects().contains(&AppFx::Search(SearchCmd::Retry(0))), "Try again asks the failed row again");
        }

        #[test]
        fn a_row_that_lands_later_never_moves_the_rows_above_it() {
            let _g = sn_base::testlock::serial();
            let mut app = app(&[]);
            let q = search(&mut app, vec![(1, Ok(page("s", 5)))]);
            assert_eq!(screen(&app).page.shown_for_test(&app), [1, 0], "the series row first, the movie row still asking under it");
            // the movie row lands now: it joins under the series row, which stays first
            assert!(app.rig.search.land(SearchLanding { qgen: q, row: 0, result: Ok(page("m", 5)) }));
            app.frames(3);
            assert_eq!(screen(&app).page.shown_for_test(&app), [1, 0]);
            assert_eq!(screen(&app).page.landed, [1, 0]);
        }

        #[test]
        fn picking_a_recent_fills_the_field_and_searches_it() {
            let _g = sn_base::testlock::serial();
            let mut app = app(&["news", "batman"]);
            app.press(Key::Down);
            app.press(Key::Down);
            app.press(Key::Right);
            app.take_effects();
            app.press(Key::Ok);
            let fx = app.take_effects();
            assert!(fx.contains(&AppFx::Search(SearchCmd::Submit("batman".into()))));
            assert!(fx.contains(&AppFx::Search(SearchCmd::Remember("batman".into()))));
            assert_eq!(screen(&app).query(), "batman");
            assert_eq!(focus(&app), Some(FIELD));
        }

        #[test]
        fn back_climbs_to_the_field_then_the_bar() {
            let _g = sn_base::testlock::serial();
            let mut app = app(&["news"]);
            search(&mut app, vec![(0, Ok(page("m", 30))), (1, Ok(page("s", 30)))]);
            for _ in 0..6 {
                app.press(Key::Down);
            }
            assert_ne!(focus(&app), Some(FIELD));
            app.press(Key::Back);
            assert_eq!(focus(&app), Some(FIELD), "BACK from the rows returns to the field");
            assert_eq!(app.top_name(), "search");
            app.press(Key::Back);
            assert_eq!(focus(&app), Some(Tab::Search.elem()), "then to the bar");
        }

        #[test]
        fn list_mode_rows_open_and_hold_like_cards_and_the_choice_is_kept() {
            let _g = sn_base::testlock::serial();
            let mut app = app(&[]);
            search(&mut app, vec![(0, Ok(page("m", 30)))]);
            app.press(Key::Down);
            app.press(Key::Right);
            assert_eq!(focus(&app), Some(TOGGLE), "the View toggle closes the filter row");
            app.press(Key::Ok);
            assert!(app.rig.settings.list(ViewPage::Search), "the choice is the settings store's (persisted)");
            app.frames(3);
            assert!(screen(&app).page.list);
            app.press(Key::Down);
            assert_eq!(focus(&app), Some(MORE_BASE), "the row's linked heading");
            app.press(Key::Down);
            let base = screen(&app).page.base(0);
            assert_eq!(focus(&app), Some(base), "its first item's row");
            app.press(Key::Down);
            assert_eq!(focus(&app), Some(base + 1), "DOWN walks the rows");
            app.press(Key::Ok);
            assert!(matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id == "m1"), "OK opens the item: {:?}", app.top_arg());
            app.press(Key::Back);
            app.frames(5);
            assert_eq!(focus(&app), Some(base + 1));
            app.hold_ok();
            assert_eq!(app.top_name(), "menu", "a held OK opens the item's menu");
        }

        #[test]
        fn typing_offers_suggestions_under_the_field_and_a_pick_searches_it() {
            let _g = sn_base::testlock::serial();
            let mut app = app(&["batman begins", "news"]);
            app.press(Key::Ok);
            app.type_text("bat");
            assert_eq!(screen(&app).page.suggest, ["batman begins"], "a matching recent, at once");
            app.press(Key::Down);
            assert_eq!(focus(&app), Some(SUGGEST_BASE), "DOWN from the field reaches the suggestions first");
            app.press(Key::Down);
            assert_eq!(focus(&app), Some(ADDONS), "then the addon filter");
            assert!(screen(&app).page.suggest.is_empty(), "leaving them closes the row");
            app.press(Key::Back);
            assert_eq!(focus(&app), Some(FIELD));
            app.press(Key::Ok);
            app.type_text("m");
            app.press(Key::Down);
            assert_eq!(focus(&app), Some(SUGGEST_BASE));
            app.press(Key::Back);
            assert_eq!(focus(&app), Some(FIELD), "BACK from the suggestions returns to the field");
            app.press(Key::Down);
            assert_eq!(focus(&app), Some(SUGGEST_BASE));
            app.take_effects();
            app.press(Key::Ok);
            let fx = app.take_effects();
            assert!(fx.contains(&AppFx::Search(SearchCmd::Submit("batman begins".into()))), "{fx:?}");
            assert_eq!(screen(&app).query(), "batman begins");
        }

        /// The list is a COLUMN under the field: DOWN walks it, UP walks back to the field, a recent
        /// wears the clock, and OK on the field (the search runs) puts the list away.
        #[test]
        fn the_suggestions_are_a_list_walked_up_and_down_and_a_search_puts_it_away() {
            let _g = sn_base::testlock::serial();
            let mut app = app(&["batman begins", "the batman", "news"]);
            app.press(Key::Ok);
            app.type_text("bat");
            assert_eq!(screen(&app).page.suggest, ["batman begins", "the batman"]);
            app.press(Key::Down);
            assert_eq!(focus(&app), Some(SUGGEST_BASE));
            app.press(Key::Down);
            assert_eq!(focus(&app), Some(SUGGEST_BASE + 1), "DOWN walks the list");
            app.press(Key::Up);
            assert_eq!(focus(&app), Some(SUGGEST_BASE));
            app.press(Key::Up);
            assert_eq!(focus(&app), Some(FIELD), "UP off the first row is the field");
            assert!(!screen(&app).page.suggest.is_empty(), "the field and its list are one place");
            let r0 = Page::suggest_rect(0, 0.0);
            let r1 = Page::suggest_rect(1, 0.0);
            assert_eq!((r1.x, r1.y - r0.y), (r0.x, SUGGEST_ROW_H), "rows stack, one under the other");
            app.press(Key::Ok);
            app.type_text("m");
            assert!(!screen(&app).page.suggest.is_empty());
            app.press(Key::Ok);
            assert!(screen(&app).page.suggest.is_empty(), "the search ran: the list is gone");
            assert_eq!(focus(&app), Some(FIELD));
        }
    }

    #[test]
    fn every_row_state_has_its_own_view() {
        assert_eq!(row_view(&RowState::Loading), RowView::Loading);
        assert_eq!(row_view(&RowState::Empty), RowView::Empty);
        assert_eq!(row_view(&RowState::Failed(sn_stremio::FetchError::Status(500))), RowView::Failed("HTTP 500".into()));
        assert_eq!(row_view(&RowState::Results { count: 3, more: true }), RowView::Results { count: 3, more: true });
    }
}
