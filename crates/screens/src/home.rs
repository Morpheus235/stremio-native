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
//!   first and cancel what was scrolled past. A row is its catalog's FIRST page and never pages
//!   on (a landing under the cards on screen swapped them); See all opens the whole catalog.
//!
//! **The type dropdown** (`crate::dropdown`, its options `crate::type_filter`'s) stands above the
//! shelves when the rows hold more than one type: OK opens its popover, and a pick keeps only the
//! catalog rows of that type (*All*, first, is every row in today's order); Continue Watching stays. Nothing is fetched again: the rows already loaded are
//! filtered, and the window the store is told counts only the rows shown (`RowWindow`), so a
//! hidden row is never loaded. The choice is kept for the session.
//!
//! **See all.** A catalog row opens its catalog whole (`AppArg::Catalog`, the page Search's MORE
//! opens), Continue Watching the Continue Watching page (`AppArg::Continue`,
//! `crate::continue_page`), four ways, none of them a stop on the way between rows:
//!
//! - the row's **See all tile** after its last loaded card (`CardSource::see_all`, the shared
//!   shelf's tile): RIGHT from the last card reaches it, OK opens the row;
//! - **LEFT on the row's first card** (the shelf's left edge is `EdgeRule::Screen`, and the page
//!   takes the re-delivered key on its DOWN edge only, so a held LEFT walking back stops there);
//! - the remote's **YELLOW** key on any card of the row (Settings lists it);
//! - the row's **heading**, which carries a chevron: the pointer's target. Its group takes no
//!   D-pad landing (`AxisMask(0)`), so UP and DOWN still go row to row (`HomeScreen::links`).
//!
//! **The View toggle** (`crate::item_list`) stands at the dropdown row's right end (Discover's and
//! the Library's place): posters, or each row as a list section — its heading, its first
//! [`LIST_PREVIEW`] items as list rows, then a *See all* row that opens the rest. Continue
//! Watching's rows wear the resume bar and the time left. The choice is a setting
//! (`ViewPage::Home`). Lists stay lazy: the window the store is told is the rows whose section is
//! on screen, and a list row off the screen draws nothing (so asks for no thumbnail).
//!
//! OK on a card opens its Detail page; a held OK opens its item menu (`crate::menu`). The page
//! wears the shared top bar (`crate::tabs`): UP from the first row reaches the dropdown row, then
//! the bar, BACK on a row returns focus to the bar, and BACK on the bar is the application's (the
//! television's Home).

use std::collections::HashMap;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, Fx, GroupId, Handled, InputEvent, InputKind, Key, LogicalState, Machine, NavOp,
};
use sn_ui::screen::Dir;
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::types::MetaPreview;
use sn_ui::cards::{CardEvent, CardSource, Kind, RowStyle, SectionSpec, Stack, StackEvent, StackPage, TileLabel};
use sn_ui::consts::{CARD_H, MARGIN_X, SCR_H, SCR_W};
use sn_ui::label::{Label, VAlign};
use sn_ui::linked_heading::LinkedHeading;
use sn_ui::screen::{At, DrawFrame, EdgeRule, FocusSource, Focusable, GroupSpec, HitSource, Link, RenderStrategy, Screen, ScreenEvent};
use sn_ui::widgets::{Art, StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect, View};

use crate::cards::item_ref;
use crate::item_cards::{self, ItemCards};
use crate::item_list::{self, ListRow};
use crate::registry::{kind_label, word, AppArg, AppFx, AppMsg, Bundle, CatalogCmd, CatalogRef, ChoiceTarget, FirstSeat, MenuArg, SettingsCmd};
use crate::stores::grid::GridKey;
use crate::stores::settings::ViewPage;
use crate::stores::catalog::{AddonsState, CatalogStore, Row, RowId, RowStatus, RowWindow};
use crate::stores::StoreId;
use crate::dropdown::{self, Dropdown};
use crate::type_filter;
use sn_stremio::library::LibraryItem;

/// The first card element; every card of every row is interned above it, never reused.
const FIRST_CARD_ELEM: u32 = 0x1000;
/// The first Continue Watching element: far above any catalog card's.
const FIRST_CW_ELEM: u32 = 0x4000_0000;
const CW_GROUP: GroupId = GroupId(0x7FFF_0000);
/// The failed install list's Retry action (a key namespace, not an index).
const RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 1;
/// The same Retry when the read-out stands as a band under Continue Watching ([`Sec::Band`]).
pub const BAND_RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 3;
const BAND_GROUP: GroupId = GroupId(0x7FFF_0004);
/// The band's height: the verdict, a two-line reason and the Try again pill, with air.
const BAND_H: f32 = 320.0;
/// The band's verdict frame (the reason and the pill hang below it, inside the band).
const BAND_VERDICT_H: f32 = 96.0;
/// A failed row's Try again: [`row_retry_elem`] (a key namespace, not an index).
const ROW_RETRY_BASE: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x0200_0000;
const STATUS_GROUP: GroupId = GroupId(0);
/// The type dropdown's group and element (a key namespace, not an index).
const TYPES_GROUP: GroupId = GroupId(0x7FFF_0001);
pub const TYPE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x100;
/// Continue Watching's See all: its shelf's tile, or its list's last row.
pub const CW_MORE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x101;
/// Continue Watching's heading's group.
const CW_MORE_GROUP: GroupId = GroupId(0x7FFF_0002);
/// List mode: Continue Watching's See all row's group.
const CW_SEE_ALL_GROUP: GroupId = GroupId(0x7FFF_0003);
/// The View toggle, at the dropdown row's right end.
pub const TOGGLE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x102;
/// A catalog row's See all: [`more_elem`] (a key namespace, not an index).
const MORE_BASE: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x0100_0000;
/// A catalog row's heading, the pointer's way to its See all: [`head_elem`].
const HEAD_BASE: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x0300_0000;
/// Continue Watching's heading.
const CW_HEAD_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x103;
/// List mode shows a row's first items; its See all row opens the rest.
pub const LIST_PREVIEW: usize = 10;
/// The rank of the headings and the dropdown row: after every row, so the engine's seat on a page
/// with no focus is the first card, never the dropdown.
const LAST_RANK: u32 = u32::MAX;
/// The page's top: the first shelf's heading sits this far down, clear of the shared top bar.
pub const CONTENT_TOP: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
/// From a shelf's top to its cards: the heading's band.
pub const HEAD_TO_ROW: f32 = 64.0;
/// The heading's cap top, inside its band.
const HEADING_TOP: f32 = 10.0;
static POSTER: RowStyle = RowStyle::HOME;

