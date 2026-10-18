//! **One catalog, whole** — the full paginated grid a search row's MORE (and a Home row's linked heading) opens: the row's addon and
//! catalog under the same query, every page the addon holds, loaded as the grid reaches its end.
//!
//! A `cards::Stack` of a heading, the grid, and while there are no cards the read-out for why:
//! loading (the spinner), nothing, a failure with its reason and a Retry, or an addon that is not
//! installed any more. OK on a card opens its Detail page; a held OK its item menu. The view is
//! held open in the grid store for as long as the page lives ([`GridCmd::Open`] / `Close`), and a
//! search row's grid opens on the cards the row already holds.
//!
//! The heading carries the *View* chip (`crate::item_list`): posters, or the same items as a list
//! — one row per item, paged the same way (the next page is asked once focus is within a
//! screenful of the end). The choice is a setting (`ViewPage::Catalog`).

use std::borrow::Cow;

use sn_machine::machine::{Canon, Cx, Effects, EntryId, Fx, GroupId, Handled, Leave, LogicalState, Machine, NavOp};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_ui::cards::{CardEvent, GridSpec, Kind, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{MARGIN_X, MARGIN_Y, SCR_W};
use sn_ui::screen::{At, AxisMask, DrawFrame, EdgeRule, ElemKind, Focusable, GroupKind, GroupSpec, RenderStrategy, Screen, ScreenEvent, Seat};
use sn_ui::widgets::{Button, StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect, View};

use crate::cards::{item_ref, ItemCards};
use crate::item_list::{self, ListRow};
use crate::registry::{kind_label, word, AppArg, AppFx, Bundle, CatalogRef, FirstSeat, GridCmd, MenuArg, SettingsCmd};
use crate::stores::settings::ViewPage;
use crate::stores::grid::{GridKey, GridState, GridStore};
use crate::stores::StoreId;

const CARD_BASE: u32 = 0x1000;
const RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 1;
const TOGGLE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 2;
/// The *Try again* after the last item when the next page failed ([`Sec::MoreFailed`]).
pub const MORE_RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 3;
const MORE_GROUP: GroupId = GroupId(3);
fn list_group(i: usize) -> GroupId {
    GroupId(0x10_0000 + i as u32)
}
const HEAD_GROUP: GroupId = GroupId(1);
const ITEMS_GROUP: GroupId = GroupId(0);
const STATUS_GROUP: GroupId = GroupId(2);
/// The heading band above the grid.
const HEAD_H: f32 = MARGIN_Y + 96.0;
const HEADING_TOP: f32 = MARGIN_Y + theme::space::LG;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    Head,
    Status,
    Items,
    /// List mode: item `i`.
    Row(usize),
    /// After the last item: the next page failed, *Try again* ([`more_failed_rect`]).
    MoreFailed,
}

/// The band after a listing whose next page failed: room for its one pill.
pub const MORE_FAILED_H: f32 = 140.0;

/// The *Couldn't load more · Try again* pill, centred in its band `section`.
pub fn more_failed_rect(section: Rect, measure: &dyn sn_machine::machine::Measure) -> Rect {
    let h = StatusOverlay::CTRL_H;
    let w = Button::pill_w_measured(msg::app_grid_more_failed_c(), theme::size::LABEL, true, false, measure);
    Rect::new((SCR_W - w) * 0.5, section.y + (MORE_FAILED_H - h) * 0.5, w, h)
}

/// Draw that pill.
pub fn draw_more_failed(p: sn_ui::Painter, rect: Rect, focused: bool) {
    Button::new(msg::app_grid_more_failed_c().as_ptr(), theme::size::LABEL, rect).icon(sn_ui::icons::Icon::Restart).focused(focused).draw(&Env::inert(), p);
}

/// The read-out a grid shows in place of its cards, if any.
pub fn readout(state: &GridState, frame: Rect) -> Option<StatusOverlay<'static>> {
    Some(match state {
        GridState::Ready { .. } | GridState::MoreFailed => return None,
        GridState::Loading => StatusOverlay::new(frame, msg::app_grid_loading_c(), StatusKind::Working),
        GridState::Empty => StatusOverlay::new(frame, msg::app_grid_empty_c(), StatusKind::Empty),
        GridState::Missing => StatusOverlay::new(frame, msg::app_grid_missing_c(), StatusKind::Empty),
        GridState::Failed(_) => StatusOverlay::new(frame, msg::app_grid_failed_c(), StatusKind::Failed)
            .reason(msg::app_grid_failed_reason_c())
            .action(msg::app_home_retry_c()),
    })
}

