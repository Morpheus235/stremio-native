//! **Search** — a top-bar destination: the query field, the addon filter, and one result row per
//! searchable catalog of every addon switched on (`sn_stremio::search::plan`, any type: a live-TV
//! addon's `tv` catalog is searched exactly like Cinemeta's `movie` and `series` ones).
//!
//! PlxNative's Search page, over Stremio's protocol:
//!
//! - **The field** carries its focus by ink alone (the design system's contract): OK raises the
//!   television's keyboard (`Delivery::Keyboard`), typed text arrives as `InputKind::Text`, the
//!   panel's edit keys (◀ ▶, delete, Clear all) edit the draft, OK or DOWN commits it and BACK puts
//!   the keyboard away. Typing is debounced by the search store (`stores::search::DEBOUNCE_MS`).
//! - **The filter chips** switch an addon in or out of the search (`AddonFilter`, persisted).
//! - **With no query**: the recent searches (OK searches one again) and a Clear control, or a
//!   line saying what to type.
//! - **Each result row says where it stands**: Loading (the shared working read-out), nothing
//!   found, failed with the reason and a Retry, or its results. A row whose addon holds more than
//!   the row shows has a linked heading — OK on it opens the whole catalog as a paginated grid
//!   (`AppArg::Catalog`), seeded with what the row already holds.
//!
//! OK on a card opens its Detail page; a held OK its item menu. UP from the field reaches the bar,
//! BACK returns there.

use std::borrow::Cow;
use std::ffi::CString;

use sn_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind, InputOwner, InstanceId, Key,
    LogicalState, Machine, NavOp, TextEdit,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::search::{searchable_addons, RowState, SearchRow, ROW_PREVIEW};