/// The page's sections, in document order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sec {
    /// The top band; the install list's read-out while it has no row to show.
    Status,
    /// The type dropdown (`crate::dropdown`).
    Types,
    /// The account library's items in progress.
    Continue,
    Row(RowId),
    /// Continue Watching's heading: a pointer stop with a chevron, never a D-pad stop.
    ContinueMore,
    /// A catalog row's heading: a pointer stop with a chevron, never a D-pad stop.
    More(RowId),
    /// List mode: Continue Watching's item `i`. Its element is the card's.
    ContinueItem(usize),
    /// List mode: item `i` of a catalog row. Its element is the card's.
    Item(RowId, usize),
    /// List mode: Continue Watching's last row, *See all* ([`CW_MORE_ELEM`]).
    ContinueSeeAll,
    /// List mode: a catalog row's last row, *See all* ([`more_elem`]).
    SeeAll(RowId),
    /// A row whose first page failed: its heading, the reason and a *Try again* pill, where the
    /// cards would stand. Not a shelf, because a shelf with no cards and no more to come is out of
    /// the flow.
    Failed(RowId),
    /// The install list's read-out (loading, failed with its Try again, nothing to browse) as a
    /// band under Continue Watching, which keeps the page.
    Band,
}

/// A failed row's band under its heading.
const FAILED_BAND: f32 = CARD_H * 0.5;

/// A row's focus group: stable per row identity, never a position.
fn row_group(id: RowId) -> GroupId {
    GroupId(1 + ((u32::from(id.source) << 12) | u32::from(id.catalog)))
}

/// A failed row's *Try again* element.
pub fn row_retry_elem(id: RowId) -> u32 {
    ROW_RETRY_BASE + row_group(id).0
}

fn is_row_retry(elem: u32) -> bool {
    (ROW_RETRY_BASE..ROW_RETRY_BASE + 0x0100_0000).contains(&elem)
}

/// A failed row's *Try again* pill inside its section `r`: under the note, at the row's margin.
fn row_retry_rect(r: Rect, measure: &dyn sn_machine::machine::Measure) -> Rect {
    let w = sn_ui::widgets::Button::pill_w_measured(msg::app_home_retry_c(), theme::size::LABEL, false, false, measure);
    Rect::new(MARGIN_X, r.y + HEAD_TO_ROW + FAILED_BAND - StatusOverlay::CTRL_H - theme::space::SM, w, StatusOverlay::CTRL_H)
}

/// A catalog row's See all element (its shelf's tile, or its list's last row).
pub fn more_elem(id: RowId) -> u32 {
    MORE_BASE + row_group(id).0
}

/// A catalog row's heading element: a pointer stop (D-pad moves never land on it) that opens the
/// row's See all.
fn head_elem(id: RowId) -> u32 {
    HEAD_BASE + row_group(id).0
}

/// The See all a heading element opens, or `None` for any other element.
fn heading_see_all(elem: u32) -> Option<u32> {
    match elem {
        CW_HEAD_ELEM => Some(CW_MORE_ELEM),
        e if (HEAD_BASE..HEAD_BASE + 0x0100_0000).contains(&e) => Some(e - HEAD_BASE + MORE_BASE),
        _ => None,
    }
}

/// Is `elem` a catalog row's See all?
fn is_more(elem: u32) -> bool {
    (MORE_BASE..MORE_BASE + 0x0100_0000).contains(&elem)
}

fn more_group(id: RowId) -> GroupId {
    GroupId(0x4000_0000 | row_group(id).0)
}

/// List mode: a catalog row's See all row's group.
fn see_all_group(id: RowId) -> GroupId {
    GroupId(0x5000_0000 | row_group(id).0)
}

/// List mode: the group of item `i` of a catalog row (one per row, `item_list::group`).
fn item_group(id: RowId, i: usize) -> GroupId {
    GroupId(0x2000_0000 | ((row_group(id).0 & 0xF_FFFF) << 8) | (i as u32 & 0xFF))
}

fn cw_item_group(i: usize) -> GroupId {
    GroupId(0x7FFE_0000 + i as u32)
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
    pub(crate) row: &'a Row,
    pub(crate) elems: &'a [u32],
    pub(crate) at: &'a HashMap<u32, (RowIdent, usize)>,
    /// The library whose watched titles wear the tick (`cards::ItemCards`' rule).
    pub(crate) watched: Option<&'a crate::stores::library::LibraryStore>,
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
        let watched = |item: &MetaPreview| self.watched.and_then(|l| l.item(&item.id)).is_some_and(|i| i.watched());
        Art::Poster(self.row.items().get(i).map(|item| sn_ui::tile::TileFacts { watched: watched(item), ..tile_facts(item) }))
    }
    fn label(&self, i: usize) -> TileLabel {
        match self.row.items().get(i) {
            Some(item) => TileLabel::titled(&item.name, item.release_info.as_deref().unwrap_or("")),
            None => TileLabel::default(),
        }
    }
    /// A Home row is its catalog's first page: more is coming only until that page lands, then it
    /// never pages on (a page landing under the cards on screen swapped them for others; the rest
    /// of the catalog is See all's).
    fn more(&self) -> bool {
        self.row.items().is_empty() && self.row.pager.has_more()
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
    /// Every row ends in its See all tile; a row whose next page failed in its Try again tile.
    fn see_all(&self) -> Option<u32> {
        Some(match self {
            HomeCards::Row(c) if self.see_all_retry() => row_retry_elem(c.row.id),
            HomeCards::Row(c) => more_elem(c.row.id),
            HomeCards::Continue(_) => CW_MORE_ELEM,
        })
    }
    fn see_all_retry(&self) -> bool {
        matches!(self, HomeCards::Row(c) if matches!(c.row.status(), RowStatus::Failed(_)))
    }
}

