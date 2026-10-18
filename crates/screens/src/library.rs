//! **Library** — the signed-in account's library, a shelf per type (movies, series, then the
//! rest, `stores::library::TYPE_ORDER`), in the chosen sort's order (last watched first by
//! default), each poster carrying its
//! resume bar when it is in progress. OK opens the item's Detail page.
//!
//! The page is a `sn_ui::cards::Stack` like Home: a status section (signed out, loading, failed,
//! empty — the shared `StatusOverlay` read-out, at the shared placement) and one shelf per type.
//! Signed out, the read-out's action opens the sign-in page; a failed load's action asks again.
//!
//! **The type dropdown** (`crate::dropdown`, its options `crate::type_filter`'s) stands above the
//! shelves once the library holds more than one type: the types its own items carry.
//! A pick from its popover keeps only that type's shelf (the empty read-out when it has no item);
//! the choice is kept for the session.
//!
//! **The sort dropdown** stands beside it: stremio-core's Library sorts (`sn_stremio::library::Sort`:
//! last watched — the default —, A–Z, Z–A, most watched, watched, not watched), its comparators
//! ordering each shelf. The choice is a per-device setting (`SettingsCmd::LibrarySort`).
//!
//! A held OK on a card (or a list row) opens the item menu (`crate::menu`), as on Home.
//!
//! **The View toggle** (`crate::item_list`) stands at the same row's right end: posters, or each type's items as a
//! list under the type's heading — the same items, the same OK. The choice is a setting
//! (`ViewPage::Library`).
//!
//! It is the top bar's Library destination (`crate::tabs`): no crumb, UP from its first shelf (or
//! the read-out's action) to the bar, BACK to its pill. With nothing to focus (loading, empty) the
//! remote rests on the pill.

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputEvent, InputKind, Key, LogicalState, Machine,
    NavOp,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::library::{LibraryItem, Sort};
use sn_ui::cards::{CardEvent, Kind, RowStyle, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{MARGIN_X, SCR_W};
use sn_ui::screen::{
    At, Dir, DrawFrame, FocusSource, FocusTarget, Focusable, GroupSpec, HitSource, Link, RenderStrategy, Screen, ScreenEvent,
};
use sn_ui::widgets::{StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect};

use crate::item_cards::{self, Interned, ItemCards};
use crate::item_list::{self, ListRow};
use crate::dropdown::{self, Dropdown};
use crate::registry::{kind_label, word, AppArg, AppFx, AppMsg, Bundle, ChoiceTarget, FirstSeat, ItemRef, LibraryCmd, MenuArg, SettingsCmd};
use crate::stores::settings::ViewPage;
use crate::stores::account::SessionState;
use crate::stores::library::{LibraryState, LibraryStore};
use crate::stores::StoreId;
use crate::tabs::Tab;
use crate::type_filter;

const FIRST_CARD_ELEM: u32 = 0x1000;
const ACTION_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 2;
const STATUS_GROUP: GroupId = GroupId(0);
const TYPES_GROUP: GroupId = GroupId(0x7FFF_0001);
pub const TYPE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x100;
pub const TOGGLE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 3;
pub const SORT_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x101;
fn list_group(shelf: usize, i: usize) -> GroupId {
    GroupId(0x0100_0000 + ((shelf as u32 & 0xFF) << 16) + (i as u32 & 0xFFFF))
}
/// The shelves start under the top bar (Home's placement).
const CONTENT_TOP: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
const HEAD_TO_ROW: f32 = 64.0;
const HEADING_TOP: f32 = 10.0;
static POSTER: RowStyle = RowStyle::HOME;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    Status,
    /// The type dropdown and the View toggle.
    Types,
    /// The shelf of the [`shelves`] group at this position.
    Type(usize),
    /// List mode: that group's heading.
    Head(usize),
    /// List mode: item `i` of that group.
    Item(usize, usize),
}

