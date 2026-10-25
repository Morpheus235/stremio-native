//! **Discover** — Stremio's Discover as a top-bar destination: the addons' catalogs by type, then
//! by catalog, then by genre, as one paged grid.
//!
//! Under the bar stands ONE row of dropdowns (`crate::dropdown`, Stremio's "Movies ▾ Popular ▾
//! Genre ▾") and the grid:
//!
//! 1. **the type**, one option per type a browsable catalog serves ([`sub_tabs`]): Movies, Series,
//!    Anime, Live TV (`tv` and `channel` together, [`LIVE`]), then the rest alphabetically;
//! 2. **the catalog** of that type, in install then manifest order, each named by the catalog,
//!    or "catalog · addon" where two share a name ([`catalog_labels`]);
//! 3. **the genre**, only when the chosen catalog declares `genre` options: *All* first unless
//!    the genre is required (a required genre starts on its first option);
//!
//! and at the row's right end the *View* toggle (`crate::item_list`, the catalog grid's setting
//! `ViewPage::Catalog`).
//!
//! The derivation follows stremio-core's `catalog_with_filters`: a catalog is selectable when all
//! its required extras can be met with nothing typed (`Catalog::is_browsable`; a search-only
//! catalog is not), the types are the selectable catalogs' types, and the extras offered are the
//! chosen catalog's `genre` options. The grid is the grid store's view of the selection, paged by
//! `skip` as it reaches its end (`sn_stremio::catalog::Pager` builds and encodes the URL).
//!
//! LEFT/RIGHT walk the row, OK (or a click) opens a dropdown's popover under it, a pick applies and
//! closes it, BACK closes it unchanged; DOWN reaches the grid, UP the top bar. BACK from the grid
//! returns to the type dropdown, from the row to the bar. The
//! choices are remembered for the session: the sub-tab through `type_filter::Page::Discover`, the
//! catalog per sub-tab and the genre per catalog here. The views the page opened stay held while
//! it lives (up to [`HELD_VIEWS`]), so switching back to one already loaded asks nothing.
//!
//! Live TV is this page on its Live TV sub-tab (`AppArg::LiveTv`), not a page of its own.
//!
//! A Detail page's genre link pushes this page over it (`AppArg::DiscoverAt`, [`DiscoverScreen::at`])
//! on the link's type, its catalog and its genre ([`genre_catalog`]), chosen as a pick would
//! choose them (so the session remembers them); BACK from anywhere on it returns to the Detail page.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputEvent, InputKind, Key, Leave, LogicalState,
    Machine, NavOp,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::manifest::{Addon, Catalog};
use sn_stremio::types::MetaPreview;
use sn_ui::cards::{CardEvent, GridSpec, Kind, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{SCR_H, SCR_W};
use sn_ui::screen::{
    At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, FocusTarget, Focusable, GroupKind, GroupSpec, Link,
    RenderStrategy, Screen, ScreenEvent, Seat,
};
use sn_ui::widgets::{StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect};

use crate::cards::{item_ref, ItemCards};
use crate::item_list::{self, ListRow};
use crate::dropdown::{self, Dropdown};
use crate::registry::{word, AppArg, AppFx, AppMsg, Bundle, ChoiceTarget, FirstSeat, GenreRef, GridCmd, MenuArg, SettingsCmd};
use crate::stores::catalog::AddonsState;
use crate::stores::grid::{GridKey, GridState};
use crate::stores::settings::ViewPage;
use crate::stores::StoreId;
use crate::tabs::Tab;
use crate::type_filter::{self, ALL};

/// The Live TV sub-tab's key: the `tv` and the `channel` catalogs together.
pub const LIVE: &str = "tv";
/// How many grid views the page keeps held, most recent first; an older one is let go.
// ponytail: a fixed count of views (a few pages of previews each); a byte budget if one grows large
pub const HELD_VIEWS: usize = 8;

const RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 1;
/// The *Try again* after the last item when the next page failed.
pub const MORE_RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 3;
const MORE_GROUP: GroupId = GroupId(5);
/// The View toggle at the row's right end.
pub const TOGGLE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 2;
/// The type, catalog and genre dropdowns.
pub const TYPE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x1000;
pub const CATALOG_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x2000;
pub const GENRE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x3000;
const CARD_BASE: u32 = 0x1000;
/// Elements per grid view: a new view takes the next band, so a card key never names two items.
const VIEW_BAND: u32 = 0x0010_0000;
const ITEMS_GROUP: GroupId = GroupId(0);
const STATUS_GROUP: GroupId = GroupId(2);
const TOP_GROUP: GroupId = GroupId(3);
const BAR_GROUP: GroupId = GroupId(4);
fn list_group(i: usize) -> GroupId {
    GroupId(0x10_0000 + i as u32)
}
/// The dropdown row starts under the top bar (the Library's placement).
pub const CONTENT_TOP: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;

/// The sub-tab a catalog of `kind` belongs to.
pub fn tab_of(kind: &str) -> &str {
    if kind == "channel" {
        LIVE
    } else {
        kind
    }
}

/// A sub-tab's label: Live TV, or the type's ([`type_filter::label`]).
pub fn sub_tab_label(tab: &str) -> String {
    if tab == LIVE {
        msg::app_tab_live_tv().to_string()
    } else {
        type_filter::label(tab)
    }
}

/// One browsable addon catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoverCatalog {
    pub addon_id: String,
    pub addon_name: String,
    pub catalog: Catalog,
}

