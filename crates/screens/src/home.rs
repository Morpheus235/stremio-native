//! **Home** — one shelf per browsable catalog of the installed addons, in install order, each
//! filled by its catalog's pager as the reader reaches it.
//!
//! The page is a `sn_ui::cards::Stack`: a status section (the install list's loading / failure
//! read-out) and one [`Shelf`](sn_ui::cards::Shelf) per row. The stack owns the layout, the one
//! scroll, the groups and their seats, placement and focus recovery; this screen owns the row
//! content ([`RowCards`]), the card element interning, and the two requests it makes of the
//! catalog store:
//!
//! - **the rows on screen** (`CatalogCmd::View`), recomputed from the stack's own shelf frames on
//!   every tick and sent when they change, which is what makes the store load what is in view
//!   first and cancel what was scrolled past;
//! - **more of a row** (`CatalogCmd::More`), from the shelf's own `Want` when its window reaches
//!   past what has loaded.
//!
//! OK on a card opens its Detail page. BACK at Home is the application's (the television's Home).

use std::collections::HashMap;

use sn_machine::machine::{Canon, Cx, Effects, EntryId, Fx, GroupId, Handled, LogicalState, Machine, NavOp};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::types::MetaPreview;
use sn_ui::cards::{CardEvent, CardSource, Kind, RowStyle, SectionSpec, Stack, StackEvent, StackPage, TileLabel};
use sn_ui::consts::{CARD_H, MARGIN_X, SCR_H, SCR_W};
use sn_ui::label::{Label, VAlign};
use sn_ui::screen::{DrawFrame, FocusSource, Focusable, HitSource, RenderStrategy, Screen, ScreenEvent};
use sn_ui::tile::{TileFacts, TileKind};
use sn_ui::widgets::{Art, StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect};

use crate::registry::{kind_label, word, AppArg, AppFx, Bundle, CatalogCmd, FirstSeat, ItemRef};
use crate::stores::catalog::{AddonsState, CatalogStore, Row, RowId, RowStatus};
use crate::stores::StoreId;

/// The first card element; every card of every row is interned above it, never reused.
const FIRST_CARD_ELEM: u32 = 0x1000;
/// The failed install list's Retry action (a key namespace, not an index).
const RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 1;
const STATUS_GROUP: GroupId = GroupId(0);
/// The page's top: the first shelf's heading sits this far down.
pub const CONTENT_TOP: f32 = 64.0;
/// From a shelf's top to its cards: the heading's band.
pub const HEAD_TO_ROW: f32 = 64.0;
/// The heading's cap top, inside its band.
const HEADING_TOP: f32 = 10.0;
static POSTER: RowStyle = RowStyle::HOME;

/// The page's sections, in document order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    /// The top band; the install list's read-out while it has no row to show.
    Status,
    Row(RowId),
    /// A row whose first page failed: its heading and the reason, where the cards would stand.
    /// Not a shelf, because a shelf with no cards and no more to come is out of the flow.
    Failed(RowId),
}

/// A failed row's band under its heading.
const FAILED_BAND: f32 = CARD_H * 0.5;

/// A row's focus group: stable per row identity, never a position.
fn row_group(id: RowId) -> GroupId {
    GroupId(1 + ((u32::from(id.source) << 12) | u32::from(id.catalog)))
}

/// The card elements of every row, interned as the rows' items land. A pager only ever appends,
/// so a row's elements are a growing list and an element never changes item.
#[derive(Default)]
struct Interned {
    rows: HashMap<RowId, Vec<u32>>,
    at: HashMap<u32, (RowId, usize)>,
    next: u32,
}

impl Interned {
    fn sync(&mut self, store: &CatalogStore) -> bool {
        let mut grew = false;
        if self.next == 0 {
            self.next = FIRST_CARD_ELEM;
        }
        for row in store.rows() {
            let elems = self.rows.entry(row.id).or_default();
            while elems.len() < row.items().len() {
                let e = self.next;
                self.next += 1;
                self.at.insert(e, (row.id, elems.len()));
                elems.push(e);
                grew = true;
            }
        }
        grew
    }

    fn cards(&self) -> usize {
        self.at.len()
    }
}

/// One row's cards: the row's items read through the interning.
pub struct RowCards<'a> {
    row: &'a Row,
    elems: &'a [u32],
    at: &'a HashMap<u32, (RowId, usize)>,
}