struct Page {
    entry: EntryId,
    cards: Interned,
    types: Dropdown,
    /// The sort dropdown (its options [`Sort::ALL`], keyed by `Sort::name`; the setting keeps the choice).
    sorts: Dropdown,
    /// The items show as lists (the setting, as of the last event).
    list: bool,
    /// List mode: each group's items' elements, as of the last event (`plain_elem` has no `Cx`).
    rows: Vec<Vec<u32>>,
}

impl Page {
    /// The row is shown: there are types to choose, or items to show either way.
    fn bar(&self, cx: &Cx<'_, Bundle>) -> bool {
        self.types.shown() || matches!(cx.views.library.state(), LibraryState::Ready(_)) && !cx.views.library.by_type(Sort::LastWatched).is_empty()
    }

    /// The row's dropdowns that show: the types (when there are types to choose), the sort.
    fn dropdowns(&self) -> impl Iterator<Item = &Dropdown> {
        [&self.types, &self.sorts].into_iter().filter(|d| d.shown())
    }

    /// The row's elements: the dropdowns, then the View toggle.
    fn bar_elems(&self) -> Vec<u32> {
        self.dropdowns().map(Dropdown::elem).chain([TOGGLE_ELEM]).collect()
    }

    /// The row's rects at `y`, as [`bar_elems`](Self::bar_elems).
    fn bar_rects(&self, measure: &dyn sn_machine::machine::Measure, y: f32) -> Vec<Rect> {
        let widths: Vec<f32> = self.dropdowns().map(|d| d.width(measure)).collect();
        dropdown::row(&widths, Some(item_list::toggle_width(measure)), y)
    }

    /// The order the setting names.
    fn sort(cx: &Cx<'_, Bundle>) -> Sort {
        cx.views.settings.library_sort()
    }
}

/// The shelves the page shows: the store's `by_type()` groups of the type chosen (all of them for
/// `None`).
fn shelves<'a>(cx: &Cx<'a, Bundle>, kind: Option<&str>) -> Vec<(String, Vec<&'a LibraryItem>)> {
    let mut groups = cx.views.library.by_type(Page::sort(cx));
    groups.retain(|(k, _)| kind.is_none_or(|want| want == k));
    groups
}

/// The type options: the types the library's own items carry (an addon's catalog type the library
/// holds nothing of is no choice: it could only ever show the empty read-out).
fn chip_kinds(cx: &Cx<'_, Bundle>) -> Vec<String> {
    if !matches!(cx.views.library.state(), LibraryState::Ready(_)) {
        return Vec::new();
    }
    let groups = cx.views.library.by_type(Sort::LastWatched);
    type_filter::chips(groups.iter().map(|(k, _)| k.as_str()))
}

/// The empty read-out's verdict while narrowed to `kind`: "No Series in your library".
fn none_of_type(kind: &str) -> std::ffi::CString {
    std::ffi::CString::new(msg::app_library_none_of_type(&type_filter::label(kind))).unwrap_or_default()
}

/// Is the page narrowed to a type it holds nothing of (its read-out offers *All types*)?
fn filtered_empty(cx: &Cx<'_, Bundle>, kind: Option<&str>) -> bool {
    kind.is_some() && status(cx, kind) == Some(StatusKind::Empty)
}

fn type_group(i: usize) -> GroupId {
    GroupId(1 + i as u32)
}

fn status(cx: &Cx<'_, Bundle>, kind: Option<&str>) -> Option<StatusKind> {
    match cx.views.account.session() {
        SessionState::Restoring => return Some(StatusKind::Working),
        SessionState::SignedOut { .. } => return Some(StatusKind::Failed),
        SessionState::SignedIn(_) => {}
    }
    match cx.views.library.state() {
        LibraryState::None | LibraryState::Loading => Some(StatusKind::Working),
        LibraryState::Failed(_) => Some(StatusKind::Failed),
        LibraryState::Ready(_) if shelves(cx, kind).is_empty() => Some(StatusKind::Empty),
        LibraryState::Ready(_) => None,
    }
}