/// The page's content, read by every section. The `Stack` is the screen's other field, so
/// `self.stack.on(&self.page, ..)` borrows the two apart.
struct Page {
    entry: EntryId,
    cards: Interned,
    /// Continue Watching's cards, by library item id.
    cw: item_cards::Interned,
    /// The type dropdown and the type the rows are narrowed to.
    types: Dropdown,
    /// The rows show as lists (the setting, as of the last event).
    list: bool,
    /// List mode: each list row's element (its card's), as of the last event (`elem_of` has no
    /// `Cx`).
    list_elems: HashMap<Sec, u32>,
}

pub use crate::item_cards::{continue_items, resume_arg};

impl Page {
    /// The dropdown row stands once a row holds cards: a dropdown focusable before that took the
    /// boot's first seat, and the seat waits for an empty focus to put the first card under it. ANY
    /// row, not one of the chosen type: a type whose rows have not landed yet is a pick the viewer
    /// just made, and the button they made it on (focused) must not leave the page until they do.
    fn bar(&self, cx: &Cx<'_, Bundle>) -> bool {
        !continue_items(cx).is_empty() || shown_rows(Self::store(cx), None).any(|(_, r)| !r.items().is_empty())
    }

    /// The row's elements: the type dropdown (when there are types), then the View toggle.
    fn bar_elems(&self) -> Vec<u32> {
        self.types.shown().then_some(self.types.elem()).into_iter().chain([TOGGLE_ELEM]).collect()
    }

    /// The row's rects at `y`, as [`bar_elems`](Self::bar_elems).
    fn bar_rects(&self, measure: &dyn sn_machine::machine::Measure, y: f32) -> Vec<Rect> {
        let widths: Vec<f32> = self.types.shown().then(|| self.types.width(measure)).into_iter().collect();
        dropdown::row(&widths, Some(item_list::toggle_width(measure)), y)
    }

    /// The type dropdown's rect, its section at `y`.
    fn type_rect(&self, measure: &dyn sn_machine::machine::Measure, y: f32) -> Rect {
        self.bar_rects(measure, y)[0]
    }

    /// A heading's title and annotation, and the See all element of its row (`None`: not a
    /// heading, or its row is gone).
    fn heading(cx: &Cx<'_, Bundle>, k: Sec) -> Option<(String, String, u32)> {
        match k {
            Sec::ContinueMore => Some((msg::app_home_continue().to_string(), String::new(), CW_MORE_ELEM)),
            Sec::More(id) => {
                let store = Self::store(cx);
                let row = store.row(id)?;
                Some((row.title().to_string(), Self::source_line(store, row), more_elem(id)))
            }
            _ => None,
        }
    }

    /// The See all of the row `elem` stands in (a card, a list row, or the See all itself).
    fn see_all_of(&self, cx: &Cx<'_, Bundle>, elem: u32) -> Option<u32> {
        if elem == CW_MORE_ELEM || is_more(elem) {
            return Some(elem);
        }
        if let Some(e) = heading_see_all(elem) {
            return Some(e);
        }
        if self.cw.contains(elem) {
            return Some(CW_MORE_ELEM);
        }
        let ((b, k, id), _) = self.cards.at.get(&elem)?;
        Self::store(cx).row_by_identity((b, k, id)).map(|r| more_elem(r.id))
    }

    /// List mode: how many of a row's items it lists.
    fn listed(&self, k: impl Fn(usize) -> Sec) -> usize {
        (0..LIST_PREVIEW).take_while(|i| self.list_elems.contains_key(&k(*i))).count()
    }