impl DiscoverCatalog {
    /// Its identity among the page's catalogs (the catalog dropdown's key, the memory's key).
    pub fn ident(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{}", self.addon_id, self.catalog.kind, self.catalog.id)
    }

    /// Must a genre be named? Then there is no *All*.
    pub fn genre_required(&self) -> bool {
        self.catalog.extra("genre").is_some_and(|g| g.is_required)
    }

    /// The genre dropdown's options `(key, label)`: *All* ([`ALL`]) first unless a genre is
    /// required, then the catalog's options. Empty: the catalog offers no genres.
    pub fn genre_chips(&self) -> Vec<(String, String)> {
        let genres = self.catalog.genres();
        if genres.is_empty() {
            return Vec::new();
        }
        let all = (!self.genre_required()).then(|| (ALL.to_string(), msg::app_filter_all().to_string()));
        all.into_iter().chain(genres.into_iter().map(|g| (g.clone(), g))).collect()
    }

    /// The grid this catalog shows under `genre` (none: a required genre's first option).
    pub fn key(&self, genre: Option<&str>) -> GridKey {
        let genre = match genre {
            Some(g) => Some(g.to_string()),
            None => self.catalog.default_genre(),
        };
        GridKey { addon_id: self.addon_id.clone(), kind: self.catalog.kind.clone(), catalog_id: self.catalog.id.clone(), genre, search: None }
    }
}

/// Every browsable catalog of `addons` (stremio-core's selectable catalogs), in install then
/// manifest order.
pub fn catalogs(addons: &[Addon]) -> Vec<DiscoverCatalog> {
    addons
        .iter()
        .flat_map(|a| {
            let name = if a.manifest.name.is_empty() { a.manifest.id.clone() } else { a.manifest.name.clone() };
            a.manifest
                .catalogs
                .iter()
                .filter(|c| !c.kind.is_empty() && c.is_browsable())
                .map(move |c| DiscoverCatalog { addon_id: a.manifest.id.clone(), addon_name: name.clone(), catalog: c.clone() })
        })
        .collect()
}

/// The sub-tabs the catalogs make: Movies, Series, Anime, Live TV, then the rest alphabetically by
/// label; each once.
pub fn sub_tabs(catalogs: &[DiscoverCatalog]) -> Vec<String> {
    let rank = |t: &str| match t {
        "movie" => 0,
        "series" => 1,
        "anime" => 2,
        LIVE => 3,
        _ => 4,
    };
    let mut v: Vec<(u8, String, String)> = catalogs
        .iter()
        .map(|c| tab_of(&c.catalog.kind))
        .map(|t| (rank(t), sub_tab_label(t).to_lowercase(), t.to_string()))
        .collect();
    v.sort();
    v.dedup();
    v.into_iter().map(|(_, _, t)| t).collect()
}

/// The catalogs of sub-tab `tab`, in order.
pub fn in_tab<'a>(catalogs: &'a [DiscoverCatalog], tab: &str) -> Vec<&'a DiscoverCatalog> {
    catalogs.iter().filter(|c| tab_of(&c.catalog.kind) == tab).collect()
}

/// The catalog options' labels: the catalog's name, or "name · addon" where another catalog of
/// the type has the same name. Server text.
pub fn catalog_labels(row: &[&DiscoverCatalog]) -> Vec<String> {
    row.iter()
        .map(|c| {
            let name = c.catalog.display_name();
            if row.iter().filter(|o| o.catalog.display_name() == name).count() > 1 {
                format!("{name} \u{b7} {}", c.addon_name)
            } else {
                name.to_string()
            }
        })
        .collect()
}