use sn_ui::cards::{CardEvent, Kind, RowStyle, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{MARGIN_X, SCR_W};
use sn_ui::label::{Label, VAlign};
use sn_ui::linked_heading::LinkedHeading;
use sn_ui::screen::{
    At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, FocusTarget, Focusable, GroupKind, GroupSpec, Link, RenderStrategy,
    Screen, ScreenEvent, Seat,
};
use sn_ui::text_buffer::TextBuffer;
use sn_ui::value_chip::ValueChip;
use sn_ui::widgets::{Button, StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect, Spring, View};

use crate::cards::{item_ref, ItemCards};
use crate::registry::{reseat, word, AppArg, AppFx, Bundle, CatalogRef, MenuArg, MenuKind, SearchCmd};
use crate::stores::grid::GridKey;
use crate::stores::search::{Phase, SearchStore};
use crate::stores::StoreId;
use crate::tabs::Tab;

const NS: u32 = sn_ui::geom::NAMESPACE_FLOOR;
const FIELD: u32 = NS + 0x20;
const CLEAR: u32 = NS + 0x21;
const CHIP_BASE: u32 = NS + 0x100;
const RECENT_BASE: u32 = NS + 0x200;
const RETRY_BASE: u32 = NS + 0x300;
const MORE_BASE: u32 = NS + 0x400;
/// Result rows shown (the plan is one per searchable catalog; more than this is a misbehaving
/// install, and the extra rows are cut rather than overflowing a key band).
pub const MAX_ROWS: usize = 0xFF;
const CARD_BASE: u32 = 0x0100_0000;

const FIELD_GROUP: GroupId = GroupId(0x5345_4100);
const CHIPS_GROUP: GroupId = GroupId(0x5345_4101);
const RECENTS_GROUP: GroupId = GroupId(0x5345_4102);
const CLEAR_GROUP: GroupId = GroupId(0x5345_4103);
const STATUS_GROUP: GroupId = GroupId(0x5345_4104);
fn row_group(r: usize) -> GroupId {
    GroupId(0x5345_4200 + r as u32)
}
fn note_group(r: usize) -> GroupId {
    GroupId(0x5345_4400 + r as u32)
}
fn more_group(r: usize) -> GroupId {
    GroupId(0x5345_4600 + r as u32)
}

/// The field: under the bar, the whole safe width.
pub const FIELD_Y: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
pub const FIELD_H: f32 = 80.0;
const FIELD_BAND: f32 = FIELD_Y + FIELD_H + theme::space::SM;
const CHIP_H: f32 = sn_ui::widgets::TAB_PILL_H;
const CHIP_GAP: f32 = theme::space::SM;
const CHIPS_BAND: f32 = CHIP_H + theme::space::LG;
const HEAD_TO_ROW: f32 = 64.0;
const HEADING_TOP: f32 = 10.0;
/// A row's band when it holds a read-out instead of cards.
const NOTE_BAND: f32 = 120.0;
const RECENT_H: f32 = 72.0;
const RECENT_W: f32 = 820.0;
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
    Chips,
    Recents,
    Clear,
    Hint,
    Status,
    /// A row's linked heading (it has more than it shows).
    More(u32, usize),
    /// A row's cards.
    Row(u32, usize),
    /// A row's read-out (loading, nothing found, failed).
    Note(u32, usize),
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
        RowState::Failed(e) => RowView::Failed(e.to_string()),
        RowState::Results { count, more } => RowView::Results { count: *count, more: *more },
    }
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
    /// How many filter chips and recent searches the multi-element sections hold, as of the last
    /// sync (`plain_len` has no `Cx`).
    chips: usize,
    recents: usize,
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

    fn idle(cx: &Cx<'_, Bundle>) -> bool {
        Self::store(cx).phase() == Phase::Idle
    }

    fn chips<'a>(cx: &Cx<'a, Bundle>) -> Vec<(&'a str, &'a str)> {
        searchable_addons(cx.views.addons())
    }

    fn chip_text(cx: &Cx<'_, Bundle>, i: usize) -> Option<(CString, CString, bool)> {
        let (id, name) = *Self::chips(cx).get(i)?;
        let on = Self::store(cx).filter().allows(id);
        let name = if name.is_empty() { id } else { name };
        let value = if on { msg::app_search_addon_on() } else { msg::app_search_addon_off() };
        Some((CString::new(name.replace('\0', "")).unwrap_or_default(), CString::new(format!(" \u{b7} {value}")).unwrap_or_default(), on))
    }

    fn chip_rect(cx: &Cx<'_, Bundle>, n: usize, section_y: f32) -> Rect {
        let mut x = MARGIN_X;
        for i in 0..Self::chips(cx).len() {
            let Some((name, value, _)) = Self::chip_text(cx, i) else { break };
            let w = ValueChip::width(cx.measure, &name, &value, None);
            if i == n {
                return Rect::new(x, section_y, w, CHIP_H);
            }
            x += w + CHIP_GAP;
        }
        Rect::new(x, section_y, 0.0, CHIP_H)
    }

    fn recents<'a>(cx: &Cx<'a, Bundle>) -> &'a [String] {
        Self::store(cx).recents()
    }

    fn base(&self, r: usize) -> u32 {
        CARD_BASE + ((self.qgen & 0x3F) << 18) + ((r as u32 & 0xFF) << 10)
    }

    fn heading(row: &SearchRow) -> (&str, String) {
        let kind = crate::registry::kind_label(&row.pager.catalog.kind).map_or_else(|| row.pager.catalog.kind.clone(), str::to_string);
        (row.pager.catalog.display_name(), msg::app_home_row_source(&kind, &row.addon_name))
    }

    fn status_readout(cx: &Cx<'_, Bundle>) -> Option<StatusOverlay<'static>> {
        let frame = Rect::new(0.0, FIELD_BAND + CHIPS_BAND, SCR_W, sn_ui::consts::SCR_H - FIELD_BAND - CHIPS_BAND);
        match Self::store(cx).phase() {
            Phase::NoCatalogs => Some(StatusOverlay::new(frame, msg::app_search_no_catalogs_c(), StatusKind::Empty).reason(msg::app_search_no_catalogs_reason_c())),
            _ => None,
        }
    }
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(Self::store(cx).gen()) << 32) ^ (u64::from(cx.views.catalog.gen()) << 8) ^ u64::from(self.qgen)
    }

    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        out.push(SectionSpec::new(Sec::Field, Kind::Custom { height: FIELD_BAND, focusable: true }, FIELD_GROUP));
        let chips = Self::chips(cx).len();
        if chips > 0 {
            out.push(SectionSpec::new(Sec::Chips, Kind::Custom { height: CHIPS_BAND, focusable: true }, CHIPS_GROUP).ranked(1));
        }
        if Self::idle(cx) {
            let recents = Self::recents(cx).len();
            if recents > 0 {
                out.push(SectionSpec::new(Sec::Recents, Kind::Custom { height: recents as f32 * RECENT_H + theme::space::SM, focusable: true }, RECENTS_GROUP).ranked(2));
                out.push(SectionSpec::new(Sec::Clear, Kind::Custom { height: CHIP_H + theme::space::LG, focusable: true }, CLEAR_GROUP).ranked(3));
            } else {
                out.push(SectionSpec::new(Sec::Hint, Kind::Custom { height: HINT_BAND, focusable: false }, RECENTS_GROUP));
            }
            return;
        }
        if Page::status_readout(cx).is_some() {
            out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect: Rect::FULL, focusable: false }, STATUS_GROUP));
            return;
        }
        for (r, _) in Self::rows(cx).iter().enumerate() {
            let rank = 4 + r as u32;
            match Self::view_of(cx, r) {
                RowView::Results { more: true, .. } => {
                    out.push(SectionSpec::new(Sec::More(self.qgen, r), Kind::Custom { height: HEAD_TO_ROW, focusable: true }, more_group(r)).ranked(rank));
                    out.push(SectionSpec::new(Sec::Row(self.qgen, r), Kind::Shelf { style: &POSTER, heading: 0.0 }, row_group(r)).ranked(rank));
                }
                RowView::Results { .. } => {
                    out.push(SectionSpec::new(Sec::Row(self.qgen, r), Kind::Shelf { style: &POSTER, heading: HEAD_TO_ROW }, row_group(r)).ranked(rank));
                }
                RowView::Failed(_) => {
                    out.push(SectionSpec::new(Sec::Note(self.qgen, r), Kind::Custom { height: HEAD_TO_ROW + NOTE_BAND, focusable: true }, note_group(r)).ranked(rank));
                }
                RowView::Loading | RowView::Empty => {
                    out.push(SectionSpec::new(Sec::Note(self.qgen, r), Kind::Custom { height: HEAD_TO_ROW + NOTE_BAND, focusable: false }, note_group(r)).ranked(rank));
                }
            }
        }
    }

    fn fallback(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        for (r, _) in Self::rows(cx).iter().enumerate() {
            out.push(Sec::Row(self.qgen, r));
        }
        out.extend([Sec::Recents, Sec::Chips, Sec::Field]);
    }

    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        let Sec::Row(q, r) = k else { return None };
        if q != self.qgen {
            return None;
        }
        let row = Self::rows(cx).get(r)?;
        Some(ItemCards { items: row.preview(), base: self.base(r), more: false })
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Field => Some(FIELD),
            Sec::Clear => Some(CLEAR),
            Sec::Chips => Some(CHIP_BASE),
            Sec::Recents => Some(RECENT_BASE),
            Sec::More(_, r) => Some(MORE_BASE + r as u32),
            Sec::Note(_, r) => Some(RETRY_BASE + r as u32),
            _ => None,
        }
    }

    fn plain_len(&self, k: Sec) -> usize {
        // the multi-element sections are sized at `sections` time from the store; the Stack asks
        // with no `Cx`, so the page keeps their counts (see `SearchScreen::counts`)
        match k {
            Sec::Chips => self.counts().0,
            Sec::Recents => self.counts().1,
            _ => self.elem_of(k).is_some() as usize,
        }
    }

    fn plain_elem(&self, k: Sec, n: usize) -> Option<u32> {
        match k {
            Sec::Chips => (n < self.counts().0).then_some(CHIP_BASE + n as u32),
            Sec::Recents => (n < self.counts().1).then_some(RECENT_BASE + n as u32),
            _ if n == 0 => self.elem_of(k),
            _ => None,
        }
    }

    fn plain_step(&self, k: Sec, at: usize, dir: Dir) -> Option<usize> {
        let n = self.plain_len(k);
        match (k, dir) {
            (Sec::Chips, Dir::Left) => at.checked_sub(1),
            (Sec::Chips, Dir::Right) => (at + 1 < n).then_some(at + 1),
            (Sec::Recents, Dir::Up) => at.checked_sub(1),
            (Sec::Recents, Dir::Down) => (at + 1 < n).then_some(at + 1),
            _ => None,
        }
    }

    fn element_rect(&self, cx: &Cx<'_, Bundle>, k: Sec, n: usize, section: Rect) -> Rect {
        match k {
            Sec::Field => Rect::new(MARGIN_X, section.y + FIELD_Y, SCR_W - 2.0 * MARGIN_X, FIELD_H),
            Sec::Chips => Self::chip_rect(cx, n, section.y),
            Sec::Recents => Rect::new(MARGIN_X, section.y + n as f32 * RECENT_H, RECENT_W, RECENT_H - theme::space::XS),
            Sec::Clear => {
                let label = msg::app_search_clear_recents_c();
                let w = Button::pill_w_measured(label, theme::size::BODY, false, false, cx.measure);
                Rect::new(MARGIN_X, section.y + theme::space::XS, w, CHIP_H)
            }
            Sec::More(_, r) => {
                let (title, note) = Self::rows(cx).get(r).map(Self::heading).unwrap_or(("", String::new()));
                let more = format!("{note} \u{b7} {}", sn_ui::text_view::more_mark().to_string_lossy());
                let h = LinkedHeading::heading(title, &more).bounded(SCR_W - 2.0 * MARGIN_X);
                let m = h.measure(cx.measure);
                h.face_rect(MARGIN_X, section.y + HEADING_TOP, 1.0, &m)
            }
            Sec::Note(_, _) => {
                let label = msg::app_home_retry_c();
                let w = Button::pill_w_measured(label, theme::size::BODY, false, false, cx.measure);
                Rect::new(MARGIN_X, section.y + HEAD_TO_ROW + theme::space::XL, w, CHIP_H)
            }
            _ => section,
        }
    }

    fn plain_group(&self, _cx: &Cx<'_, Bundle>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        let (kind, len) = match k {
            Sec::Chips => (GroupKind::Row { wrap: false }, self.counts().0),
            Sec::Recents => (GroupKind::Column, self.counts().1),
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

    fn reveal_margin(&self, _k: Sec) -> f32 {
        sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::SM
    }

    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, lift: f32) {
        let Sec::Row(_, row) = k else { return };
        let Some(row) = Self::rows(f.cx).get(row) else { return };
        let (title, note) = Self::heading(row);
        sn_ui::cards::draw_heading(f.painter.alpha(f.page_alpha), title, &note, MARGIN_X, r.y + HEADING_TOP - lift, SCR_W - 2.0 * MARGIN_X, f.measure);
    }

    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        let p = f.painter.alpha(f.page_alpha);
        let focused = |elem: u32| f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == elem);
        match k {
            Sec::Field => self.draw_field(f, Rect::new(MARGIN_X, r.y + FIELD_Y, SCR_W - 2.0 * MARGIN_X, FIELD_H)),
            Sec::Chips => {
                for i in 0..self.counts().0 {
                    let Some((name, value, on)) = Self::chip_text(f.cx, i) else { break };
                    let rect = Self::chip_rect(f.cx, i, r.y);
                    let icon = if on { sn_ui::icons::Icon::Check } else { sn_ui::icons::Icon::Minus };
                    ValueChip::new(&name, &value, None, rect).icon(icon).focused(focused(CHIP_BASE + i as u32)).draw(&Env::inert(), p);
                }
            }
            Sec::Recents => {
                for (i, term) in Self::recents(f.cx).iter().enumerate() {
                    let rect = Rect::new(MARGIN_X, r.y + i as f32 * RECENT_H, RECENT_W, RECENT_H - theme::space::XS);
                    let label = CString::new(term.replace('\0', "")).unwrap_or_default();
                    Button::new(label.as_ptr(), theme::size::BODY, rect).focused(focused(RECENT_BASE + i as u32)).draw(&Env::inert(), p);
                }
            }
            Sec::Clear => {
                let label = msg::app_search_clear_recents_c();
                let w = Button::pill_w_measured(label, theme::size::BODY, false, false, f.measure);
                Button::new(label.as_ptr(), theme::size::BODY, Rect::new(MARGIN_X, r.y + theme::space::XS, w, CHIP_H))
                    .focused(focused(CLEAR))
                    .draw(&Env::inert(), p);
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
            Sec::More(_, row) => {
                let Some(sr) = Self::rows(f.cx).get(row) else { return };
                let (title, note) = Self::heading(sr);
                let more = format!("{note} \u{b7} {}", sn_ui::text_view::more_mark().to_string_lossy());
                let h = LinkedHeading::heading(title, &more).bounded(SCR_W - 2.0 * MARGIN_X);
                let m = h.measure(f.measure);
                let t = if focused(MORE_BASE + row as u32) { 1.0 } else { 0.0 };
                h.draw(p, MARGIN_X, r.y + HEADING_TOP, t, &m, f.measure);
            }
            Sec::Note(_, row) => {
                let Some(sr) = Self::rows(f.cx).get(row) else { return };
                let (title, note) = Self::heading(sr);
                sn_ui::cards::draw_heading(p, title, &note, MARGIN_X, r.y + HEADING_TOP, SCR_W - 2.0 * MARGIN_X, f.measure);
                let band = Rect::new(MARGIN_X, r.y + HEAD_TO_ROW, SCR_W - 2.0 * MARGIN_X, NOTE_BAND);
                match Self::view_of(f.cx, row) {
                    RowView::Loading => {
                        StatusOverlay::new(band, msg::app_search_row_loading_c(), StatusKind::Working)
                            .phase(f.cx.tick.ms)
                            .draw_measured(&Env::inert(), p, f.measure);
                    }
                    RowView::Empty => {
                        Label::new(msg::app_search_row_empty_c().as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY)
                            .v(VAlign::Middle)
                            .draw(p, Rect::new(band.x, band.y, band.w, theme::space::XL));
                    }
                    RowView::Failed(why) => {
                        let line = CString::new(msg::app_search_row_failed(&why).replace('\0', "")).unwrap_or_default();
                        Label::new(line.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY)
                            .v(VAlign::Middle)
                            .draw(p, Rect::new(band.x, band.y, band.w, theme::space::XL));
                        let label = msg::app_home_retry_c();
                        let w = Button::pill_w_measured(label, theme::size::BODY, false, false, f.measure);
                        Button::new(label.as_ptr(), theme::size::BODY, Rect::new(MARGIN_X, band.y + theme::space::XL, w, CHIP_H))
                            .focused(focused(RETRY_BASE + row as u32))
                            .draw(&Env::inert(), p);
                    }
                    RowView::Results { .. } => {}
                }
            }
            Sec::Row(..) => {}
        }
    }
}