    fn store<'a>(cx: &Cx<'a, Bundle>) -> &'a CatalogStore {
        cx.views.catalog
    }

    /// The read-out the status section shows, if any: while the install list has no row (of the
    /// type `kind`, when one is chosen).
    fn status(store: &CatalogStore, kind: Option<&str>) -> Option<StatusKind> {
        match store.addons_state() {
            AddonsState::Loading => Some(StatusKind::Working),
            AddonsState::Failed => Some(StatusKind::Failed),
            AddonsState::Ready if shown_rows(store, kind).next().is_none() && !loading_more(store, kind) => Some(StatusKind::Empty),
            AddonsState::Ready => None,
        }
    }

    fn status_overlay<'a>(store: &CatalogStore, types: Option<&str>, ms: u32) -> Option<StatusOverlay<'a>> {
        let kind = Self::status(store, types)?;
        Some(match kind {
            StatusKind::Working => StatusOverlay::new(Rect::FULL, msg::app_home_loading_addons_c(), StatusKind::Working).phase(ms),
            StatusKind::Failed => StatusOverlay::new(Rect::FULL, msg::app_home_addons_failed_c(), StatusKind::Failed)
                .reason(msg::app_home_addons_failed_reason_c())
                .action(msg::app_home_retry_c())
                .page(sn_ui::icons::Icon::Alert),
            _ => StatusOverlay::new(Rect::FULL, msg::app_home_nothing_c(), StatusKind::Empty),
        })
    }

    /// The same read-out as a band at `top` under Continue Watching: in the page's flow, no glyph.
    fn band_overlay<'a>(store: &CatalogStore, types: Option<&str>, ms: u32, top: f32) -> Option<StatusOverlay<'a>> {
        let kind = Self::status(store, types)?;
        let frame = |h: f32| Rect::new(0.0, top, SCR_W, h);
        Some(match kind {
            StatusKind::Working => StatusOverlay::new(frame(BAND_H), msg::app_home_loading_addons_c(), StatusKind::Working).phase(ms),
            StatusKind::Failed => StatusOverlay::new(frame(BAND_VERDICT_H), msg::app_home_addons_failed_c(), StatusKind::Failed)
                .reason(msg::app_home_addons_failed_reason_c())
                .action(msg::app_home_retry_c()),
            _ => StatusOverlay::new(frame(BAND_H * 0.5), msg::app_home_nothing_c(), StatusKind::Empty),
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

/// The rows Home shows: every row of the chosen type (every row for `None`) but one whose catalog
/// ended empty, each with its position among the rows of that type (the store's window counts).
fn shown_rows<'a>(store: &'a CatalogStore, kind: Option<&'a str>) -> impl Iterator<Item = (usize, &'a Row)> + 'a {
    store.rows_of(kind).filter(|(_, r)| r.status() != RowStatus::Empty)
}

/// Where focus goes on a Home that has none yet: the first row with cards, else the failed install
/// list's Retry. In list mode a row's first card is its first list row.
fn first_seat(store: &CatalogStore, kind: Option<&str>, list: bool) -> Option<sn_ui::screen::FocusTarget<u32>> {
    use sn_ui::screen::FocusTarget;
    shown_rows(store, kind)
        .find(|(_, r)| !r.items().is_empty())
        .map(|(_, r)| FocusTarget::FirstInGroup(if list { item_group(r.id, 0) } else { row_group(r.id) }))
        .or_else(|| (Page::status(store, kind) == Some(StatusKind::Failed)).then_some(FocusTarget::FirstInGroup(STATUS_GROUP)))
}

/// A row that failed before any card landed (it shows a note, not a shelf).
fn failed_empty(row: &Row) -> bool {
    matches!(row.status(), RowStatus::Failed(_)) && row.items().is_empty()
}

/// Is some row of the type shown still waiting for its first page (so an empty page is not yet
/// "nothing")?
fn loading_more(store: &CatalogStore, kind: Option<&str>) -> bool {
    store.rows_of(kind).any(|(_, r)| r.status() == RowStatus::Loading)
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = HomeCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        let library = u64::from(cx.views.library.gen()) ^ (u64::from(cx.views.account.gen()) << 16);
        (u64::from(Self::store(cx).gen()) << 32)
            ^ (library << 20)
            ^ (self.cards.cards() + self.cw.len()) as u64
            ^ (self.types.rev() << 44)
            ^ (u64::from(self.list) << 63)
            ^ ((self.list_elems.len() as u64) << 52)
    }

    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        let store = Self::store(cx);
        let kind = self.types.kind();
        let cw = !continue_items(cx).is_empty();
        let status = Self::status(store, kind);
        match Self::status_overlay(store, kind, 0).filter(|_| status == Some(StatusKind::Failed) && !cw) {
            Some(overlay) => {
                let rect = overlay.action_frame_measured(cx.measure).unwrap_or(Rect::FULL);
                out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: true }, STATUS_GROUP));
            }
            None => out.push(SectionSpec::new(Sec::Status, Kind::Custom { height: CONTENT_TOP, focusable: false }, STATUS_GROUP)),
        }
        if self.bar(cx) {
            out.push(SectionSpec::new(Sec::Types, Kind::Custom { height: dropdown::BAND, focusable: true }, TYPES_GROUP).ranked(LAST_RANK));
        }
        // a heading is the pointer's way to the row's See all; its group takes no D-pad landing
        // (`plain_group`), so UP and DOWN still go row to row
        let heading = |k: Sec, group: GroupId| SectionSpec::new(k, Kind::Custom { height: HEAD_TO_ROW, focusable: true }, group).ranked(LAST_RANK);
        let item = |k: Sec, group: GroupId, rank: u32| SectionSpec::new(k, Kind::Custom { height: item_list::ROW_H, focusable: true }, group).ranked(rank);
        // LEFT past a shelf's first card is re-delivered to the page, which opens the row's See all
        let shelf = |k: Sec, group: GroupId, rank: u32| {
            SectionSpec::new(k, Kind::Shelf { style: &POSTER, heading: 0.0 }, group)
                .ranked(rank)
                .edges([EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Screen, EdgeRule::Geometric])
        };
        let first = u32::from(cw);
        if cw {
            out.push(heading(Sec::ContinueMore, CW_MORE_GROUP));
            if self.list {
                // Continue Watching's list rows carry the resume bar and the time left
                // (`ListRow::of_library`), so it lists like every other row
                let n = self.listed(Sec::ContinueItem);
                for i in 0..n {
                    out.push(item(Sec::ContinueItem(i), cw_item_group(i), 0));
                }
                if n > 0 {
                    out.push(item(Sec::ContinueSeeAll, CW_SEE_ALL_GROUP, 0));
                }
            } else {
                out.push(shelf(Sec::Continue, CW_GROUP, 0));
            }
            // the install list's read-out keeps out of Continue Watching's way: a band under it
            if status.is_some() {
                let focusable = status == Some(StatusKind::Failed);
                out.push(SectionSpec::new(Sec::Band, Kind::Custom { height: BAND_H, focusable }, BAND_GROUP).ranked(1));
            }
        }
        for (rank, (_, row)) in shown_rows(store, kind).enumerate() {
            let rank = rank as u32 + first;
            if failed_empty(row) {
                let group = row_group(row.id);
                out.push(SectionSpec::new(Sec::Failed(row.id), Kind::Custom { height: HEAD_TO_ROW + FAILED_BAND, focusable: true }, group).ranked(rank));
                continue;
            }
            out.push(heading(Sec::More(row.id), more_group(row.id)));
            if self.list {
                // a row still waiting for its first page is its heading alone until it lands
                let n = self.listed(|i| Sec::Item(row.id, i));
                for i in 0..n {
                    out.push(item(Sec::Item(row.id, i), item_group(row.id, i), rank));
                }
                if n > 0 {
                    out.push(item(Sec::SeeAll(row.id), see_all_group(row.id), rank));
                }
            } else {
                out.push(shelf(Sec::Row(row.id), row_group(row.id), rank));
            }
        }
    }

    fn fallback(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        if !continue_items(cx).is_empty() {
            out.extend([Sec::Continue, Sec::ContinueItem(0), Sec::ContinueSeeAll]);
        }
        out.push(Sec::Band);
        for (_, r) in shown_rows(Self::store(cx), self.types.kind()) {
            if failed_empty(r) {
                out.push(Sec::Failed(r.id));
            } else {
                out.extend([Sec::Row(r.id), Sec::Item(r.id, 0), Sec::SeeAll(r.id)]);
            }
        }
        out.push(Sec::Types);
        out.push(Sec::Status);
    }

    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<HomeCards<'a>> {
        match k {
            Sec::Continue => Some(HomeCards::Continue(ItemCards { items: continue_items(cx), cards: &self.cw })),
            Sec::Row(id) => {
                let row = Self::store(cx).row(id)?;
                let elems = self.cards.rows.get(&ident(row)).map_or(&[][..], Vec::as_slice);
                Some(HomeCards::Row(RowCards { row, elems, at: &self.cards.at, watched: Some(cx.views.library) }))
            }
            _ => None,
        }
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Status => Some(RETRY_ELEM),
            Sec::Band => Some(BAND_RETRY_ELEM),
            Sec::Failed(id) => Some(row_retry_elem(id)),
            Sec::ContinueSeeAll => Some(CW_MORE_ELEM),
            Sec::SeeAll(id) => Some(more_elem(id)),
            Sec::ContinueMore => Some(CW_HEAD_ELEM),
            Sec::More(id) => Some(head_elem(id)),
            Sec::Item(..) | Sec::ContinueItem(_) => self.list_elems.get(&k).copied(),
            _ => None,
        }
    }

    fn plain_len(&self, k: Sec) -> usize {
        match k {
            Sec::Types => self.bar_elems().len(),
            _ => self.elem_of(k).is_some() as usize,
        }
    }

    fn plain_elem(&self, k: Sec, n: usize) -> Option<u32> {
        match k {
            Sec::Types => self.bar_elems().get(n).copied(),
            _ if n == 0 => self.elem_of(k),
            _ => None,
        }
    }

    fn plain_step(&self, k: Sec, at: usize, dir: Dir) -> Option<usize> {
        match (k, dir) {
            (Sec::Types, Dir::Left) => at.checked_sub(1),
            (Sec::Types, Dir::Right) => (at + 1 < self.bar_elems().len()).then_some(at + 1),
            _ => None,
        }
    }

    fn element_rect(&self, cx: &Cx<'_, Bundle>, k: Sec, n: usize, section: Rect) -> Rect {
        match k {
            Sec::Types => self.bar_rects(cx.measure, section.y).get(n).copied().unwrap_or(section),
            Sec::Item(..) | Sec::ContinueItem(_) | Sec::SeeAll(_) | Sec::ContinueSeeAll => item_list::face_rect(section),
            Sec::Failed(_) => row_retry_rect(section, cx.measure),
            Sec::More(_) | Sec::ContinueMore => {
                let Some((title, note, _)) = Self::heading(cx, k) else { return section };
                let h = LinkedHeading::heading(&title, &note).bounded(SCR_W - 2.0 * MARGIN_X);
                let m = h.measure(cx.measure);
                h.face_rect(MARGIN_X, section.y + HEADING_TOP, 1.0, &m)
            }
            Sec::Band => Self::band_overlay(Self::store(cx), self.types.kind(), 0, section.y)
                .and_then(|o| o.action_frame_measured(cx.measure))
                .unwrap_or(section),
            _ => section,
        }
    }

    fn plain_group(&self, _cx: &Cx<'_, Bundle>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        use sn_ui::screen::{AxisMask, ElemKind, GroupKind, Seat};
        match k {
            Sec::Types => dropdown::group(id, extent, self.bar_elems().len()),
            Sec::Item(..) | Sec::ContinueItem(_) | Sec::SeeAll(_) | Sec::ContinueSeeAll => item_list::group(id, extent),
            // the pointer's only: no D-pad move lands here; one that leaves goes to the rows
            Sec::More(_) | Sec::ContinueMore => GroupSpec {
                id,
                kind: GroupKind::Free,
                seat: Seat::First,
                reachable: AxisMask(0),
                edge: [EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop],
                extent,
                len: 1,
                elem: ElemKind::Control,
            },
            _ => GroupSpec {
                id,
                kind: GroupKind::Free,
                seat: Seat::First,
                reachable: AxisMask::BOTH,
                edge: [EdgeRule::Stop; 4],
                extent,
                len: 1,
                elem: ElemKind::Bare,
            },
        }
    }

    /// Every card opens its item menu on a held OK; a row's See all tile does not.
    fn card_has_menu(&self, _cx: &Cx<'_, Bundle>, _k: Sec, elem: &u32) -> bool {
        self.cards.at.contains_key(elem) || self.cw.contains(*elem)
    }

    /// A shelf scrolled back into view stops under the top bar with its heading (the section
    /// above it) in view, not under the screen's edge.
    fn reveal_margin(&self, k: Sec) -> f32 {
        match k {
            Sec::Row(_) | Sec::Continue => CONTENT_TOP + HEAD_TO_ROW,
            _ => CONTENT_TOP,
        }
    }

    /// So do the dropdown row and a list row: stopped at the screen's edge, the tab bar's pills
    /// covered them.
    fn reveal_under_margin(&self, k: Sec) -> bool {
        matches!(k, Sec::Types | Sec::Item(..) | Sec::ContinueItem(_) | Sec::SeeAll(_) | Sec::ContinueSeeAll)
    }

    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        let focused = |f: &DrawFrame<'_, '_, Bundle>, elem: Option<u32>| f.focus.current.is_some_and(|key| key.entry == self.entry && Some(key.elem) == elem);
        match k {
            Sec::Types => {
                let rects = self.bar_rects(f.measure, r.y);
                if self.types.shown() {
                    self.types.draw(f, rects[0]);
                }
                if let Some(chip) = rects.last() {
                    let on = focused(f, Some(TOGGLE_ELEM));
                    item_list::draw_toggle(f.painter.alpha(f.page_alpha), *chip, self.list, on);
                }
            }
            Sec::More(_) | Sec::ContinueMore => {
                if r.y >= SCR_H || r.y + r.h <= 0.0 {
                    return;
                }
                let Some((title, note, _)) = Self::heading(f.cx, k) else { return };
                let p = f.painter.alpha(f.page_alpha);
                let h = LinkedHeading::heading(&title, &note).bounded(SCR_W - 2.0 * MARGIN_X);
                let m = h.measure(f.measure);
                let t = if focused(f, self.elem_of(k)) { 1.0 } else { 0.0 };
                h.draw(p, MARGIN_X, r.y + HEADING_TOP, t, &m, f.measure);
            }
            Sec::SeeAll(_) | Sec::ContinueSeeAll => {
                let on = focused(f, self.elem_of(k));
                item_list::draw_see_all(f, r, on);
            }
            Sec::ContinueItem(i) => {
                let Some(item) = continue_items(f.cx).get(i).copied() else { return };
                let on = focused(f, self.elem_of(k));
                item_list::draw(f, r, &ListRow::of_library(item), on);
            }
            Sec::Item(id, i) => {
                let store = Self::store(f.cx);
                let Some(row) = store.row(id) else { return };
                let Some(item) = row.items().get(i) else { return };
                let on = focused(f, self.elem_of(k));
                let source = (store.sources().len() > 1).then_some(row.addon_name.as_str());
                item_list::draw(f, r, &ListRow::of_meta(item, source), on);
            }
            Sec::Failed(id) => {
                let store = Self::store(f.cx);
                let Some(row) = store.row(id) else { return };
                let p = f.painter.alpha(f.page_alpha);
                let source = Self::source_line(store, row);
                sn_ui::cards::draw_heading(p, row.title(), &source, MARGIN_X, r.y + HEADING_TOP, SCR_W - 2.0 * MARGIN_X, f.measure);
                // the strip the cards would have stood in: the reason, and Try again under it
                let note = msg::app_home_row_failed_c();
                let pill = row_retry_rect(r, f.measure);
                let line = Rect::new(MARGIN_X, r.y + HEAD_TO_ROW, SCR_W - 2.0 * MARGIN_X, pill.y - r.y - HEAD_TO_ROW);
                Label::new(note.as_ptr(), theme::size::BODY, theme::TEXT_SECONDARY).v(VAlign::Middle).draw(p, line);
                let on = focused(f, Some(row_retry_elem(id)));
                sn_ui::widgets::Button::new(msg::app_home_retry_c().as_ptr(), theme::size::LABEL, pill).focused(on).draw(&Env::inert(), p);
            }
            Sec::Status => {
                // with Continue Watching on the page the read-out is its band (below)
                if continue_items(f.cx).is_empty() {
                    let store = Self::store(f.cx);
                    if let Some(overlay) = Self::status_overlay(store, self.types.kind(), f.cx.tick.ms) {
                        let on = focused(f, Some(RETRY_ELEM));
                        overlay.focused(on).draw_measured(&Env::inert(), f.painter.alpha(f.page_alpha), f.measure);
                    }
                }
            }
            Sec::Band => {
                if r.y >= SCR_H || r.y + r.h <= 0.0 {
                    return;
                }
                let store = Self::store(f.cx);
                if let Some(overlay) = Self::band_overlay(store, self.types.kind(), f.cx.tick.ms, r.y) {
                    let on = focused(f, Some(BAND_RETRY_ELEM));
                    overlay.focused(on).draw_measured(&Env::inert(), f.painter.alpha(f.page_alpha), f.measure);
                }
            }
            Sec::Continue | Sec::Row(_) => {}
        }
    }
}