/// The read-out; `filtered` is [`none_of_type`]'s verdict for the type chosen (the caller keeps it
/// alive for the draw).
fn overlay<'a>(cx: &Cx<'_, Bundle>, types: Option<&str>, ms: u32, filtered: &'a std::ffi::CStr) -> Option<StatusOverlay<'a>> {
    let kind = status(cx, types)?;
    let signed_out = matches!(cx.views.account.session(), SessionState::SignedOut { .. });
    Some(match kind {
        StatusKind::Working => StatusOverlay::new(Rect::FULL, msg::app_library_loading_c(), StatusKind::Working).phase(ms),
        StatusKind::Failed if signed_out => StatusOverlay::new(Rect::FULL, msg::app_library_signed_out_c(), StatusKind::Failed)
            .reason(msg::app_library_signed_out_reason_c())
            .action(msg::app_addons_sign_in_c())
            .page(sn_ui::icons::Icon::Alert),
        StatusKind::Failed => StatusOverlay::new(Rect::FULL, msg::app_library_failed_c(), StatusKind::Failed)
            .reason(msg::app_home_addons_failed_reason_c())
            .action(msg::app_home_retry_c())
            .page(sn_ui::icons::Icon::Alert),
        _ if types.is_some() => StatusOverlay::new(Rect::FULL, filtered, StatusKind::Empty).action(msg::app_filter_all_types_c()),
        _ => StatusOverlay::new(Rect::FULL, msg::app_library_empty_c(), StatusKind::Empty).reason(msg::app_library_empty_hint_c()),
    })
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        ((u64::from(cx.views.library.gen()) << 32) | (u64::from(cx.views.account.gen()) << 16) | self.cards.len() as u64)
            ^ (self.types.rev() << 44)
            ^ (self.sorts.rev() << 40)
            ^ (u64::from(self.list) << 63)
    }

    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        let kind = self.types.kind();
        let filtered = none_of_type(kind.unwrap_or_default());
        match overlay(cx, kind, 0, &filtered).filter(|_| status(cx, kind) == Some(StatusKind::Failed) || filtered_empty(cx, kind)) {
            Some(o) => {
                let rect = o.action_frame_measured(cx.measure).unwrap_or(Rect::FULL);
                out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: true }, STATUS_GROUP));
            }
            None => out.push(SectionSpec::new(Sec::Status, Kind::Custom { height: CONTENT_TOP, focusable: false }, STATUS_GROUP)),
        }
        if self.bar(cx) {
            out.push(SectionSpec::new(Sec::Types, Kind::Custom { height: dropdown::BAND, focusable: true }, TYPES_GROUP));
        }
        if status(cx, kind).is_some() {
            return;
        }
        for (i, _) in shelves(cx, kind).iter().enumerate() {
            if self.list {
                out.push(SectionSpec::new(Sec::Head(i), Kind::Custom { height: HEAD_TO_ROW, focusable: false }, GroupId(0x00FF_0000 + i as u32)));
                for j in 0..self.rows.get(i).map_or(0, Vec::len) {
                    out.push(SectionSpec::new(Sec::Item(i, j), Kind::Custom { height: item_list::ROW_H, focusable: true }, list_group(i, j)).ranked(i as u32));
                }
            } else {
                out.push(SectionSpec::new(Sec::Type(i), Kind::Shelf { style: &POSTER, heading: HEAD_TO_ROW }, type_group(i)).ranked(i as u32));
            }
        }
    }

    fn fallback(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        out.extend((0..shelves(cx, self.types.kind()).len()).flat_map(|i| [Sec::Type(i), Sec::Item(i, 0)]));
        if self.bar(cx) {
            out.push(Sec::Types);
        }
        out.push(Sec::Status);
    }

    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        let Sec::Type(i) = k else { return None };
        let (_, items) = shelves(cx, self.types.kind()).into_iter().nth(i)?;
        Some(ItemCards { items, cards: &self.cards })
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Status => Some(ACTION_ELEM),
            Sec::Item(i, j) => self.rows.get(i)?.get(j).copied(),
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
            Sec::Item(..) => item_list::face_rect(section),
            _ => section,
        }
    }

    fn plain_group(&self, _cx: &Cx<'_, Bundle>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        if k == Sec::Types {
            return dropdown::group(id, extent, self.bar_elems().len());
        }
        if let Sec::Item(..) = k {
            return item_list::group(id, extent);
        }
        GroupSpec {
            id,
            kind: sn_ui::screen::GroupKind::Free,
            seat: sn_ui::screen::Seat::First,
            reachable: sn_ui::screen::AxisMask::BOTH,
            edge: [sn_ui::screen::EdgeRule::Stop; 4],
            extent,
            len: 1,
            elem: sn_ui::screen::ElemKind::Bare,
        }
    }

    fn card_has_menu(&self, _cx: &Cx<'_, Bundle>, _k: Sec, _elem: &u32) -> bool {
        true
    }

    /// A shelf scrolled back into view stops under the top bar.
    fn reveal_margin(&self, _k: Sec) -> f32 {
        CONTENT_TOP
    }

    /// So does a list row.
    fn reveal_under_margin(&self, k: Sec) -> bool {
        matches!(k, Sec::Item(..) | Sec::Head(..) | Sec::Types)
    }

    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, lift: f32) {
        let Sec::Type(i) = k else { return };
        let Some((kind, items)) = shelves(f.cx, self.types.kind()).into_iter().nth(i) else { return };
        let p = f.painter.alpha(f.page_alpha);
        let title = kind_label(&kind).map_or_else(|| kind.clone(), str::to_string);
        let count = msg::app_library_count(items.len() as i64);
        sn_ui::cards::draw_heading(p, &title, &count, MARGIN_X, r.y + HEADING_TOP - lift, SCR_W - 2.0 * MARGIN_X, f.measure);
    }

    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        match k {
            Sec::Types => {
                let rects = self.bar_rects(f.measure, r.y);
                let shown: Vec<&Dropdown> = self.dropdowns().collect();
                for (d, rect) in shown.into_iter().zip(&rects) {
                    d.draw(f, *rect);
                }
                if let Some(chip) = rects.last() {
                    let focused = f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == TOGGLE_ELEM);
                    item_list::draw_toggle(f.painter.alpha(f.page_alpha), *chip, self.list, focused);
                }
                return;
            }
            Sec::Head(i) => {
                if r.y >= sn_ui::consts::SCR_H || r.y + r.h <= 0.0 {
                    return;
                }
                let Some((kind, items)) = shelves(f.cx, self.types.kind()).into_iter().nth(i) else { return };
                let title = kind_label(&kind).map_or_else(|| kind.clone(), str::to_string);
                let count = msg::app_library_count(items.len() as i64);
                sn_ui::cards::draw_heading(f.painter.alpha(f.page_alpha), &title, &count, MARGIN_X, r.y + HEADING_TOP, SCR_W - 2.0 * MARGIN_X, f.measure);
                return;
            }
            Sec::Item(i, j) => {
                let Some(item) = shelves(f.cx, self.types.kind()).into_iter().nth(i).and_then(|(_, items)| items.get(j).copied()) else { return };
                let focused = f.focus.current.is_some_and(|key| key.entry == self.entry && Some(key.elem) == self.elem_of(k));
                item_list::draw(f, r, &ListRow::of_library(item), focused);
                return;
            }
            _ => {}
        }
        if k != Sec::Status {
            return;
        }
        let filtered = none_of_type(self.types.kind().unwrap_or_default());
        if let Some(o) = overlay(f.cx, self.types.kind(), f.cx.tick.ms, &filtered) {
            let focused = f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == ACTION_ELEM);
            o.focused(focused).draw_measured(&Env::inert(), f.painter.alpha(f.page_alpha), f.measure);
        }
    }
}