struct Page {
    key: GridKey,
    title: String,
    note: String,
    /// The items show as a list (the setting, as of the last event).
    list: bool,
}

impl Page {
    fn state(&self, store: &GridStore) -> GridState {
        store.state(&self.key).unwrap_or(GridState::Loading)
    }
    fn status_frame() -> Rect {
        Rect::new(0.0, HEAD_H, SCR_W, sn_ui::consts::SCR_H - HEAD_H)
    }
    /// The View chip, at the heading's right end.
    fn toggle_rect(measure: &dyn sn_machine::machine::Measure, section_y: f32) -> Rect {
        let w = item_list::toggle_width(measure);
        Rect::new(SCR_W - MARGIN_X - w, section_y + HEAD_H - sn_ui::widgets::TAB_PILL_H - theme::space::MD, w, sn_ui::widgets::TAB_PILL_H)
    }
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(cx.views.grid.gen()) << 32) | cx.views.grid.items(&self.key).len() as u64 | (u64::from(self.list) << 31)
    }
    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        out.push(SectionSpec::new(Sec::Head, Kind::Custom { height: HEAD_H, focusable: true }, HEAD_GROUP).ranked(2));
        let state = self.state(cx.views.grid);
        if let Some(o) = readout(&state, Self::status_frame()) {
            let failed = matches!(state, GridState::Failed(_));
            let rect = if failed { o.action_frame_measured(cx.measure).unwrap_or(Self::status_frame()) } else { Self::status_frame() };
            out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: failed }, STATUS_GROUP).ranked(1));
        }
        if self.list {
            for i in 0..cx.views.grid.items(&self.key).len() {
                out.push(SectionSpec::new(Sec::Row(i), Kind::Custom { height: item_list::ROW_H, focusable: true }, list_group(i)));
            }
        } else {
            out.push(SectionSpec::new(Sec::Items, Kind::Grid { spec: GridSpec::new(0.0, MARGIN_Y) }, ITEMS_GROUP));
        }
        if state == GridState::MoreFailed {
            out.push(SectionSpec::new(Sec::MoreFailed, Kind::Custom { height: MORE_FAILED_H, focusable: true }, MORE_GROUP));
        }
    }
    fn fallback(&self, _cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        out.extend([Sec::Items, Sec::Row(0), Sec::MoreFailed, Sec::Status, Sec::Head]);
    }
    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        (k == Sec::Items).then(|| ItemCards {
            items: cx.views.grid.items(&self.key),
            base: CARD_BASE,
            more: matches!(self.state(cx.views.grid), GridState::Ready { more: true }),
            watched: Some(cx.views.library),
        })
    }
    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Status => Some(RETRY_ELEM),
            Sec::Head => Some(TOGGLE_ELEM),
            Sec::Row(i) => Some(CARD_BASE + i as u32),
            Sec::MoreFailed => Some(MORE_RETRY_ELEM),
            Sec::Items => None,
        }
    }
    fn element_rect(&self, cx: &Cx<'_, Bundle>, k: Sec, _n: usize, section: Rect) -> Rect {
        match k {
            Sec::Head => Self::toggle_rect(cx.measure, section.y),
            Sec::Row(_) => item_list::face_rect(section),
            Sec::MoreFailed => more_failed_rect(section, cx.measure),
            _ => section,
        }
    }
    fn plain_group(&self, _cx: &Cx<'_, Bundle>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        if let Sec::Row(_) = k {
            return item_list::group(id, extent);
        }
        let edge = if k == Sec::Head { [EdgeRule::Stop, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop] } else { [EdgeRule::Stop; 4] };
        GroupSpec { id, kind: GroupKind::Free, seat: Seat::First, reachable: AxisMask::BOTH, edge, extent, len: 1, elem: ElemKind::Bare }
    }
    fn reveal_under_margin(&self, k: Sec) -> bool {
        matches!(k, Sec::Row(_))
    }
    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        let p = f.painter.alpha(f.page_alpha);
        match k {
            Sec::Head => {
                let chip = Self::toggle_rect(f.measure, r.y);
                let w = chip.x - theme::space::LG - MARGIN_X;
                sn_ui::cards::draw_heading(p, &self.title, &self.note, MARGIN_X, r.y + HEADING_TOP, w, f.measure);
                let focused = f.focus.current.is_some_and(|key| key.elem == TOGGLE_ELEM);
                item_list::draw_toggle(p, chip, self.list, focused);
            }
            Sec::Row(i) => {
                let Some(item) = f.cx.views.grid.items(&self.key).get(i) else { return };
                let focused = f.focus.current.is_some_and(|key| key.elem == CARD_BASE + i as u32);
                item_list::draw(f, r, &ListRow::of_meta(item, None), focused);
            }
            Sec::Status => {
                let state = self.state(f.cx.views.grid);
                if let Some(o) = readout(&state, Self::status_frame()) {
                    let focused = f.focus.current.is_some_and(|key| key.elem == RETRY_ELEM);
                    o.phase(f.cx.tick.ms).focused(focused).draw_measured(&Env::inert(), p, f.measure);
                }
            }
            Sec::MoreFailed => {
                let focused = f.focus.current.is_some_and(|key| key.elem == MORE_RETRY_ELEM);
                draw_more_failed(p, more_failed_rect(r, f.measure), focused);
            }
            Sec::Items => {}
        }
    }
}