/// The catalog a genre link opens: the one the link names (its addon by base URL, its type and
/// id) when it offers the genre, else the first catalog of the type that does, else the first of
/// its sub-tab that does (Live TV's `tv` and `channel`). `None`: none offers it.
pub fn genre_catalog<'a>(catalogs: &'a [DiscoverCatalog], addons: &[Addon], to: &GenreRef) -> Option<&'a DiscoverCatalog> {
    let named = |c: &&DiscoverCatalog| {
        let base = to.addon_base.as_deref().map(sn_stremio::manifest::base_of);
        let addon = base.is_none_or(|b| addons.iter().any(|a| a.base == b && a.manifest.id == c.addon_id));
        addon && c.catalog.kind == to.kind && to.catalog.as_deref().is_none_or(|id| c.catalog.id == id)
    };
    let offers = |c: &&DiscoverCatalog| to.genre.as_ref().is_none_or(|g| c.catalog.genres().contains(g));
    let row = in_tab(catalogs, tab_of(&to.kind));
    let first = |f: &dyn Fn(&&DiscoverCatalog) -> bool| row.iter().copied().find(|c| f(c) && offers(c));
    first(&named).or_else(|| first(&|c| c.catalog.kind == to.kind)).or_else(|| first(&|_| true))
}

/// The session's choices beyond the sub-tab: the catalog per sub-tab, the genre per catalog.
#[derive(Default)]
struct Memory {
    catalog: HashMap<String, String>,
    genre: HashMap<String, String>,
}

// ponytail: the frame thread's session memory, as `type_filter`'s; a restart forgets it
thread_local! {
    static MEMORY: RefCell<Memory> = RefCell::new(Memory::default());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    /// The band under the top bar.
    Top,
    /// The dropdown row and the View toggle.
    Bar,
    Status,
    /// The grid of one view (its band ordinal).
    Items(u32),
    /// List mode: item `i`.
    Row(usize),
    /// After the last item: the next page failed, *Try again* (`crate::grid::more_failed_rect`).
    MoreFailed,
}

struct Page {
    entry: EntryId,
    catalogs: Vec<DiscoverCatalog>,
    types: Dropdown,
    row: Dropdown,
    genres: Dropdown,
    key: Option<GridKey>,
    view: u32,
    /// The items show as a list (the setting, as of the last event).
    list: bool,
}

impl Page {
    fn tab(&self) -> Option<&str> {
        self.types.kind()
    }

    fn current(&self) -> Option<&DiscoverCatalog> {
        let ident = self.row.kind()?;
        self.catalogs.iter().find(|c| c.ident() == ident)
    }

    /// Re-derive the rows from the catalogs and the remembered choices.
    fn arrange(&mut self) {
        let tabs = sub_tabs(&self.catalogs);
        self.types.sync_named(tabs.iter().map(|t| (t.clone(), sub_tab_label(t))).collect());
        let tab = self.tab().unwrap_or_default().to_string();
        let row = in_tab(&self.catalogs, &tab);
        let chips = row.iter().map(|c| c.ident()).zip(catalog_labels(&row)).collect();
        let chosen = MEMORY.with(|m| m.borrow().catalog.get(&tab).cloned()).unwrap_or_default();
        self.row.set_chosen(&chosen);
        self.row.sync_named(chips);
        let (ident, genres) = self.current().map(|c| (c.ident(), c.genre_chips())).unwrap_or_default();
        let genre = MEMORY.with(|m| m.borrow().genre.get(&ident).cloned()).unwrap_or_default();
        self.genres.set_chosen(&genre);
        self.genres.sync_named(genres);
    }

    /// The dropdowns standing, left to right.
    fn buttons(&self) -> Vec<&Dropdown> {
        [&self.types, &self.row, &self.genres].into_iter().filter(|d| d.shown()).collect()
    }

    /// The row's elements: the dropdowns, then the View toggle. Empty: no row.
    fn bar(&self) -> Vec<u32> {
        let buttons = self.buttons();
        if buttons.is_empty() {
            return Vec::new();
        }
        buttons.iter().map(|d| d.elem()).chain([TOGGLE_ELEM]).collect()
    }

    /// The row's rects at `y`, as [`bar`](Self::bar).
    fn bar_rects(&self, measure: &dyn sn_machine::machine::Measure, y: f32) -> Vec<Rect> {
        let widths: Vec<f32> = self.buttons().iter().map(|d| d.width(measure)).collect();
        dropdown::row(&widths, Some(item_list::toggle_width(measure)), y)
    }

    fn dropdown(&self, target: ChoiceTarget) -> Option<&Dropdown> {
        match target {
            ChoiceTarget::DiscoverType => Some(&self.types),
            ChoiceTarget::DiscoverCatalog => Some(&self.row),
            ChoiceTarget::DiscoverGenre => Some(&self.genres),
            _ => None,
        }
    }