impl Page {
    /// The chip and recents counts the multi-element sections are laid out with.
    fn counts(&self) -> (usize, usize) {
        (self.chips, self.recents)
    }

    fn draw_field(&self, f: &mut DrawFrame<'_, '_, Bundle>, rect: Rect) {
        let p = f.painter.alpha(f.page_alpha);
        let blank = self.query.trim().is_empty();
        let hot = self.hot.pos;
        let text = if blank { msg::app_search_placeholder() } else { self.query.as_str() };
        let run = CString::new(text.replace('\0', "")).unwrap_or_default();
        let run_w = f.measure.width(&run, theme::size::HERO, true);
        let head = &self.query[..self.caret.min(self.query.len())];
        let head_w = if blank { 0.0 } else { f.measure.width_str(head, theme::size::HERO, true) };
        let ink = if blank {
            theme::cross(theme::TEXT_TERTIARY, theme::TEXT_SECONDARY, hot)
        } else {
            theme::cross(theme::TEXT_SECONDARY, if self.editing { theme::FIELD_EDITING_INK } else { theme::FIELD_WAITING_INK }, hot)
        };
        let (run_dx, caret_dx) = run_layout(run_w, head_w, rect.w, self.editing);
        let (cap_top, cap_base) = sn_gfx::text::text_cap_band(theme::size::HERO, 1);
        let _clip = f.clip(p, Rect::new(rect.x, rect.y, rect.w, rect.h + theme::space::SM));
        Label::new(run.as_ptr(), theme::size::HERO, ink).bold().draw(p, Rect::new(rect.x + run_dx, rect.y, rect.w, rect.h));
        if self.editing && (self.blink_ms / BLINK_MS) % 2 == 0 {
            let text_y = sn_gfx::text::text_vcenter_y(theme::size::HERO, 1, rect.cy());
            p.rect(Rect::new(rect.x + caret_dx, text_y + cap_top, CARET_W, cap_base - cap_top), 0.0, theme::TEXT_PRIMARY, theme::TEXT_PRIMARY, 0.0);
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
                chips: 0,
                recents: 0,
            },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Search),
            draft: TextBuffer::new(typed.clone(), typed.len()),
            read: None,
            seen_query: store.query().to_string(),
        }
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
        self.sync_draft();
        fx.invalidate(Provenance::Input);
    }

    fn sync_draft(&mut self) {
        self.page.query = self.draft.text().to_string();
        self.page.caret = self.draft.caret();
    }

    /// The first group under the field and the chips: what DOWN out of the field commits into.
    fn first_content(&self, cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        if Page::idle(cx) {
            return (!Page::recents(cx).is_empty()).then_some(RECENTS_GROUP);
        }
        (0..Page::rows(cx).len()).find_map(|r| match Page::view_of(cx, r) {
            RowView::Results { more: true, .. } => Some(more_group(r)),
            RowView::Results { .. } => Some(row_group(r)),
            RowView::Failed(_) => Some(note_group(r)),
            _ => None,
        })
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
            crate::tabs::on_pill(elem, Tab::Search, Some(FIELD_GROUP), fx);
            return;
        }
        match elem {
            FIELD => self.keyboard(!self.page.editing, true, fx),
            CLEAR => {
                self.store(SearchCmd::ClearRecents, fx);
                reseat(FocusTarget::Elem(self.key(FIELD)), fx);
            }
            e if (CHIP_BASE..CHIP_BASE + 0x100).contains(&e) => {
                if let Some((id, _)) = Page::chips(cx).get((e - CHIP_BASE) as usize) {
                    self.store(SearchCmd::ToggleAddon((*id).to_string()), fx);
                }
            }
            e if (RECENT_BASE..RECENT_BASE + 0x100).contains(&e) => {
                if let Some(term) = Page::recents(cx).get((e - RECENT_BASE) as usize).cloned() {
                    self.draft = TextBuffer::new(term.clone(), term.len());
                    self.sync_draft();
                    self.store(SearchCmd::Submit(term.clone()), fx);
                    self.store(SearchCmd::Remember(term), fx);
                    reseat(FocusTarget::Elem(self.key(FIELD)), fx);
                }
            }
            e if (RETRY_BASE..RETRY_BASE + 0x100).contains(&e) => self.store(SearchCmd::Retry((e - RETRY_BASE) as usize), fx),
            e if (MORE_BASE..MORE_BASE + 0x100).contains(&e) => self.open_more((e - MORE_BASE) as usize, cx, fx),
            e => {
                if let Some(item) = self.item_at(cx, e) {
                    fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(item)))));
                }
            }
        }
        fx.invalidate(Provenance::Input);
    }

    /// The store moved: a new query is a new set of rows.
    fn sync(&mut self, cx: &Cx<'_, Bundle>) {
        let store = Page::store(cx);
        self.page.chips = Page::chips(cx).len().min(0x100);
        self.page.recents = store.recents().len().min(0x100);
        if self.read == Some(store.gen()) {
            return;
        }
        self.read = Some(store.gen());
        if store.query() != self.seen_query {
            self.seen_query = store.query().to_string();
            self.page.qgen = self.page.qgen.wrapping_add(1);
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
                reseat(FocusTarget::Elem(self.key(FIELD)), fx);
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                let hot = self.page.editing || cx.focus.current == Some(self.key(FIELD));
                self.page.hot.step(if hot { 1.0 } else { 0.0 }, 300.0, t.dt());
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
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Search.ord() || *ord == StoreId::Catalog.ord() => {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::Input(input) => match &input.kind {
                InputKind::SystemKeyboard(up) => {
                    if *up && !self.page.editing && cx.focus.current != Some(self.key(FIELD)) {
                        reseat(FocusTarget::Elem(self.key(FIELD)), fx);
                    }
                    self.page.editing = *up;
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
                                Key::Ok => self.keyboard(false, true, fx),
                                Key::Down => {
                                    self.keyboard(false, true, fx);
                                    if let Some(group) = self.first_content(cx) {
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
                    if *key == Key::Back && *edge == Edge::Down {
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
            ScreenEvent::PressHold(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Hold(elem))) = got {
                    if let Some(item) = self.item_at(cx, elem) {
                        let anchor = self.stack.focused_card(&self.page, cx, At::Drawn).map_or_else(crate::menu::fallback_anchor, |(_, at)| at.rect);
                        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
                            host: self.entry,
                            reply: fx.from(),
                            focus: cx.focus.current,
                            anchor: MenuArg::anchor_of(anchor),
                            kind: MenuKind::Item(item_ref(item)),
                        }))));
                    }
                }
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                if to.elem != FIELD {
                    self.keyboard(false, true, fx);
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
    fn every_row_state_has_its_own_view() {
        assert_eq!(row_view(&RowState::Loading), RowView::Loading);
        assert_eq!(row_view(&RowState::Empty), RowView::Empty);
        assert_eq!(row_view(&RowState::Failed(sn_stremio::FetchError::Status(500))), RowView::Failed("HTTP 500".into()));
        assert_eq!(row_view(&RowState::Results { count: 3, more: true }), RowView::Results { count: 3, more: true });
    }
}