/// A sort's label (Stremio's own words for them).
pub fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::LastWatched => msg::app_library_sort_last_watched(),
        Sort::Name => msg::app_library_sort_name(),
        Sort::NameReverse => msg::app_library_sort_name_reverse(),
        Sort::TimesWatched => msg::app_library_sort_times_watched(),
        Sort::Watched => msg::app_library_sort_watched(),
        Sort::NotWatched => msg::app_library_sort_not_watched(),
    }
}

pub struct LibraryScreen {
    page: Page,
    stack: Stack<Sec>,
    seat: FirstSeat,
    /// The viewer has moved focus themselves: no landing re-seats over it.
    chose: bool,
    /// The group UP from which reaches the bar, as last declared.
    first: Option<GroupId>,
}

impl LibraryScreen {
    pub fn new(entry: EntryId) -> Self {
        LibraryScreen {
            page: Page {
                entry,
                cards: Interned::with_base(FIRST_CARD_ELEM),
                types: Dropdown::new(type_filter::Page::Library, entry, TYPE_ELEM).all_label(msg::app_filter_all_types),
                sorts: Dropdown::row(entry, SORT_ELEM),
                list: false,
                rows: Vec::new(),
            },
            stack: Stack::new(entry),
            seat: FirstSeat::new(),
            chose: false,
            first: None,
        }
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    /// How many type options the dropdown offers (none: it is not shown), for the tests.
    pub fn type_options(&self) -> usize {
        self.page.types.len()
    }

    fn intern(&mut self, store: &LibraryStore) -> bool {
        let before = self.page.cards.len();
        for (_, items) in store.by_type(Sort::LastWatched) {
            for item in items {
                self.page.cards.elem(&item.id);
            }
        }
        self.page.cards.len() != before
    }

    fn item<'a>(&self, cx: &Cx<'a, Bundle>, elem: u32) -> Option<&'a LibraryItem> {
        cx.views.library.items().iter().find(|i| self.page.cards.get(&i.id) == Some(elem))
    }

    fn open(&self, cx: &Cx<'_, Bundle>, elem: u32, fx: &mut Effects<'_, Bundle>) {
        if let Some(item) = self.item(cx, elem) {
            fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(ItemRef { kind: item.kind.clone(), id: item.id.clone(), name: item.name.clone(), from: crate::tabs::Tab::Library.label().to_string() }))));
        }
    }

    /// The page's first focusable group: the first shelf (or list row), or a failed read-out's
    /// action.
    fn first_group(&self, cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        match status(cx, self.page.types.kind()) {
            None if self.page.list => Some(list_group(0, 0)),
            None => Some(type_group(0)),
            Some(StatusKind::Failed) => Some(STATUS_GROUP),
            Some(StatusKind::Empty) if self.page.types.kind().is_some() => Some(STATUS_GROUP),
            _ => None,
        }
    }

    /// The list setting and, in list mode, each group's elements (what `plain_elem` reads).
    fn sync_list(&mut self, cx: &Cx<'_, Bundle>) -> bool {
        let list = cx.views.settings.list(ViewPage::Library);
        let rows: Vec<Vec<u32>> = if list {
            shelves(cx, self.page.types.kind()).iter().map(|(_, items)| items.iter().filter_map(|i| self.page.cards.get(&i.id)).collect()).collect()
        } else {
            Vec::new()
        };
        let changed = list != self.page.list || rows != self.page.rows;
        self.page.list = list;
        self.page.rows = rows;
        changed
    }

    /// The View chip: posters ⇄ list.
    fn toggle(&self, fx: &mut Effects<'_, Bundle>) {
        fx.push(Fx::App(AppFx::Settings(SettingsCmd::List(ViewPage::Library, !self.page.list))));
        fx.invalidate(Provenance::Input);
    }

    /// OK or a click on the type dropdown: its popover, under it.
    fn open_types(&self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let rect = match self.stack.focused_plain(&self.page, cx) {
            Some((Sec::Types, r)) if cx.focus.current.is_some_and(|k| self.page.types.is(k.elem)) => r,
            _ => self.page.bar_rects(cx.measure, CONTENT_TOP - self.stack.scroll())[0],
        };
        dropdown::open(self.page.entry, cx, fx, rect, self.page.types.menu(msg::app_filter_type(), ChoiceTarget::LibraryType));
    }

    /// The sort dropdown's options (once there is a library to order) and its choice, the setting's.
    fn sync_sorts(&mut self, cx: &Cx<'_, Bundle>) -> bool {
        let shown = matches!(cx.views.library.state(), LibraryState::Ready(_)) && !cx.views.library.by_type(Sort::LastWatched).is_empty();
        let options = if shown { Sort::ALL.iter().map(|s| (s.name().to_string(), sort_label(*s).to_string())).collect() } else { Vec::new() };
        let changed = self.page.sorts.sync_named(options);
        let before = self.page.sorts.rev();
        self.page.sorts.set_chosen(Page::sort(cx).name());
        changed || before != self.page.sorts.rev()
    }

    /// OK or a click on the sort dropdown: its popover, under it.
    fn open_sorts(&self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let at = self.page.bar_elems().iter().position(|e| *e == SORT_ELEM).unwrap_or(0);
        let rect = match self.stack.focused_plain(&self.page, cx) {
            Some((Sec::Types, r)) if cx.focus.current.is_some_and(|k| self.page.sorts.is(k.elem)) => r,
            _ => self.page.bar_rects(cx.measure, CONTENT_TOP - self.stack.scroll())[at],
        };
        dropdown::open(self.page.entry, cx, fx, rect, self.page.sorts.menu(msg::app_library_sort(), ChoiceTarget::LibrarySort));
    }

    /// A held OK on a card or a list row: the item's menu.
    fn menu(&self, cx: &Cx<'_, Bundle>, elem: u32, at: Option<Rect>, fx: &mut Effects<'_, Bundle>) {
        let Some(item) = self.item(cx, elem) else { return };
        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
            host: self.page.entry,
            reply: fx.from(),
            focus: cx.focus.current,
            anchor: MenuArg::anchor_of(at.unwrap_or_else(crate::menu::fallback_anchor)),
            kind: item_cards::item_menu(item, crate::tabs::Tab::Library.label()),
        }))));
    }

    /// A pick from the type popover: the shelves are narrowed to its type.
    fn choose(&mut self, n: usize, fx: &mut Effects<'_, Bundle>) {
        if self.page.types.choose(n) {
            self.stack.scroll_to(0.0);
        }
        fx.invalidate(Provenance::Input);
    }
}