/// What a poster tile reads of a catalog item.
pub fn tile_facts(item: &MetaPreview) -> TileFacts<'_> {
    TileFacts {
        owner: std::ptr::from_ref(item) as usize,
        src: 0,
        kind: match item.kind.as_str() {
            "movie" => TileKind::Movie,
            "series" => TileKind::Show,
            _ => TileKind::Other,
        },
        title: &item.name,
        thumb: item.poster.as_deref().unwrap_or(""),
        ..Default::default()
    }
}

impl CardSource<Bundle> for RowCards<'_> {
    fn len(&self) -> usize {
        self.elems.len().min(self.row.items().len())
    }
    fn elem(&self, i: usize) -> u32 {
        self.elems[i]
    }
    fn index_of(&self, e: &u32) -> Option<usize> {
        self.at.get(e).filter(|(row, i)| *row == self.row.id && *i < self.len()).map(|(_, i)| *i)
    }
    fn art(&self, i: usize) -> Art<'_> {
        Art::Poster(self.row.items().get(i).map(tile_facts))
    }
    fn label(&self, i: usize) -> TileLabel {
        match self.row.items().get(i) {
            Some(item) => TileLabel::titled(&item.name, item.release_info.as_deref().unwrap_or("")),
            None => TileLabel::default(),
        }
    }
    fn more(&self) -> bool {
        self.row.pager.has_more()
    }
}

/// The page's content, read by every section. The `Stack` is the screen's other field, so
/// `self.stack.on(&self.page, ..)` borrows the two apart.
struct Page {
    entry: EntryId,
    cards: Interned,
}

impl Page {
    fn store<'a>(cx: &Cx<'a, Bundle>) -> &'a CatalogStore {
        cx.views.catalog
    }

    /// The read-out the status section shows, if any: while the install list has no row.
    fn status(store: &CatalogStore) -> Option<StatusKind> {
        match store.addons_state() {
            AddonsState::Loading => Some(StatusKind::Working),
            AddonsState::Failed => Some(StatusKind::Failed),
            AddonsState::Ready if shown_rows(store).next().is_none() && !loading_more(store) => Some(StatusKind::Empty),
            AddonsState::Ready => None,
        }
    }

    fn status_overlay<'a>(store: &CatalogStore, ms: u32) -> Option<StatusOverlay<'a>> {
        let kind = Self::status(store)?;
        Some(match kind {
            StatusKind::Working => StatusOverlay::new(Rect::FULL, msg::app_home_loading_addons_c(), StatusKind::Working).phase(ms),
            StatusKind::Failed => StatusOverlay::new(Rect::FULL, msg::app_home_addons_failed_c(), StatusKind::Failed)
                .reason(msg::app_home_addons_failed_reason_c())
                .action(msg::app_home_retry_c())
                .page(sn_ui::icons::Icon::Alert),
            _ => StatusOverlay::new(Rect::FULL, msg::app_home_nothing_c(), StatusKind::Empty),
        })
    }

    /// The heading's annotation: the type, and the addon when more than one is installed.
    fn source_line(store: &CatalogStore, row: &Row) -> String {
        let kind = kind_label(row.kind()).map_or_else(|| row.kind().to_string(), str::to_string);
        if store.sources().len() > 1 {
            msg::app_home_row_source(&kind, &row.addon_name)
        } else {
            kind
        }
    }
}

/// The rows Home shows: every row but one whose catalog ended empty.
fn shown_rows(store: &CatalogStore) -> impl Iterator<Item = (usize, &Row)> {
    store.rows().iter().enumerate().filter(|(_, r)| r.status() != RowStatus::Empty)
}

/// Where focus goes on a Home that has none yet: the first row with cards, else the failed install
/// list's Retry.
fn first_seat(store: &CatalogStore) -> Option<sn_ui::screen::FocusTarget<u32>> {
    use sn_ui::screen::FocusTarget;
    shown_rows(store)
        .find(|(_, r)| !r.items().is_empty())
        .map(|(_, r)| FocusTarget::FirstInGroup(row_group(r.id)))
        .or_else(|| (Page::status(store) == Some(StatusKind::Failed)).then_some(FocusTarget::FirstInGroup(STATUS_GROUP)))
}