pub struct HomeScreen {
    page: Page,
    stack: Stack<Sec>,
    /// The rows last reported on screen, as positions among the rows of the type shown.
    view: Option<RowWindow>,
    /// Home mounts before its rows land: the first one that does takes focus.
    seat: FirstSeat,
    /// The group DOWN from the bar enters (the type dropdown, else the first row), as of the last tick
    /// (`Screen::links` has no `Cx`).
    first_row: Option<GroupId>,
    /// The viewer has moved focus themselves.
    chose: bool,
    /// UP and DOWN from row to row (as of the last tick).
    down: Vec<Link>,
}

impl HomeScreen {
    pub const SHAPE: &'static str = "HomeScreen{entry:u32,cards:u32,view:Option<(u32,u32)>,stack:Stack}";

    pub fn new(entry: EntryId) -> Self {
        HomeScreen {
            page: Page {
                entry,
                cards: Interned::default(),
                cw: item_cards::Interned::with_base(FIRST_CW_ELEM),
                types: Dropdown::new(type_filter::Page::Home, entry, TYPE_ELEM).all_label(msg::app_filter_all_types),
                list: false,
                list_elems: HashMap::new(),
            },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Library),
            view: None,
            seat: FirstSeat::new(),
            first_row: None,
            chose: false,
            down: Vec::new(),
        }
    }

    /// The list setting and, in list mode, each list row's element; `true` when either moved.
    fn sync_list(&mut self, cx: &Cx<'_, Bundle>) -> bool {
        let list = cx.views.settings.list(ViewPage::Home);
        let mut elems = HashMap::new();
        if list {
            for (i, item) in continue_items(cx).into_iter().take(LIST_PREVIEW).enumerate() {
                if let Some(e) = self.page.cw.get(&item.id) {
                    elems.insert(Sec::ContinueItem(i), e);
                }
            }
            for (_, row) in shown_rows(cx.views.catalog, self.page.types.kind()) {
                let interned = self.page.cards.rows.get(&ident(row)).map_or(&[][..], Vec::as_slice);
                for (i, e) in interned.iter().take(row.items().len().min(LIST_PREVIEW)).enumerate() {
                    elems.insert(Sec::Item(row.id, i), *e);
                }
            }
        }
        let changed = list != self.page.list || elems != self.page.list_elems;
        self.page.list = list;
        self.page.list_elems = elems;
        changed
    }

    /// The groups UP and DOWN walk, a row each: each row's first and last group (a shelf is one;
    /// a list is its rows', its See all row last), in page order, rows with nothing to focus left
    /// out.
    fn row_doors(&self, cx: &Cx<'_, Bundle>) -> Vec<(GroupId, GroupId)> {
        let mut doors = Vec::new();
        let list = self.page.list;
        if !continue_items(cx).is_empty() {
            let n = self.page.listed(Sec::ContinueItem);
            match list {
                true if n > 0 => doors.push((cw_item_group(0), CW_SEE_ALL_GROUP)),
                true => {}
                false => doors.push((CW_GROUP, CW_GROUP)),
            }
            if Page::status(cx.views.catalog, self.page.types.kind()) == Some(StatusKind::Failed) {
                doors.push((BAND_GROUP, BAND_GROUP));
            }
        }
        for (_, row) in shown_rows(cx.views.catalog, self.page.types.kind()) {
            if failed_empty(row) {
                doors.push((row_group(row.id), row_group(row.id)));
                continue;
            }
            if row.items().is_empty() {
                continue;
            }
            let n = self.page.listed(|i| Sec::Item(row.id, i));
            match list {
                true if n > 0 => doors.push((item_group(row.id, 0), see_all_group(row.id))),
                true => {}
                false => doors.push((row_group(row.id), row_group(row.id))),
            }
        }
        doors
    }

    /// DOWN from the dropdown row and from each row lands on the next row, UP on the row above
    /// (the dropdown row from the first): one press a row each way, the headings between them
    /// drawn, never stops.
    fn row_links(&self, cx: &Cx<'_, Bundle>) -> Vec<Link> {
        let doors = self.row_doors(cx);
        let mut out = Vec::new();
        if let (true, Some((first, _))) = (self.page.bar(cx), doors.first()) {
            out.push(Link { from: TYPES_GROUP, dir: Dir::Down, to: *first });
            out.push(Link { from: *first, dir: Dir::Up, to: TYPES_GROUP });
        }
        for w in doors.windows(2) {
            out.push(Link { from: w[0].1, dir: Dir::Down, to: w[1].0 });
            out.push(Link { from: w[1].0, dir: Dir::Up, to: w[0].1 });
        }
        out
    }

    /// The View toggle: posters ⇄ list.
    fn toggle(&self, fx: &mut Effects<'_, Bundle>) {
        fx.push(Fx::App(AppFx::Settings(SettingsCmd::List(ViewPage::Home, !self.page.list))));
        fx.invalidate(Provenance::Input);
    }

    /// A catalog row's See all: the catalog whole, the page Search's MORE opens.
    fn open_catalog(&self, cx: &Cx<'_, Bundle>, elem: u32, fx: &mut Effects<'_, Bundle>) {
        let store = cx.views.catalog;
        let Some(row) = store.rows().iter().find(|r| more_elem(r.id) == elem) else { return };
        let Some(addon) = store.addons().iter().find(|a| a.base == row.pager.base) else { return };
        let key = GridKey {
            addon_id: addon.manifest.id.clone(),
            kind: row.pager.catalog.kind.clone(),
            catalog_id: row.pager.catalog.id.clone(),
            genre: row.pager.genre.clone(),
            search: None,
        };
        fx.push(Fx::Nav(NavOp::Push(AppArg::Catalog(CatalogRef { key, title: format!("{} \u{2014} {}", row.addon_name, row.title()) }))));
    }

    /// OK or a click on one of the page's own controls; `false` when `elem` is none of them.
    fn activate(&self, cx: &Cx<'_, Bundle>, elem: u32, fx: &mut Effects<'_, Bundle>) -> bool {
        match elem {
            TOGGLE_ELEM => self.toggle(fx),
            CW_MORE_ELEM => fx.push(Fx::Nav(NavOp::Push(AppArg::Continue))),
            e if heading_see_all(e).is_some() => return heading_see_all(e).is_some_and(|m| self.activate(cx, m, fx)),
            e if is_more(e) => self.open_catalog(cx, e, fx),
            RETRY_ELEM | BAND_RETRY_ELEM => fx.push(Fx::App(AppFx::Catalog(CatalogCmd::RetryAddons))),
            e if is_row_retry(e) => {
                if let Some(row) = cx.views.catalog.rows().iter().find(|r| row_retry_elem(r.id) == e) {
                    fx.push(Fx::App(AppFx::Catalog(CatalogCmd::Retry(row.id))));
                }
            }
            // a list row: its card's element (the Stack reports no press for a plain element)
            e if self.page.cards.at.contains_key(&e) || self.page.cw.contains(e) => self.press(cx, e, fx),
            _ => return false,
        }
        true
    }

    /// The See all of the row focus is in (YELLOW, or LEFT past the row's first card): its whole
    /// catalog, or the Continue Watching page. Nothing when focus is on no row.
    fn see_all(&self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let focus = cx.focus.current.filter(|k| k.entry == self.page.entry);
        if let Some(e) = focus.and_then(|k| self.page.see_all_of(cx, k.elem)) {
            self.activate(cx, e, fx);
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
            fx.push(Fx::Nav(NavOp::Push(resume_arg(item, msg::app_nav_home()))));
        } else {
            self.open(cx, elem, fx);
        }
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    /// The store rows whose shelves (or list sections) are on screen now (or a row's worth at the top, before the
    /// layout exists).
    fn rows_in_view(&self, cx: &Cx<'_, Bundle>) -> std::ops::Range<usize> {
        let store = cx.views.catalog;
        let mut lo = usize::MAX;
        let mut hi = 0;
        for (i, row) in shown_rows(store, self.page.types.kind()) {
            // a row's section starts at its heading; a list is as tall as its rows (one at least,
            // so a row still waiting for its first page is asked for once its heading shows)
            let Some(frame) = self.stack.shelf_frame(&self.page, cx, Sec::More(row.id)) else { continue };
            let listed = self.page.listed(|n| Sec::Item(row.id, n));
            // its rows and its See all row
            let body = if self.page.list { (listed + usize::from(listed > 0)).max(1) as f32 * item_list::ROW_H } else { CARD_H };
            if sn_ui::on_axis(frame.y, HEAD_TO_ROW + body, SCR_H, 0.0) {
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
            fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(item, msg::app_nav_home())))));
        }
    }

    /// A held OK on a card: its item menu, anchored beside the card.
    fn menu(&self, cx: &Cx<'_, Bundle>, elem: u32, fx: &mut Effects<'_, Bundle>) {
        let kind = match self.continue_item(cx, elem) {
            Some(i) => item_cards::item_menu(i, msg::app_nav_home()),
            None => {
                let Some(item) = self.item(cx, elem) else { return };
                crate::cards::item_menu(item, msg::app_nav_home())
            }
        };
        // a card's drawn rect, or a list row's
        let anchor = self.stack.focused_card(&self.page, cx, At::Drawn).map(|(_, at)| at.rect);
        let anchor = anchor.or_else(|| self.stack.focused_plain(&self.page, cx).map(|(_, r)| r)).unwrap_or_else(crate::menu::fallback_anchor);
        let focus = cx.focus.current.filter(|k| k.entry == self.page.entry);
        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
            host: self.page.entry,
            reply: fx.from(),
            focus,
            anchor: MenuArg::anchor_of(anchor),
            kind,
        }))));
    }

    /// The group the Home pill seats: the first row with cards, else the failed install list's
    /// Retry.
    fn first_group(&self, cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        if !continue_items(cx).is_empty() {
            return Some(if self.page.list { cw_item_group(0) } else { CW_GROUP });
        }
        match first_seat(cx.views.catalog, self.page.types.kind(), self.page.list)? {
            sn_ui::screen::FocusTarget::FirstInGroup(g) => Some(g),
            _ => None,
        }
    }

    /// The type dropdown follows the rows' types (a store that changed, an install list replaced).
    fn sync_types(&mut self, cx: &Cx<'_, Bundle>) -> bool {
        self.page.types.sync(type_filter::chips(cx.views.catalog.rows().iter().map(Row::kind)))
    }

    /// OK or a click on the type dropdown: its popover, under it.
    fn open_types(&self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let rect = match self.stack.focused_plain(&self.page, cx) {
            Some((Sec::Types, r)) => r,
            _ => self.page.type_rect(cx.measure, CONTENT_TOP - self.stack.scroll()),
        };
        dropdown::open(self.page.entry, cx, fx, rect, self.page.types.menu(msg::app_filter_type(), ChoiceTarget::HomeType));
    }

    /// A pick from the type popover: the rows are narrowed to its type (nothing is fetched; the
    /// next tick reports the new window).
    fn choose(&mut self, n: usize, fx: &mut Effects<'_, Bundle>) {
        if self.page.types.choose(n) {
            self.stack.scroll_to(0.0);
        }
        fx.invalidate(Provenance::Input);
    }
}