impl LogicalState for LibraryScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.page.entry.0).u32(self.page.cards.len() as u32);
        self.stack.write(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("library cards={}", self.page.cards.len()));
    }
}

impl Machine<Bundle> for LibraryScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        let grew = match ev {
            ScreenEvent::Tick(_) | ScreenEvent::Mount | ScreenEvent::StoreChanged(..) => {
                self.intern(cx.views.library) | self.page.types.sync(chip_kinds(cx)) | self.sync_sorts(cx)
            }
            _ => false,
        };
        let grew = grew | self.sync_list(cx);
        let got = self.stack.on(&self.page, ev, cx, fx);
        match ev {
            ScreenEvent::Mount => {
                fx.push(Fx::App(AppFx::Library(LibraryCmd::Reload)));
                Handled::Yes
            }
            ScreenEvent::Tick(_) => {
                let first = self.first_group(cx);
                // a type with nothing in it still has its dropdown to choose another from
                let seat = first.or(self.page.bar(cx).then_some(TYPES_GROUP));
                let seated = crate::tabs::rests(self.page.entry, Tab::Library, cx, self.chose, seat.is_some());
                let target = seat.map(FocusTarget::FirstInGroup).unwrap_or(FocusTarget::Elem(FocusKey {
                    entry: self.page.entry,
                    elem: Tab::Library.elem(),
                }));
                self.seat.tick(seated, Some(target), fx);
                self.first = if self.page.bar(cx) { Some(TYPES_GROUP) } else { first };
                if grew {
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _)
                if *ord == StoreId::Library.ord() || *ord == StoreId::Account.ord() || *ord == StoreId::Settings.ord() =>
            {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Activate(elem))) = got {
                    self.open(cx, elem, fx);
                } else if let Some(k) = cx.focus.current.filter(|k| k.entry == self.page.entry && crate::tabs::on_strip(k.elem)) {
                    crate::tabs::on_pill(k.elem, Tab::Library, self.first_group(cx), cx, fx);
                } else if cx.focus.current.is_some_and(|k| k.entry == self.page.entry && self.page.types.is(k.elem)) {
                    self.open_types(cx, fx);
                } else if cx.focus.current.is_some_and(|k| k.entry == self.page.entry && self.page.sorts.is(k.elem)) {
                    self.open_sorts(cx, fx);
                } else if let Some(k) = cx.focus.current.filter(|k| k.entry == self.page.entry) {
                    match k.elem {
                        TOGGLE_ELEM => self.toggle(fx),
                        // a list row: the Stack reports no press for a plain element, focus names it
                        elem if self.page.cards.contains(elem) => self.open(cx, elem, fx),
                        _ => {}
                    }
                }
                Handled::Yes
            }
            ScreenEvent::Activate(TOGGLE_ELEM) => {
                self.toggle(fx);
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => {
                self.chose = true;
                crate::tabs::on_back(self.page.entry, Tab::Library, cx, fx)
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, Tab::Library, self.first_group(cx), cx, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if self.page.types.is(*elem) => {
                self.open_types(cx, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if self.page.sorts.is(*elem) => {
                self.open_sorts(cx, fx);
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::Chose(ChoiceTarget::LibrarySort, n)) => {
                if let Some(sort) = Sort::ALL.get(*n) {
                    fx.push(Fx::App(AppFx::Settings(SettingsCmd::LibrarySort(*sort))));
                    self.stack.scroll_to(0.0);
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                // a card reports its hold; a list row is the same item, held through its plain stop
                let held = match got {
                    Some(StackEvent::Card(_, CardEvent::Hold(elem))) => Some((elem, self.stack.focused_card(&self.page, cx, At::Drawn).map(|(_, at)| at.rect))),
                    _ => self.stack.focused_plain(&self.page, cx).and_then(|(k, rect)| match k {
                        Sec::Item(..) => self.page.elem_of(k).map(|e| (e, Some(rect))),
                        _ => None,
                    }),
                };
                if let Some((elem, at)) = held {
                    self.menu(cx, elem, at, fx);
                }
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::Chose(ChoiceTarget::LibraryType, n)) => {
                self.choose(*n, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if *elem == ACTION_ELEM => {
                if filtered_empty(cx, self.page.types.kind()) {
                    // *All types*: the dropdown's first option
                    self.choose(0, fx);
                } else if matches!(cx.views.account.session(), SessionState::SignedOut { .. }) {
                    fx.push(Fx::Nav(NavOp::Push(AppArg::SignIn)));
                } else {
                    fx.push(Fx::App(AppFx::Library(LibraryCmd::Reload)));
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.open(cx, *elem, fx);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { from, by, .. } => {
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

sn_ui::focusable_via_view!(LibraryScreen, Bundle, view);

impl Screen<Bundle> for LibraryScreen {
    fn name(&self) -> &'static str {
        word::LIBRARY
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<std::borrow::Cow<'_, str>> {
        None
    }
    fn links(&self, out: &mut Vec<Link>) {
        if let Some(first) = self.first {
            crate::tabs::links(first, out);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

    /// Every sort's label fits the dropdown's button uncapped, and the sort popover fits its
    /// panel, in every shipped language.
    #[test]
    fn the_sort_labels_fit_in_every_shipped_language() {
        let m = sn_base::fontcov::advances::ShippedMeasure;
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let mut d = Dropdown::row(EntryId(1), SORT_ELEM);
            d.sync_named(Sort::ALL.iter().map(|s| (s.name().to_string(), sort_label(*s).to_string())).collect());
            for s in Sort::ALL {
                let w = dropdown::face_w(sort_label(s), &m);
                assert!(w < dropdown::BUTTON_W_CAP, "{}: {:?} is {w}px", language.tag(), sort_label(s));
            }
            let mut t = sn_ui::form::FormTable::<crate::menu::MenuRow, crate::menu::MenuRow, std::convert::Infallible>::new(0x1000);
            t.table.compact = true;
            t.set(crate::menu::build(&d.menu(msg::app_library_sort(), ChoiceTarget::LibrarySort), Default::default(), crate::stores::library::AddState::Absent), None);
            out.extend(t.table.app_fit_failures_hugged(language.tag()));
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }
}