/// A row that failed before any card landed (it shows a note, not a shelf).
fn failed_empty(row: &Row) -> bool {
    matches!(row.status(), RowStatus::Failed(_)) && row.items().is_empty()
}

/// Is some row still waiting for its first page (so an empty page is not yet "nothing")?
fn loading_more(store: &CatalogStore) -> bool {
    store.rows().iter().any(|r| r.status() == RowStatus::Loading)
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = RowCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(Self::store(cx).gen()) << 32) | self.cards.cards() as u64
    }

    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        let store = Self::store(cx);
        match Self::status_overlay(store, 0).filter(|_| Self::status(store) == Some(StatusKind::Failed)) {
            Some(overlay) => {
                let rect = overlay.action_frame_measured(cx.measure).unwrap_or(Rect::FULL);
                out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: true }, STATUS_GROUP));
            }
            None => out.push(SectionSpec::new(Sec::Status, Kind::Custom { height: CONTENT_TOP, focusable: false }, STATUS_GROUP)),
        }
        for (rank, (_, row)) in shown_rows(store).enumerate() {
            let spec = if failed_empty(row) {
                SectionSpec::new(
                    Sec::Failed(row.id),
                    Kind::Custom { height: HEAD_TO_ROW + FAILED_BAND, focusable: false },
                    row_group(row.id),
                )
            } else {
                SectionSpec::new(Sec::Row(row.id), Kind::Shelf { style: &POSTER, heading: HEAD_TO_ROW }, row_group(row.id))
            };
            out.push(spec.ranked(rank as u32));
        }
    }

    fn fallback(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        out.extend(shown_rows(Self::store(cx)).filter(|(_, r)| !failed_empty(r)).map(|(_, r)| Sec::Row(r.id)));
        out.push(Sec::Status);
    }

    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<RowCards<'a>> {
        let Sec::Row(id) = k else { return None };
        let row = Self::store(cx).row(id)?;
        Some(RowCards { row, elems: self.cards.rows.get(&id).map_or(&[], Vec::as_slice), at: &self.cards.at })
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        (k == Sec::Status).then_some(RETRY_ELEM)
    }

    /// Catalog cards carry no item menu yet.
    fn card_has_menu(&self, _cx: &Cx<'_, Bundle>, _k: Sec, _elem: &u32) -> bool {
        false
    }

    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, lift: f32) {
        let Sec::Row(id) = k else { return };
        let store = Self::store(f.cx);
        let Some(row) = store.row(id) else { return };
        let p = f.painter.alpha(f.page_alpha);
        let source = Self::source_line(store, row);
        sn_ui::cards::draw_heading(p, row.title(), &source, MARGIN_X, r.y + HEADING_TOP - lift, SCR_W - 2.0 * MARGIN_X, f.measure);
    }

    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        if let Sec::Failed(id) = k {
            let store = Self::store(f.cx);
            let Some(row) = store.row(id) else { return };
            let p = f.painter.alpha(f.page_alpha);
            let source = Self::source_line(store, row);
            sn_ui::cards::draw_heading(p, row.title(), &source, MARGIN_X, r.y + HEADING_TOP, SCR_W - 2.0 * MARGIN_X, f.measure);
            // the strip the cards would have stood in, with the reason instead
            let note = msg::app_home_row_failed_c();
            let band = Rect::new(MARGIN_X, r.y + HEAD_TO_ROW, SCR_W - 2.0 * MARGIN_X, FAILED_BAND);
            Label::new(note.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY).v(VAlign::Middle).draw(p, band);
            return;
        }
        if k != Sec::Status {
            return;
        }
        let store = Self::store(f.cx);
        if let Some(overlay) = Self::status_overlay(store, f.cx.tick.ms) {
            let focused = f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == RETRY_ELEM);
            overlay.focused(focused).draw_measured(&Env::inert(), f.painter.alpha(f.page_alpha), f.measure);
        }
    }
}

pub struct HomeScreen {
    page: Page,
    stack: Stack<Sec>,
    /// The rows last reported on screen, as store indices.
    view: Option<std::ops::Range<usize>>,
    /// Home mounts before its rows land: the first one that does takes focus.
    seat: FirstSeat,
}

impl HomeScreen {
    pub const SHAPE: &'static str = "HomeScreen{entry:u32,cards:u32,view:Option<(u32,u32)>,stack:Stack}";