    /// The dropdown `elem` is, with its popover's target and title.
    fn button_of(&self, elem: u32) -> Option<(&Dropdown, ChoiceTarget, &'static str)> {
        [
            (&self.types, ChoiceTarget::DiscoverType, msg::app_filter_type()),
            (&self.row, ChoiceTarget::DiscoverCatalog, msg::app_discover_catalog()),
            (&self.genres, ChoiceTarget::DiscoverGenre, msg::app_discover_genre()),
        ]
        .into_iter()
        .find(|(d, _, _)| d.is(elem))
    }

    /// Where the grid's read-out stands: under the row.
    fn status_frame(&self) -> Rect {
        let top = if self.catalogs.is_empty() { sn_ui::widgets::TOP_BAR_BOTTOM } else { CONTENT_TOP + if self.bar().is_empty() { 0.0 } else { dropdown::BAND } };
        Rect::new(0.0, top, SCR_W, SCR_H - top)
    }

    fn state(&self, cx: &Cx<'_, Bundle>) -> Option<GridState> {
        if self.catalogs.is_empty() {
            return None;
        }
        Some(self.key.as_ref().and_then(|k| cx.views.grid.state(k)).unwrap_or(GridState::Loading))
    }

    fn readout(&self, cx: &Cx<'_, Bundle>) -> Option<StatusOverlay<'static>> {
        if self.catalogs.is_empty() {
            // no catalog yet is Home's wait or Home's failure while the install list loads or failed
            let frame = self.status_frame();
            return Some(match cx.views.catalog.addons_state() {
                AddonsState::Loading => StatusOverlay::new(frame, msg::app_home_loading_addons_c(), StatusKind::Working),
                AddonsState::Failed => StatusOverlay::new(frame, msg::app_home_addons_failed_c(), StatusKind::Failed)
                    .reason(msg::app_home_addons_failed_reason_c())
                    .action(msg::app_home_retry_c()),
                AddonsState::Ready => StatusOverlay::new(frame, msg::app_discover_none_c(), StatusKind::Empty).reason(msg::app_discover_none_reason_c()),
            });
        }
        crate::grid::readout(&self.state(cx)?, self.status_frame())
    }

    fn items<'a>(&self, cx: &Cx<'a, Bundle>) -> &'a [MetaPreview] {
        self.key.as_ref().map_or(&[], |k| cx.views.grid.items(k))
    }

    fn base(&self) -> u32 {
        CARD_BASE + (self.view % 64) * VIEW_BAND
    }

    /// The read-out's Retry stands (the grid's first page, or the install list, failed).
    fn retry_shown(&self, cx: &Cx<'_, Bundle>) -> bool {
        if self.catalogs.is_empty() {
            return cx.views.catalog.addons_state() == AddonsState::Failed;
        }
        matches!(self.state(cx), Some(GridState::Failed(_)))
    }

    /// Where BACK from the grid goes: the type dropdown.
    fn back_seat(&self) -> Option<u32> {
        self.bar().first().copied()
    }
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(cx.views.grid.gen()) << 32)
            ^ (u64::from(self.view) << 16)
            ^ (self.catalogs.len() as u64)
            ^ (self.types.rev() << 40)
            ^ (self.row.rev() << 20)
            ^ (self.genres.rev() << 8)
            ^ (u64::from(self.list) << 63)
            ^ ((self.items(cx).len() as u64) << 48)
            ^ (u64::from(cx.views.catalog.gen()) << 56)
            ^ (u64::from(cx.views.epg.gen()) << 24)
    }
    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        out.push(SectionSpec::new(Sec::Top, Kind::Custom { height: CONTENT_TOP, focusable: false }, TOP_GROUP));
        if !self.bar().is_empty() {
            out.push(SectionSpec::new(Sec::Bar, Kind::Custom { height: dropdown::BAND, focusable: true }, BAR_GROUP).ranked(1));
        }
        if let Some(o) = self.readout(cx) {
            let failed = self.retry_shown(cx);
            let rect = if failed { o.action_frame_measured(cx.measure).unwrap_or(Rect::FULL) } else { Rect::FULL };
            out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: failed }, STATUS_GROUP).ranked(2));
        }
        if self.catalogs.is_empty() {
            return;
        }
        if self.list {
            for i in 0..self.items(cx).len() {
                out.push(SectionSpec::new(Sec::Row(i), Kind::Custom { height: item_list::ROW_H, focusable: true }, list_group(i)));
            }
        } else {
            out.push(SectionSpec::new(Sec::Items(self.view), Kind::Grid { spec: GridSpec::new(0.0, CONTENT_TOP) }, ITEMS_GROUP));
        }
        if self.state(cx) == Some(GridState::MoreFailed) {
            out.push(SectionSpec::new(Sec::MoreFailed, Kind::Custom { height: crate::grid::MORE_FAILED_H, focusable: true }, MORE_GROUP));
        }
    }
    fn fallback(&self, _cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        out.extend([Sec::Items(self.view), Sec::Row(0), Sec::MoreFailed, Sec::Bar, Sec::Status]);
    }
    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        match k {
            Sec::Items(v) if v == self.view => Some(ItemCards {
                items: self.items(cx),
                base: self.base(),
                more: matches!(self.state(cx), Some(GridState::Ready { more: true })),
                watched: Some(cx.views.library),
                guide: Some(cx.views.epg),
            }),
            _ => None,
        }
    }
    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Status => Some(RETRY_ELEM),
            Sec::MoreFailed => Some(MORE_RETRY_ELEM),
            Sec::Row(i) => Some(self.base() + i as u32),
            _ => None,
        }
    }
    fn plain_len(&self, k: Sec) -> usize {
        match k {
            Sec::Bar => self.bar().len(),
            _ => self.elem_of(k).is_some() as usize,
        }
    }
    fn plain_elem(&self, k: Sec, n: usize) -> Option<u32> {
        match k {
            Sec::Bar => self.bar().get(n).copied(),
            _ if n == 0 => self.elem_of(k),
            _ => None,
        }
    }
    fn plain_step(&self, k: Sec, at: usize, dir: Dir) -> Option<usize> {
        match (k, dir) {
            (Sec::Bar, Dir::Left) => at.checked_sub(1),
            (Sec::Bar, Dir::Right) => (at + 1 < self.bar().len()).then_some(at + 1),
            _ => None,
        }
    }
    fn element_rect(&self, cx: &Cx<'_, Bundle>, k: Sec, n: usize, section: Rect) -> Rect {
        match k {
            Sec::Bar => self.bar_rects(cx.measure, section.y).get(n).copied().unwrap_or(section),
            Sec::Row(_) => item_list::face_rect(section),
            Sec::MoreFailed => crate::grid::more_failed_rect(section, cx.measure),
            _ => section,
        }
    }
    fn plain_group(&self, _cx: &Cx<'_, Bundle>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        if k == Sec::Bar {
            return dropdown::group(id, extent, self.bar().len());
        }
        if let Sec::Row(_) = k {
            return item_list::group(id, extent);
        }
        GroupSpec { id, kind: GroupKind::Free, seat: Seat::First, reachable: AxisMask::BOTH, edge: [EdgeRule::Stop; 4], extent, len: 1, elem: ElemKind::Bare }
    }
    /// The row scrolled back into view stops under the top bar, not under its pills.
    fn reveal_margin(&self, _k: Sec) -> f32 {
        CONTENT_TOP
    }
    fn reveal_under_margin(&self, k: Sec) -> bool {
        matches!(k, Sec::Bar | Sec::Row(_))
    }
    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        let p = f.painter.alpha(f.page_alpha);
        match k {
            Sec::Bar => {
                let rects = self.bar_rects(f.measure, r.y);
                for (d, rect) in self.buttons().into_iter().zip(&rects) {
                    d.draw(f, *rect);
                }
                if let Some(toggle) = rects.last() {
                    let focused = f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == TOGGLE_ELEM);
                    item_list::draw_toggle(f.painter.alpha(f.page_alpha), *toggle, self.list, focused);
                }
            }
            Sec::Row(i) => {
                let Some(item) = self.items(f.cx).get(i) else { return };
                let focused = f.focus.current.is_some_and(|key| key.elem == self.base() + i as u32);
                item_list::draw(f, r, &ListRow::of_meta(item, None).on_air(crate::cards::on_air(item, Some(f.cx.views.epg))), focused);
            }
            Sec::Status => {
                if let Some(o) = self.readout(f.cx) {
                    let focused = f.focus.current.is_some_and(|key| key.elem == RETRY_ELEM);
                    o.phase(f.cx.tick.ms).focused(focused).draw_measured(&Env::inert(), p, f.measure);
                }
            }
            Sec::MoreFailed => {
                let focused = f.focus.current.is_some_and(|key| key.elem == MORE_RETRY_ELEM);
                crate::grid::draw_more_failed(p, crate::grid::more_failed_rect(r, f.measure), focused);
            }
            Sec::Top | Sec::Items(_) => {}
        }
    }
}