impl LogicalState for HomeScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.page.entry.0).u32(self.page.cards.cards() as u32);
        c.option(self.view.clone(), |c, v| {
            c.u32(v.rows.start as u32).u32(v.rows.end as u32).str(v.kind.as_deref().unwrap_or(""));
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
                self.page.cards.sync(cx.views.catalog) | self.sync_continue(cx) | self.sync_types(cx)
            }
            _ => false,
        };
        let grew = grew | self.sync_list(cx);
        let got = self.stack.on(&self.page, ev, cx, fx);
        match ev {
            ScreenEvent::Tick(_) => {
                let view = RowWindow { rows: self.rows_in_view(cx), kind: self.page.types.kind().map(str::to_string) };
                if self.view.as_ref() != Some(&view) {
                    self.view = Some(view.clone());
                    fx.push(Fx::App(AppFx::Catalog(CatalogCmd::View(view))));
                }
                // Home mounts before anything has landed, with the remote resting on its pill: that
                // is a seat nobody chose, so the first row that lands (Continue Watching first)
                // takes it
                let target = self
                    .first_group(cx)
                    .map(sn_ui::screen::FocusTarget::FirstInGroup)
                    .or_else(|| first_seat(cx.views.catalog, self.page.types.kind(), self.page.list));
                let seated = crate::tabs::rests(self.page.entry, crate::tabs::Tab::Home, cx, self.chose, target.is_some());
                let target = target.or(Some(sn_ui::screen::FocusTarget::Elem(sn_machine::machine::FocusKey {
                    entry: self.page.entry,
                    elem: crate::tabs::Tab::Home.elem(),
                })));
                self.seat.tick(seated, target, fx);
                self.first_row = if self.page.bar(cx) { Some(TYPES_GROUP) } else { self.first_group(cx) };
                self.down = self.row_links(cx);
                if grew {
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _)
                if [StoreId::Catalog.ord(), StoreId::Library.ord(), StoreId::Account.ord(), StoreId::Settings.ord()].contains(ord) =>
            {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Activate(elem))) = got {
                    // a card, or a row's See all tile
                    self.activate(cx, elem, fx);
                } else if let Some(k) = cx.focus.current.filter(|k| k.entry == self.page.entry && crate::tabs::on_strip(k.elem)) {
                    crate::tabs::on_pill(k.elem, crate::tabs::Tab::Home, self.first_group(cx), cx, fx);
                } else if cx.focus.current.is_some_and(|k| k.entry == self.page.entry && self.page.types.is(k.elem)) {
                    self.open_types(cx, fx);
                } else if let Some(k) = cx.focus.current.filter(|k| k.entry == self.page.entry) {
                    // a poster card's press is the Stack's (above); a list row's is not reported
                    let card = self.page.cards.at.contains_key(&k.elem) || self.page.cw.contains(k.elem);
                    if self.page.list || !card {
                        self.activate(cx, k.elem, fx);
                    }
                }
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                // a card reports its hold; a list row is the same item, held through its plain stop
                let held = match got {
                    Some(StackEvent::Card(_, CardEvent::Hold(elem))) => Some(elem),
                    _ => self.stack.focused_plain(&self.page, cx).and_then(|(k, _)| match k {
                        Sec::Item(..) | Sec::ContinueItem(_) => self.page.list_elems.get(&k).copied(),
                        _ => None,
                    }),
                };
                if let Some(elem) = held {
                    self.menu(cx, elem, fx);
                }
                Handled::Yes
            }
            // YELLOW on a row (`docs/remote-keys.md`), and LEFT past a shelf's first card (its left
            // edge re-delivers the key): the row's See all. Only a fresh press: a held LEFT walking
            // back along a row stops at its first card.
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Other, wcode: crate::tabs::WCODE_YELLOW, edge, .. }, .. })
            | ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Left, edge, at_edge: true, .. }, .. }) => {
                if *edge == Edge::Down {
                    self.see_all(cx, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => {
                // BACK to the bar is the viewer's seat: no landing takes it back
                self.chose = true;
                crate::tabs::on_back(self.page.entry, crate::tabs::Tab::Home, cx, fx)
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, crate::tabs::Tab::Home, self.first_group(cx), cx, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if self.page.types.is(*elem) => {
                self.open_types(cx, fx);
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::Chose(ChoiceTarget::HomeType, n)) => {
                self.choose(*n, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if *elem == RETRY_ELEM || *elem == BAND_RETRY_ELEM => {
                fx.push(Fx::App(AppFx::Catalog(CatalogCmd::RetryAddons)));
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                // a card, a list row, a heading or the View toggle activated without a press (the
                // pointer's direct activation)
                self.activate(cx, *elem, fx);
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
        out.extend(self.down.iter().copied());
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