    pub fn new(entry: EntryId) -> Self {
        HomeScreen { page: Page { entry, cards: Interned::default() }, stack: Stack::new(entry), view: None, seat: FirstSeat::new() }
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    /// The store rows whose shelves are on screen now (or a row's worth at the top, before the
    /// layout exists).
    fn rows_in_view(&self, cx: &Cx<'_, Bundle>) -> std::ops::Range<usize> {
        let store = cx.views.catalog;
        let mut lo = usize::MAX;
        let mut hi = 0;
        for (i, row) in shown_rows(store) {
            let Some(frame) = self.stack.shelf_frame(&self.page, cx, Sec::Row(row.id)) else { continue };
            let top = frame.y - HEAD_TO_ROW;
            if sn_ui::on_axis(top, HEAD_TO_ROW + CARD_H, SCR_H, 0.0) {
                lo = lo.min(i);
                hi = hi.max(i + 1);
            }
        }
        if lo == usize::MAX {
            0..0
        } else {
            lo..hi
        }
    }

    /// The item behind a card element.
    fn item<'a>(&self, cx: &Cx<'a, Bundle>, elem: u32) -> Option<&'a MetaPreview> {
        let (row, i) = *self.page.cards.at.get(&elem)?;
        cx.views.catalog.row(row)?.items().get(i)
    }

    fn open(&self, cx: &Cx<'_, Bundle>, elem: u32, fx: &mut Effects<'_, Bundle>) {
        if let Some(item) = self.item(cx, elem) {
            fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(ItemRef {
                kind: item.kind.clone(),
                id: item.id.clone(),
                name: item.name.clone(),
            }))));
        }
    }
}

impl LogicalState for HomeScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.page.entry.0).u32(self.page.cards.cards() as u32);
        c.option(self.view.clone(), |c, v| {
            c.u32(v.start as u32).u32(v.end as u32);
        });
        self.stack.write(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("home cards={} view={:?}", self.page.cards.cards(), self.view));
    }
}

impl Machine<Bundle> for HomeScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        // The interning is current before the stack reads the rows for this event.
        let grew = match ev {
            ScreenEvent::Tick(_) | ScreenEvent::Mount | ScreenEvent::StoreChanged(..) => self.page.cards.sync(cx.views.catalog),
            _ => false,
        };
        let got = self.stack.on(&self.page, ev, cx, fx);
        match ev {
            ScreenEvent::Tick(_) => {
                let view = self.rows_in_view(cx);
                if self.view.as_ref() != Some(&view) {
                    self.view = Some(view.clone());
                    fx.push(Fx::App(AppFx::Catalog(CatalogCmd::View(view))));
                }
                if let Some(StackEvent::Card(Sec::Row(id), CardEvent::Want(_))) = got {
                    fx.push(Fx::App(AppFx::Catalog(CatalogCmd::More(id))));
                }
                self.seat.tick(cx.focus.current.is_some_and(|k| k.entry == self.page.entry), first_seat(cx.views.catalog), fx);
                if grew {
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Catalog.ord() => {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Activate(elem))) = got {
                    self.open(cx, elem, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if *elem == RETRY_ELEM => {
                fx.push(Fx::App(AppFx::Catalog(CatalogCmd::RetryAddons)));
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                // a card activated without a press (the pointer's direct activation)
                if self.page.cards.at.contains_key(elem) {
                    self.open(cx, *elem, fx);
                }
                Handled::Yes
            }
            ScreenEvent::FocusMoved { .. } => {
                fx.invalidate(Provenance::Input);
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

sn_ui::focusable_via_view!(HomeScreen, Bundle, view);

impl Screen<Bundle> for HomeScreen {
    fn name(&self) -> &'static str {
        word::HOME
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<std::borrow::Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        sn_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        self.stack.view(&self.page).paint(f);
    }
    fn redraw_focused(&self, f: &mut DrawFrame<'_, '_, Bundle>, focus: Option<sn_machine::machine::FocusKey<u32>>) {
        self.stack.view(&self.page).redraw_focused(f, focus);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
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
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// The heading a row draws, as the test suite reads it: title and annotation.
pub fn heading_of(store: &CatalogStore, id: RowId) -> Option<(String, String)> {
    let row = store.row(id)?;
    Some((row.title().to_string(), Page::source_line(store, row)))
}