pub struct DiscoverScreen {
    page: Page,
    stack: Stack<Sec>,
    /// The catalog store generation the catalogs were read at.
    read: Option<u32>,
    /// The page mounts before its catalogs are read: the chosen sub-tab takes the seat, and with
    /// none the remote rests on the page's pill.
    seat: FirstSeat,
    /// The viewer has moved focus themselves.
    chose: bool,
    /// The views this page holds open, least recently shown first.
    held: Vec<GridKey>,
    /// The group under the dropdown row (the grid, the first list row or the read-out's Retry).
    below: GroupId,
    /// How many items the list had when it last asked for the next page (one ask per length).
    asked_at: Option<usize>,
    /// A genre link's selection, applied once the catalogs are read.
    link: Option<GenreRef>,
    /// Pushed by a link: BACK pops back to the page that pushed it.
    pushed: bool,
}

impl DiscoverScreen {
    /// Discover, on sub-tab `tab` when one is named (Live TV opens it on [`LIVE`]), else on the
    /// session's last.
    pub fn new(entry: EntryId, tab: Option<&str>) -> Self {
        if let Some(tab) = tab {
            type_filter::remember(type_filter::Page::Discover, tab);
        }
        DiscoverScreen {
            page: Page {
                entry,
                catalogs: Vec::new(),
                types: Dropdown::new(type_filter::Page::Discover, entry, TYPE_ELEM),
                row: Dropdown::row(entry, CATALOG_ELEM),
                genres: Dropdown::row(entry, GENRE_ELEM).all_label(msg::app_discover_all_genres),
                key: None,
                view: 0,
                list: false,
            },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Library),
            read: None,
            seat: FirstSeat::new(),
            chose: false,
            held: Vec::new(),
            below: ITEMS_GROUP,
            asked_at: None,
            link: None,
            pushed: false,
        }
    }

    /// Discover pushed by a genre link: on `to`'s sub-tab, catalog and genre ([`genre_catalog`]).
    pub fn at(entry: EntryId, to: GenreRef) -> Self {
        let mut page = Self::new(entry, Some(tab_of(&to.kind)));
        page.link = Some(to);
        page.pushed = true;
        page
    }

    /// Choose the link's catalog and genre, as the dropdowns would (remembered for the session).
    fn apply_link(&mut self, addons: &[Addon]) {
        if self.page.catalogs.is_empty() {
            return;
        }
        let Some(to) = self.link.take() else { return };
        let Some(c) = genre_catalog(&self.page.catalogs, addons, &to) else { return };
        let (tab, ident) = (tab_of(&c.catalog.kind).to_string(), c.ident());
        type_filter::remember(type_filter::Page::Discover, &tab);
        MEMORY.with(|m| {
            let mut m = m.borrow_mut();
            m.catalog.insert(tab, ident.clone());
            if let Some(g) = &to.genre {
                m.genre.insert(ident, g.clone());
            }
        });
    }

    /// The grid the page shows, as the tests read it.
    pub fn key(&self) -> Option<&GridKey> {
        self.page.key.as_ref()
    }

    pub fn catalogs(&self) -> &[DiscoverCatalog] {
        &self.page.catalogs
    }

    /// The sub-tab shown.
    pub fn tab(&self) -> Option<&str> {
        self.page.tab()
    }

    /// The row's elements (the dropdowns shown, then the View toggle), for the tests.
    pub fn bar(&self) -> Vec<u32> {
        self.page.bar()
    }

    /// What dropdown `target` says, for the tests.
    pub fn value(&self, target: ChoiceTarget) -> Option<String> {
        self.page.dropdown(target).filter(|d| d.shown()).map(Dropdown::value)
    }

    /// The page's scroll (the tests read where a row stands).
    #[cfg(test)]
    pub fn scroll(&self) -> f32 {
        self.stack.scroll()
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    /// Re-read the catalogs when the install moved; the choices are kept by identity.
    fn sync(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        if self.read == Some(cx.views.catalog.gen()) {
            return;
        }
        self.read = Some(cx.views.catalog.gen());
        self.page.catalogs = catalogs(cx.views.addons());
        self.apply_link(cx.views.addons());
        self.page.arrange();
        self.show(fx);
    }

    /// Point the grid at the selection: a view this page already holds is shown as it stands, a
    /// new one is opened (and the least recently shown let go past [`HELD_VIEWS`]).
    fn show(&mut self, fx: &mut Effects<'_, Bundle>) {
        let key = self.page.current().map(|c| c.key(self.page.genres.kind()));
        if key == self.page.key {
            return;
        }
        if let Some(k) = &key {
            match self.held.iter().position(|h| h == k) {
                Some(i) => {
                    let k = self.held.remove(i);
                    self.held.push(k);
                }
                None => {
                    fx.push(Fx::App(AppFx::Grid(GridCmd::Open(k.clone()))));
                    self.held.push(k.clone());
                    if self.held.len() > HELD_VIEWS {
                        fx.push(Fx::App(AppFx::Grid(GridCmd::Close(self.held.remove(0)))));
                    }
                }
            }
        }
        self.page.key = key;
        self.page.view = self.page.view.wrapping_add(1);
        self.asked_at = None;
        fx.invalidate(Provenance::Landing(fx.from()));
    }

    /// Apply option `n` of dropdown `target` (a pick from its popover).
    fn choose(&mut self, target: ChoiceTarget, n: usize, fx: &mut Effects<'_, Bundle>) {
        match target {
            ChoiceTarget::DiscoverType => {
                self.page.types.choose(n);
            }
            ChoiceTarget::DiscoverCatalog => {
                self.page.row.choose(n);
                let tab = self.page.tab().unwrap_or_default().to_string();
                let ident = self.page.row.kind().unwrap_or_default().to_string();
                MEMORY.with(|m| m.borrow_mut().catalog.insert(tab, ident));
            }
            ChoiceTarget::DiscoverGenre => {
                self.page.genres.choose(n);
                let ident = self.page.current().map(DiscoverCatalog::ident).unwrap_or_default();
                let genre = self.page.genres.kind().unwrap_or(ALL).to_string();
                MEMORY.with(|m| m.borrow_mut().genre.insert(ident, genre));
            }
            _ => return,
        }
        self.page.arrange();
        self.show(fx);
        fx.invalidate(Provenance::Input);
    }

    /// OK or a click on a dropdown: its popover, under the button. `false`: not a dropdown.
    fn open(&self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> bool {
        let Some((d, target, title)) = self.page.button_of(elem) else { return false };
        let rect = match self.stack.focused_plain(&self.page, cx) {
            Some((_, r)) if cx.focus.current.is_some_and(|k| k.elem == elem) => r,
            _ => {
                let n = self.page.bar().iter().position(|e| *e == elem).unwrap_or(0);
                self.page.bar_rects(cx.measure, CONTENT_TOP - self.stack.scroll())[n]
            }
        };
        dropdown::open(self.page.entry, cx, fx, rect, d.menu(title, target));
        true
    }

    fn item_at(&self, elem: u32, cx: &Cx<'_, Bundle>) -> Option<MetaPreview> {
        ItemCards { items: self.page.items(cx), base: self.page.base(), more: false, watched: None, guide: None }.item(elem).cloned()
    }

    fn press(&mut self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let first = (!self.page.bar().is_empty()).then_some(BAR_GROUP);
        if crate::tabs::on_pill(elem, Tab::Discover, first, cx, fx) || self.open(elem, cx, fx) {
            return;
        }
        match elem {
            TOGGLE_ELEM => fx.push(Fx::App(AppFx::Settings(SettingsCmd::List(ViewPage::Catalog, !self.page.list)))),
            RETRY_ELEM if self.page.catalogs.is_empty() => fx.push(Fx::App(AppFx::Catalog(crate::registry::CatalogCmd::RetryAddons))),
            RETRY_ELEM | MORE_RETRY_ELEM => {
                if let Some(k) = &self.page.key {
                    fx.push(Fx::App(AppFx::Grid(GridCmd::Retry(k.clone()))));
                }
            }
            _ => {
                if let Some(item) = self.item_at(elem, cx) {
                    fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(&item, Tab::Discover.label())))));
                }
            }
        }
    }

    /// BACK: from the grid (or its read-out) to the row, from the row to the bar; pushed by a
    /// link, straight back to the page that pushed it.
    fn back(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        if self.pushed {
            return Handled::No;
        }
        self.chose = true;
        let entry = self.page.entry;
        let on_content = cx.focus.current.filter(|k| k.entry == entry).is_some_and(|k| {
            !crate::tabs::on_strip(k.elem) && !self.page.bar().contains(&k.elem)
        });
        match self.page.back_seat().filter(|_| on_content) {
            Some(elem) => {
                crate::registry::reseat(FocusTarget::Elem(FocusKey { entry, elem }), fx);
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            None => crate::tabs::on_back(entry, Tab::Discover, cx, fx),
        }
    }

    fn close_all(&mut self, fx: &mut Effects<'_, Bundle>) {
        for k in self.held.drain(..) {
            fx.push(Fx::App(AppFx::Grid(GridCmd::Close(k))));
        }
        self.page.key = None;
    }
}

