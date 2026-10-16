//! **Home** — signed in, **Continue Watching** first (the account library's items in progress,
//! most recent first, each poster with its resume bar); then one shelf per browsable catalog of
//! the addons in use (`AddonsStore::in_use`: the account's collection, in its order, or the
//! defaults), each filled by its catalog's pager as the reader reaches it.
//!
//! OK on a Continue Watching card RESUMES: `LibraryItem::resume_target` names the video (for a
//! series the EPISODE it stopped on, never the bare series id) and the offset, and that video's
//! Streams page opens with the offset carried to the play; an item with no recorded video opens
//! the Resume page, which reads its meta and takes the first episode.
//!
//! The page is a `sn_ui::cards::Stack`: a status section (the install list's loading / failure
//! read-out), the Continue Watching shelf, and one [`Shelf`](sn_ui::cards::Shelf) per row. The
//! stack owns the layout, the one scroll, the groups and their seats, placement and focus
//! recovery; this screen owns the row content ([`RowCards`]), the card element interning (by a
//! row's identity, so a refreshed collection that keeps a row keeps its cards), and the two
//! requests it makes of the catalog store:
//!
//! - **the rows on screen** (`CatalogCmd::View`), recomputed from the stack's own shelf frames on
//!   every tick and sent when they change, which is what makes the store load what is in view
//!   first and cancel what was scrolled past;
//! - **more of a row** (`CatalogCmd::More`), from the shelf's own `Want` when its window reaches
//!   past what has loaded.
//!
//! OK on a card opens its Detail page; a held OK opens its item menu (`crate::menu`). The page
//! wears the shared top bar (`crate::tabs`): UP from the first row reaches it, BACK on a row
//! returns focus to it, and BACK on the bar is the application's (the television's Home).

use std::collections::HashMap;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, Fx, GroupId, Handled, InputEvent, InputKind, Key, LogicalState, Machine, NavOp,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::types::MetaPreview;
use sn_ui::cards::{CardEvent, CardSource, Kind, RowStyle, SectionSpec, Stack, StackEvent, StackPage, TileLabel};
use sn_ui::consts::{CARD_H, MARGIN_X, SCR_H, SCR_W};
use sn_ui::label::{Label, VAlign};
use sn_ui::screen::{At, DrawFrame, FocusSource, Focusable, HitSource, Link, RenderStrategy, Screen, ScreenEvent};
use sn_ui::widgets::{Art, StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect};

use crate::cards::item_ref;
use crate::item_cards::{self, ItemCards};
use crate::registry::{kind_label, word, AppArg, AppFx, Bundle, CatalogCmd, FirstSeat, ItemRef, MenuArg, MenuKind, VideoRef};
use crate::stores::account::SessionState;
use crate::stores::catalog::{AddonsState, CatalogStore, Row, RowId, RowStatus};
use crate::stores::StoreId;
use sn_stremio::library::LibraryItem;

/// The first card element; every card of every row is interned above it, never reused.
const FIRST_CARD_ELEM: u32 = 0x1000;
/// The first Continue Watching element: far above any catalog card's.
const FIRST_CW_ELEM: u32 = 0x4000_0000;
const CW_GROUP: GroupId = GroupId(0x7FFF_0000);
/// The failed install list's Retry action (a key namespace, not an index).
const RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 1;
const STATUS_GROUP: GroupId = GroupId(0);
/// The page's top: the first shelf's heading sits this far down, clear of the shared top bar.
pub const CONTENT_TOP: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
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
    /// The account library's items in progress.
    Continue,
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

/// A row's identity as the interning keys it ([`Row::identity`], owned).
type RowIdent = (String, String, String);

fn ident(row: &Row) -> RowIdent {
    let (b, k, i) = row.identity();
    (b.to_string(), k.to_string(), i.to_string())
}

/// The card elements of every row, interned as the rows' items land, by the row's IDENTITY (its
/// addon base and catalog), not its position, so a replaced install list that keeps the row keeps
/// its elements. A pager only ever appends, so a row's elements are a growing list; a row whose
/// pager started over (its addon changed) is interned afresh.
#[derive(Default)]
struct Interned {
    rows: HashMap<RowIdent, Vec<u32>>,
    at: HashMap<u32, (RowIdent, usize)>,
    next: u32,
    epoch: u32,
}