pub struct GridScreen {
    entry: EntryId,
    page: Page,
    stack: Stack<Sec>,
    seat: FirstSeat,
    open: bool,
    /// How many items the list had when it last asked for the next page (one ask per length).
    asked_at: Option<usize>,
}

impl GridScreen {
    pub fn new(entry: EntryId, r: CatalogRef) -> Self {
        let kind = kind_label(&r.key.kind).map_or_else(|| r.key.kind.clone(), str::to_string);
        let note = match (&r.key.search, &r.key.genre) {
            (Some(q), _) => msg::app_grid_results_for(&kind, q),
            (None, Some(g)) => format!("{kind} \u{b7} {g}"),
            (None, None) => kind,
        };
        GridScreen {
            entry,
            page: Page { key: r.key, title: r.title, note, list: false },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Search),
            seat: FirstSeat::new(),
            open: false,
            asked_at: None,
        }
    }

    pub fn key(&self) -> &GridKey {
        &self.page.key
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    fn close(&mut self, fx: &mut Effects<'_, Bundle>) {
        if std::mem::replace(&mut self.open, false) {
            fx.push(Fx::App(AppFx::Grid(GridCmd::Close(self.page.key.clone()))));
        }
    }
}

impl LogicalState for GridScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).bool(self.open);
        self.page.key.write(c);
        self.stack.write(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str("catalog grid");
    }
}