impl LogicalState for DiscoverScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.page.entry.0).u32(self.page.view).bool(self.page.list);
        for row in [&self.page.types, &self.page.row, &self.page.genres] {
            c.str(row.kind().unwrap_or(ALL));
        }
        self.stack.write(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("discover catalogs={} tab={}", self.page.catalogs.len(), self.page.tab().unwrap_or("-")));
    }
}

impl Machine<Bundle> for DiscoverScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        if matches!(ev, ScreenEvent::Mount | ScreenEvent::Tick(_) | ScreenEvent::StoreChanged(..)) {
            self.sync(cx, fx);
        }
        self.page.list = cx.views.settings.list(ViewPage::Catalog);
        let got = self.stack.on(&self.page, ev, cx, fx);
        let entry = self.page.entry;
        match ev {
            ScreenEvent::Tick(_) => {
                let len = self.page.items(cx).len();
                if let Some(StackEvent::Card(Sec::Items(_), CardEvent::Want(range))) = &got {
                    if range.end > len {
                        if let Some(k) = &self.page.key {
                            fx.push(Fx::App(AppFx::Grid(GridCmd::More(k.clone()))));
                        }
                    }
                }
                let more = matches!(self.page.state(cx), Some(GridState::Ready { more: true }));
                if self.page.list && more && self.asked_at != Some(len) {
                    let at = cx.focus.current.and_then(|k| k.elem.checked_sub(self.page.base())).map(|i| i as usize).filter(|i| *i < len);
                    if item_list::wants_more(at, len) {
                        self.asked_at = Some(len);
                        if let Some(k) = &self.page.key {
                            fx.push(Fx::App(AppFx::Grid(GridCmd::More(k.clone()))));
                        }
                    }
                }
                self.below = match (len > 0, self.page.list) {
                    (true, true) => list_group(0),
                    (true, false) => ITEMS_GROUP,
                    (false, _) => STATUS_GROUP,
                };
                let elem = if self.page.types.shown() { TYPE_ELEM } else { Tab::Discover.elem() };
                let target = FocusTarget::Elem(FocusKey { entry, elem });
                let has_content = !self.page.catalogs.is_empty();
                let seated = crate::tabs::rests(entry, Tab::Discover, cx, self.chose, has_content);
                self.seat.tick(seated, Some(target), fx);
                let addons_loading = self.page.catalogs.is_empty() && cx.views.catalog.addons_state() == AddonsState::Loading;
                if self.page.state(cx) == Some(GridState::Loading) || addons_loading {
                    fx.note(sn_machine::present::PresentEvent::Motion);
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if [StoreId::Grid, StoreId::Catalog, StoreId::Settings, StoreId::Epg].iter().any(|s| s.ord() == *ord) => {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                match got {
                    Some(StackEvent::Card(_, CardEvent::Activate(elem))) => self.press(elem, cx, fx),
                    _ => {
                        if let Some(k) = cx.focus.current.filter(|k| k.entry == entry) {
                            self.press(k.elem, cx, fx);
                        }
                    }
                }
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.press(*elem, cx, fx);
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::Chose(target, n)) => {
                self.choose(*target, *n, fx);
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                // a card reports its hold; a list row is the same item, held through its plain stop
                let held = match got {
                    Some(StackEvent::Card(_, CardEvent::Hold(elem))) => Some((elem, self.stack.focused_card(&self.page, cx, At::Drawn).map(|(_, at)| at.rect))),
                    _ => self.stack.focused_plain(&self.page, cx).and_then(|(k, rect)| match k {
                        Sec::Row(i) => Some((self.page.base() + i as u32, Some(rect))),
                        _ => None,
                    }),
                };
                if let Some((elem, at)) = held {
                    if let Some(item) = self.item_at(elem, cx) {
                        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
                            host: entry,
                            reply: fx.from(),
                            focus: cx.focus.current,
                            anchor: MenuArg::anchor_of(at.unwrap_or_else(crate::menu::fallback_anchor)),
                            kind: crate::cards::item_menu(&item, Tab::Discover.label()),
                        }))));
                    }
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => self.back(cx, fx),
            ScreenEvent::WillLeave(Leave::ForGood) | ScreenEvent::Unmount => {
                self.close_all(fx);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { from, by, .. } => {
                self.chose |= crate::tabs::deliberate(entry, *from, *by);
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

sn_ui::focusable_via_view!(DiscoverScreen, Bundle, view);

impl Screen<Bundle> for DiscoverScreen {
    fn name(&self) -> &'static str {
        word::DISCOVER
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
    }
    fn links(&self, out: &mut Vec<Link>) {
        if self.page.bar().is_empty() {
            crate::tabs::links(self.below, out);
            return;
        }
        crate::tabs::links(BAR_GROUP, out);
        out.push(Link { from: BAR_GROUP, dir: Dir::Down, to: self.below });
        out.push(Link { from: self.below, dir: Dir::Up, to: BAR_GROUP });
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