impl Interned {
    fn sync(&mut self, store: &CatalogStore) -> bool {
        let mut grew = false;
        if self.next == 0 {
            self.next = FIRST_CARD_ELEM;
        }
        if self.epoch != store.epoch() {
            self.epoch = store.epoch();
            // a row that shrank started over: its old elements name nothing now
            for row in store.rows() {
                let key = ident(row);
                if self.rows.get(&key).is_some_and(|e| e.len() > row.items().len()) {
                    for e in self.rows.remove(&key).unwrap_or_default() {
                        self.at.remove(&e);
                    }
                }
            }
            grew = true;
        }
        for row in store.rows() {
            let key = ident(row);
            let elems = self.rows.entry(key.clone()).or_default();
            while elems.len() < row.items().len() {
                let e = self.next;
                self.next += 1;
                self.at.insert(e, (key.clone(), elems.len()));
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
    at: &'a HashMap<u32, (RowIdent, usize)>,
}

pub use crate::cards::tile_facts;

impl CardSource<Bundle> for RowCards<'_> {
    fn len(&self) -> usize {
        self.elems.len().min(self.row.items().len())
    }
    fn elem(&self, i: usize) -> u32 {
        self.elems[i]
    }
    fn index_of(&self, e: &u32) -> Option<usize> {
        let (b, k, id) = self.row.identity();
        self.at
            .get(e)
            .filter(|((rb, rk, rid), i)| (rb.as_str(), rk.as_str(), rid.as_str()) == (b, k, id) && *i < self.len())
            .map(|(_, i)| *i)
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

/// A section's cards: a catalog row's, or Continue Watching's.
pub enum HomeCards<'a> {
    Row(RowCards<'a>),
    Continue(ItemCards<'a>),
}

macro_rules! both {
    ($self:expr, $c:ident => $e:expr) => {
        match $self {
            HomeCards::Row($c) => $e,
            HomeCards::Continue($c) => $e,
        }
    };
}

impl CardSource<Bundle> for HomeCards<'_> {
    fn len(&self) -> usize {
        both!(self, c => CardSource::<Bundle>::len(c))
    }
    fn elem(&self, i: usize) -> u32 {
        both!(self, c => CardSource::<Bundle>::elem(c, i))
    }
    fn index_of(&self, e: &u32) -> Option<usize> {
        both!(self, c => CardSource::<Bundle>::index_of(c, e))
    }
    fn art(&self, i: usize) -> Art<'_> {
        both!(self, c => CardSource::<Bundle>::art(c, i))
    }
    fn label(&self, i: usize) -> TileLabel {
        both!(self, c => CardSource::<Bundle>::label(c, i))
    }
    fn progress(&self, i: usize) -> Option<f32> {
        both!(self, c => CardSource::<Bundle>::progress(c, i))
    }
    fn more(&self) -> bool {
        both!(self, c => CardSource::<Bundle>::more(c))
    }
}

/// The page's content, read by every section. The `Stack` is the screen's other field, so
/// `self.stack.on(&self.page, ..)` borrows the two apart.
struct Page {
    entry: EntryId,
    cards: Interned,
    /// Continue Watching's cards, by library item id.
    cw: item_cards::Interned,
}

/// The Continue Watching items Home shows: signed in, in progress, most recent first.
fn continue_items<'a>(cx: &Cx<'a, Bundle>) -> Vec<&'a LibraryItem> {
    match cx.views.account.session() {
        SessionState::SignedIn(_) => cx.views.library.continue_watching(),
        _ => Vec::new(),
    }
}

/// Where a Continue Watching press goes: the resume target's Streams page, with the offset (for a
/// series the episode it stopped on), or the Resume page for an item with no recorded video.
pub fn resume_arg(item: &LibraryItem) -> AppArg {
    match item.resume_target() {
        Some(t) => AppArg::Streams(VideoRef {
            kind: t.kind,
            video_id: t.video_id,
            show: item.name.clone(),
            title: item_cards::caption(item),
            resume_ms: t.offset_ms,
        }),
        None => AppArg::Resume(ItemRef { kind: item.kind.clone(), id: item.id.clone(), name: item.name.clone() }),
    }
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
    type Cards<'a> = HomeCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        let library = u64::from(cx.views.library.gen()) ^ (u64::from(cx.views.account.gen()) << 16);
        (u64::from(Self::store(cx).gen()) << 32) ^ (library << 20) ^ (self.cards.cards() + self.cw.len()) as u64
    }

    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        let store = Self::store(cx);
        let cw = !continue_items(cx).is_empty();
        match Self::status_overlay(store, 0).filter(|_| Self::status(store) == Some(StatusKind::Failed) && !cw) {
            Some(overlay) => {
                let rect = overlay.action_frame_measured(cx.measure).unwrap_or(Rect::FULL);
                out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: true }, STATUS_GROUP));
            }
            None => out.push(SectionSpec::new(Sec::Status, Kind::Custom { height: CONTENT_TOP, focusable: false }, STATUS_GROUP)),
        }
        let first = u32::from(cw);
        if cw {
            out.push(SectionSpec::new(Sec::Continue, Kind::Shelf { style: &POSTER, heading: HEAD_TO_ROW }, CW_GROUP).ranked(0));
        }
        for (rank, (_, row)) in shown_rows(store).enumerate() {
            let rank = rank as u32 + first;
            let spec = if failed_empty(row) {
                SectionSpec::new(
                    Sec::Failed(row.id),
                    Kind::Custom { height: HEAD_TO_ROW + FAILED_BAND, focusable: false },
                    row_group(row.id),
                )
            } else {
                SectionSpec::new(Sec::Row(row.id), Kind::Shelf { style: &POSTER, heading: HEAD_TO_ROW }, row_group(row.id))
            };
            out.push(spec.ranked(rank));
        }
    }

    fn fallback(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        if !continue_items(cx).is_empty() {
            out.push(Sec::Continue);
        }
        out.extend(shown_rows(Self::store(cx)).filter(|(_, r)| !failed_empty(r)).map(|(_, r)| Sec::Row(r.id)));
        out.push(Sec::Status);
    }

    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<HomeCards<'a>> {
        match k {
            Sec::Continue => Some(HomeCards::Continue(ItemCards { items: continue_items(cx), cards: &self.cw })),
            Sec::Row(id) => {
                let row = Self::store(cx).row(id)?;
                let elems = self.cards.rows.get(&ident(row)).map_or(&[][..], Vec::as_slice);
                Some(HomeCards::Row(RowCards { row, elems, at: &self.cards.at }))
            }
            _ => None,
        }
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        (k == Sec::Status).then_some(RETRY_ELEM)
    }

    /// Every catalog card opens its item menu on a held OK.
    fn card_has_menu(&self, _cx: &Cx<'_, Bundle>, _k: Sec, _elem: &u32) -> bool {
        true
    }

    /// A shelf scrolled back into view stops under the top bar, not under the screen's edge.
    fn reveal_margin(&self, _k: Sec) -> f32 {
        CONTENT_TOP
    }

    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, lift: f32) {
        if k == Sec::Continue {
            let p = f.painter.alpha(f.page_alpha);
            sn_ui::cards::draw_heading(p, msg::app_home_continue(), "", MARGIN_X, r.y + HEADING_TOP - lift, SCR_W - 2.0 * MARGIN_X, f.measure);
            return;
        }
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
    /// The group DOWN from the bar enters, as of the last tick (`Screen::links` has no `Cx`).
    first_row: Option<GroupId>,
    /// The viewer has moved focus themselves.
    chose: bool,
}