impl Machine<Bundle> for GridScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        if let ScreenEvent::Mount = ev {
            self.open = true;
            fx.push(Fx::App(AppFx::Grid(GridCmd::Open(self.page.key.clone()))));
        }
        self.page.list = cx.views.settings.list(ViewPage::Catalog);
        let got = self.stack.on(&self.page, ev, cx, fx);
        let cards = |cx: &Cx<'_, Bundle>| cx.views.grid.items(&self.page.key).len();
        match ev {
            ScreenEvent::Tick(_) => {
                if let Some(StackEvent::Card(Sec::Items, CardEvent::Want(range))) = &got {
                    if range.end > cards(cx) {
                        fx.push(Fx::App(AppFx::Grid(GridCmd::More(self.page.key.clone()))));
                    }
                }
                let len = cards(cx);
                if self.page.list && matches!(self.page.state(cx.views.grid), GridState::Ready { more: true }) && self.asked_at != Some(len) {
                    let at = cx.focus.current.and_then(|k| k.elem.checked_sub(CARD_BASE)).map(|i| i as usize).filter(|i| *i < len);
                    if item_list::wants_more(at, len) {
                        self.asked_at = Some(len);
                        fx.push(Fx::App(AppFx::Grid(GridCmd::More(self.page.key.clone()))));
                    }
                }
                let items = if self.page.list { list_group(0) } else { ITEMS_GROUP };
                let target = (len > 0).then_some(sn_ui::screen::FocusTarget::FirstInGroup(items)).or_else(|| {
                    matches!(self.page.state(cx.views.grid), GridState::Failed(_))
                        .then_some(sn_ui::screen::FocusTarget::FirstInGroup(STATUS_GROUP))
                });
                self.seat.tick(cx.focus.current.is_some_and(|k| k.entry == self.entry), target, fx);
                if self.page.state(cx.views.grid) == GridState::Loading {
                    fx.note(sn_machine::present::PresentEvent::Motion);
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Grid.ord() || *ord == StoreId::Settings.ord() => {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) | ScreenEvent::Activate(_) => {
                match (&got, ev) {
                    (Some(StackEvent::Card(_, CardEvent::Activate(elem))), _) | (_, ScreenEvent::Activate(elem))
                        if *elem >= CARD_BASE && *elem < RETRY_ELEM =>
                    {
                        let items = ItemCards { items: cx.views.grid.items(&self.page.key), base: CARD_BASE, more: false, watched: None };
                        if let Some(item) = items.item(*elem) {
                            fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(item, &self.page.title)))));
                        }
                    }
                    // a list row's OK: the Stack reports no press for a plain element, focus names it
                    (None, ScreenEvent::PressCommit(_)) if self.page.list => {
                        let items = ItemCards { items: cx.views.grid.items(&self.page.key), base: CARD_BASE, more: false, watched: None };
                        if let Some(item) = cx.focus.current.filter(|k| k.entry == self.entry).and_then(|k| items.item(k.elem)) {
                            fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(item, &self.page.title)))));
                        }
                    }
                    (Some(StackEvent::Press(Sec::Status | Sec::MoreFailed)), _)
                    | (_, ScreenEvent::Activate(RETRY_ELEM | MORE_RETRY_ELEM)) => {
                        fx.push(Fx::App(AppFx::Grid(GridCmd::Retry(self.page.key.clone()))));
                    }
                    (Some(StackEvent::Press(Sec::Head)), _) | (_, ScreenEvent::Activate(TOGGLE_ELEM)) => {
                        fx.push(Fx::App(AppFx::Settings(SettingsCmd::List(ViewPage::Catalog, !self.page.list))));
                    }
                    _ => {}
                }
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                // a card reports its hold; a list row is the same item, held through its plain stop
                let held = match got {
                    Some(StackEvent::Card(_, CardEvent::Hold(elem))) => Some((elem, self.stack.focused_card(&self.page, cx, At::Drawn).map(|(_, at)| at.rect))),
                    _ => self.stack.focused_plain(&self.page, cx).and_then(|(k, rect)| match k {
                        Sec::Row(i) => Some((CARD_BASE + i as u32, Some(rect))),
                        _ => None,
                    }),
                };
                if let Some((elem, at)) = held {
                    let items = ItemCards { items: cx.views.grid.items(&self.page.key), base: CARD_BASE, more: false, watched: None };
                    if let Some(item) = items.item(elem) {
                        let anchor = at.unwrap_or_else(crate::menu::fallback_anchor);
                        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
                            host: self.entry,
                            reply: fx.from(),
                            focus: cx.focus.current,
                            anchor: MenuArg::anchor_of(anchor),
                            kind: crate::cards::item_menu(item, &self.page.title),
                        }))));
                    }
                }
                Handled::Yes
            }
            ScreenEvent::WillLeave(Leave::ForGood) | ScreenEvent::Unmount => {
                self.close(fx);
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

sn_ui::focusable_via_view!(GridScreen, Bundle, view);

impl Screen<Bundle> for GridScreen {
    fn name(&self) -> &'static str {
        word::CATALOG
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
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
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{AppFx, GridCmd};
    use crate::stores::catalog::CatalogStore;
    use crate::stores::search::SearchLanding;
    use crate::test_support::TestApp;
    use sn_machine::machine::{Key, MachineId};
    use sn_stremio::manifest::{Addon, Manifest};
    use sn_stremio::types::{CatalogPage, MetaPreview};

    const CINEMETA: &str = r#"{"id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta","resources":["catalog","meta"],
        "types":["movie"],"catalogs":[{"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]}]}"#;

    /// A search row's whole catalog opened as a list: 30 items, more to come.
    fn listed() -> TestApp {
        let addon = Addon::new("https://a.example/cinemeta/manifest.json", Manifest::parse(CINEMETA.as_bytes()).unwrap());
        let mut app = TestApp::boot(CatalogStore::from_addons(vec![addon]));
        app.notify(StoreId::Catalog, 1);
        app.rig.settings.set_list(ViewPage::Catalog, true);
        app.d.request(MachineId::Nav, NavOp::SelectTab(AppArg::Search));
        app.frames(5);
        let addons = app.rig.catalog.addons().to_vec();
        let jobs = app.rig.search.submit("bat", &addons, app.rig.addons.filter());
        let page = CatalogPage {
            metas: (0..30).map(|i| MetaPreview { id: format!("tt{i}"), kind: "movie".into(), name: format!("T{i}"), ..Default::default() }).collect(),
        };
        app.rig.search.land(SearchLanding { qgen: jobs[0].qgen, row: 0, result: Ok(page) });
        app.frames(4);
        app.press(Key::Down);
        app.press(Key::Down);
        app.press(Key::Ok);
        app.frames(5);
        assert_eq!(app.top_name(), "catalog");
        app
    }

    fn mores(app: &mut TestApp) -> usize {
        app.take_effects().iter().filter(|fx| matches!(fx, AppFx::Grid(GridCmd::More(_)))).count()
    }

    #[test]
    fn the_list_opens_on_its_first_row_and_asks_for_the_next_page_only_near_its_end() {
        let _g = sn_base::testlock::serial();
        let mut app = listed();
        assert_eq!(app.d.focus().map(|k| k.elem), Some(CARD_BASE), "seated on the first row");
        assert_eq!(mores(&mut app), 0, "a screenful from the end, nothing is asked");
        for _ in 0..12 {
            app.press(Key::Down);
        }
        assert_eq!(app.d.focus().map(|k| k.elem), Some(CARD_BASE + 12), "DOWN walks the rows");
        assert_eq!(mores(&mut app), 0);
        for _ in 0..10 {
            app.press(Key::Down);
        }
        assert_eq!(mores(&mut app), 1, "within a screenful of the end: the next page, asked once");
        app.press(Key::Ok);
        assert!(matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id == "tt22"), "OK opens the row's item: {:?}", app.top_arg());
    }

    /// A next page that failed is not the end of the catalog: after the last row stands *Couldn't
    /// load more · Try again*, DOWN reaches it and OK asks the page again.
    #[test]
    fn a_failed_next_page_ends_in_a_reachable_try_again() {
        let _g = sn_base::testlock::serial();
        let mut app = listed();
        let key = app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<GridScreen>()).unwrap().key().clone();
        let _ = app.rig.grid.more(&key);
        assert!(app.rig.grid.loading(&key), "the next page is out");
        app.rig.grid.land(crate::stores::grid::GridLanding { key, result: Err(sn_stremio::FetchError::Status(503)) });
        app.frames(3);
        for _ in 0..31 {
            app.press(Key::Down);
        }
        assert_eq!(app.d.focus().map(|k| k.elem), Some(MORE_RETRY_ELEM), "DOWN from the last row reaches Try again");
        assert!(app.drawn_text().iter().any(|t| t.contains(msg::app_grid_more_failed())));
        app.take_effects();
        app.press(Key::Ok);
        let retries = app.take_effects().into_iter().filter(|fx| matches!(fx, AppFx::Grid(GridCmd::Retry(_)))).count();
        assert_eq!(retries, 1, "OK asks the failed page again, once");
    }

    #[test]
    fn the_view_chip_in_the_heading_switches_back_to_posters() {
        let _g = sn_base::testlock::serial();
        let mut app = listed();
        app.press(Key::Up);
        assert_eq!(app.d.focus().map(|k| k.elem), Some(TOGGLE_ELEM), "UP from the first row reaches the View chip");
        app.press(Key::Ok);
        assert!(!app.rig.settings.list(ViewPage::Catalog));
        app.frames(3);
        app.press(Key::Down);
        assert!(app.d.focus().is_some_and(|k| (CARD_BASE..CARD_BASE + 30).contains(&k.elem)), "a poster of the grid");
    }
}