impl HomeScreen {
    pub const SHAPE: &'static str = "HomeScreen{entry:u32,cards:u32,view:Option<(u32,u32)>,stack:Stack}";

    pub fn new(entry: EntryId) -> Self {
        HomeScreen {
            page: Page { entry, cards: Interned::default(), cw: item_cards::Interned::with_base(FIRST_CW_ELEM) },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Library),
            view: None,
            seat: FirstSeat::new(),
            first_row: None,
            chose: false,
        }
    }

    /// Intern every Continue Watching item; `true` when one was new.
    fn sync_continue(&mut self, cx: &Cx<'_, Bundle>) -> bool {
        let before = self.page.cw.len();
        for item in continue_items(cx) {
            self.page.cw.elem(&item.id);
        }
        self.page.cw.len() != before
    }

    /// The Continue Watching item behind a card element.
    fn continue_item<'a>(&self, cx: &Cx<'a, Bundle>, elem: u32) -> Option<&'a LibraryItem> {
        continue_items(cx).into_iter().find(|i| self.page.cw.get(&i.id) == Some(elem))
    }

    /// OK on a card: a Continue Watching card resumes, a catalog card opens its Detail page.
    fn press(&self, cx: &Cx<'_, Bundle>, elem: u32, fx: &mut Effects<'_, Bundle>) {
        if let Some(item) = self.continue_item(cx, elem) {
            fx.push(Fx::Nav(NavOp::Push(resume_arg(item))));
        } else {
            self.open(cx, elem, fx);
        }
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
        let ((b, k, id), i) = self.page.cards.at.get(&elem)?;
        cx.views.catalog.row_by_identity((b, k, id))?.items().get(*i)
    }

    fn open(&self, cx: &Cx<'_, Bundle>, elem: u32, fx: &mut Effects<'_, Bundle>) {
        if let Some(item) = self.item(cx, elem) {
            fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(item)))));
        }
    }

    /// A held OK on a card: its item menu, anchored beside the card.
    fn menu(&self, cx: &Cx<'_, Bundle>, elem: u32, fx: &mut Effects<'_, Bundle>) {
        let kind = match self.continue_item(cx, elem) {
            Some(i) => MenuKind::Item {
                item: ItemRef { kind: i.kind.clone(), id: i.id.clone(), name: i.name.clone() },
                poster: i.poster.clone(),
            },
            None => {
                let Some(item) = self.item(cx, elem) else { return };
                crate::cards::item_menu(item)
            }
        };
        let anchor = self.stack.focused_card(&self.page, cx, At::Drawn).map_or_else(crate::menu::fallback_anchor, |(_, at)| at.rect);
        let focus = cx.focus.current.filter(|k| k.entry == self.page.entry);
        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
            host: self.page.entry,
            reply: fx.from(),
            focus,
            anchor: MenuArg::anchor_of(anchor),
            kind,
        }))));
    }

    /// The group DOWN from the bar enters: the first row with cards, else the failed install
    /// list's Retry.
    fn first_group(&self, cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        if !continue_items(cx).is_empty() {
            return Some(CW_GROUP);
        }
        match first_seat(cx.views.catalog)? {
            sn_ui::screen::FocusTarget::FirstInGroup(g) => Some(g),
            _ => None,
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
            ScreenEvent::Tick(_) | ScreenEvent::Mount | ScreenEvent::StoreChanged(..) => {
                self.page.cards.sync(cx.views.catalog) | self.sync_continue(cx)
            }
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
                // Home mounts before anything has landed, with the remote resting on its pill: that
                // is a seat nobody chose, so the first row that lands (Continue Watching first)
                // takes it
                let target = self.first_group(cx).map(sn_ui::screen::FocusTarget::FirstInGroup).or_else(|| first_seat(cx.views.catalog));
                let seated = crate::tabs::rests(self.page.entry, crate::tabs::Tab::Home, cx, self.chose, target.is_some());
                let target = target.or(Some(sn_ui::screen::FocusTarget::Elem(sn_machine::machine::FocusKey {
                    entry: self.page.entry,
                    elem: crate::tabs::Tab::Home.elem(),
                })));
                self.seat.tick(seated, target, fx);
                self.first_row = self.first_group(cx);
                if grew {
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _)
                if [StoreId::Catalog.ord(), StoreId::Library.ord(), StoreId::Account.ord()].contains(ord) =>
            {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Activate(elem))) = got {
                    self.press(cx, elem, fx);
                } else if let Some(k) = cx.focus.current.filter(|k| k.entry == self.page.entry && crate::tabs::on_strip(k.elem)) {
                    crate::tabs::on_pill(k.elem, crate::tabs::Tab::Home, self.first_group(cx), fx);
                }
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Hold(elem))) = got {
                    self.menu(cx, elem, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => {
                // BACK to the bar is the viewer's seat: no landing takes it back
                self.chose = true;
                crate::tabs::on_back(self.page.entry, crate::tabs::Tab::Home, cx, fx)
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, crate::tabs::Tab::Home, self.first_group(cx), fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if *elem == RETRY_ELEM => {
                fx.push(Fx::App(AppFx::Catalog(CatalogCmd::RetryAddons)));
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                // a card activated without a press (the pointer's direct activation)
                if self.page.cards.at.contains_key(elem) || self.page.cw.contains(*elem) {
                    self.press(cx, *elem, fx);
                }
                Handled::Yes
            }
            ScreenEvent::FocusMoved { from, by, .. } => {
                // a deliberate move is the viewer's seat: no landing re-seats over it (the
                // arrival seat of a tab press is reported as a direction too, but from nowhere
                // on this page)
                self.chose |= crate::tabs::deliberate(self.page.entry, *from, *by);
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
    fn links(&self, out: &mut Vec<Link>) {
        // the first row's group is the stack's: a row's group is stable per row identity, so the
        // door is re-declared as rows land
        if let Some(first) = self.first_row {
            crate::tabs::links(first, out);
        }
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        sn_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        self.stack.view(&self.page).paint(f);
        self.stack.draw_hold_hint(f.painter.alpha(f.page_alpha), f.measure);
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
