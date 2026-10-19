//! **Continue Watching, whole** — the page Home's Continue Watching heading opens: every item in
//! progress (`item_cards::continue_items`, the library store's own list: nothing is fetched), as
//! the catalog page's grid (`grid.rs`) lays a catalog out, or as list rows (`crate::item_list`)
//! with each row's resume bar and time left.
//!
//! OK on a card or a row RESUMES exactly as the Home shelf's card does (`item_cards::resume_arg`);
//! a held OK opens the item menu. The heading carries the *View* chip, Home's choice
//! (`ViewPage::Home`): this page is Home's shelf expanded.

use std::borrow::Cow;

use sn_machine::machine::{Canon, Cx, Effects, EntryId, Fx, GroupId, Handled, InputEvent, InputKind, Edge, LogicalState, Machine, NavOp};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_ui::cards::{CardEvent, GridSpec, Kind, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{MARGIN_X, MARGIN_Y, SCR_W};
use sn_ui::screen::{At, AxisMask, DrawFrame, EdgeRule, ElemKind, Focusable, GroupKind, GroupSpec, RenderStrategy, Screen, ScreenEvent, Seat};
use sn_ui::widgets::{StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect};
use sn_stremio::library::LibraryItem;

use crate::item_cards::{self, continue_items, ItemCards};
use crate::item_list::{self, ListRow};
use crate::registry::{word, AppArg, AppFx, Bundle, FirstSeat, MenuArg, SettingsCmd};
use crate::stores::settings::ViewPage;
use crate::stores::StoreId;

pub const CARD_BASE: u32 = 0x1000;
pub const TOGGLE_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 2;
fn list_group(i: usize) -> GroupId {
    GroupId(0x10_0000 + i as u32)
}
const HEAD_GROUP: GroupId = GroupId(1);
const ITEMS_GROUP: GroupId = GroupId(0);
const STATUS_GROUP: GroupId = GroupId(2);
/// The heading band above the grid (the catalog page's).
const HEAD_H: f32 = MARGIN_Y + 96.0;
const HEADING_TOP: f32 = MARGIN_Y + theme::space::LG;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    Head,
    Status,
    Items,
    /// List mode: item `i`.
    Row(usize),
}

struct Page {
    entry: EntryId,
    cards: item_cards::Interned,
    /// The items show as a list (the setting, as of the last event).
    list: bool,
    /// List mode: the rows' elements, as of the last event (`elem_of` has no `Cx`).
    rows: Vec<u32>,
}

impl Page {
    fn status_frame() -> Rect {
        Rect::new(0.0, HEAD_H, SCR_W, sn_ui::consts::SCR_H - HEAD_H)
    }
    /// The View chip, at the heading's right end (the catalog page's place).
    fn toggle_rect(measure: &dyn sn_machine::machine::Measure, section_y: f32) -> Rect {
        let w = item_list::toggle_width(measure);
        Rect::new(SCR_W - MARGIN_X - w, section_y + HEAD_H - sn_ui::widgets::TAB_PILL_H - theme::space::MD, w, sn_ui::widgets::TAB_PILL_H)
    }
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(cx.views.library.gen()) << 32) ^ (u64::from(cx.views.account.gen()) << 16) ^ self.cards.len() as u64 ^ (u64::from(self.list) << 63)
    }
    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        out.push(SectionSpec::new(Sec::Head, Kind::Custom { height: HEAD_H, focusable: true }, HEAD_GROUP).ranked(2));
        if continue_items(cx).is_empty() {
            out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect: Self::status_frame(), focusable: false }, STATUS_GROUP).ranked(1));
        } else if self.list {
            for i in 0..self.rows.len() {
                out.push(SectionSpec::new(Sec::Row(i), Kind::Custom { height: item_list::ROW_H, focusable: true }, list_group(i)));
            }
        } else {
            out.push(SectionSpec::new(Sec::Items, Kind::Grid { spec: GridSpec::new(0.0, MARGIN_Y) }, ITEMS_GROUP));
        }
    }
    fn fallback(&self, _cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        out.extend([Sec::Items, Sec::Row(0), Sec::Head]);
    }
    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        (k == Sec::Items).then(|| ItemCards { items: continue_items(cx), cards: &self.cards })
    }
    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Head => Some(TOGGLE_ELEM),
            Sec::Row(i) => self.rows.get(i).copied(),
            Sec::Items | Sec::Status => None,
        }
    }
    fn element_rect(&self, cx: &Cx<'_, Bundle>, k: Sec, _n: usize, section: Rect) -> Rect {
        match k {
            Sec::Head => Self::toggle_rect(cx.measure, section.y),
            Sec::Row(_) => item_list::face_rect(section),
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
    /// Every card opens its item menu on a held OK, as on Home.
    fn card_has_menu(&self, _cx: &Cx<'_, Bundle>, _k: Sec, _elem: &u32) -> bool {
        true
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
                let count = msg::app_library_count(continue_items(f.cx).len() as i64);
                sn_ui::cards::draw_heading(p, msg::app_home_continue(), &count, MARGIN_X, r.y + HEADING_TOP, w, f.measure);
                let focused = f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == TOGGLE_ELEM);
                item_list::draw_toggle(p, chip, self.list, focused);
            }
            Sec::Row(i) => {
                let Some(item) = continue_items(f.cx).get(i).copied() else { return };
                let focused = f.focus.current.is_some_and(|key| key.entry == self.entry && Some(key.elem) == self.rows.get(i).copied());
                item_list::draw(f, r, &ListRow::of_library(item), focused);
            }
            Sec::Status => {
                StatusOverlay::new(Self::status_frame(), msg::app_grid_empty_c(), StatusKind::Empty).draw_measured(&Env::inert(), p, f.measure);
            }
            Sec::Items => {}
        }
    }
}

pub struct ContinueScreen {
    page: Page,
    stack: Stack<Sec>,
    seat: FirstSeat,
}

impl ContinueScreen {
    pub fn new(entry: EntryId) -> Self {
        ContinueScreen {
            page: Page { entry, cards: item_cards::Interned::with_base(CARD_BASE), list: false, rows: Vec::new() },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Library),
            seat: FirstSeat::new(),
        }
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    /// Intern every item and read the list setting; `true` when anything moved.
    fn sync(&mut self, cx: &Cx<'_, Bundle>) -> bool {
        let before = self.page.cards.len();
        let rows: Vec<u32> = continue_items(cx).iter().map(|i| self.page.cards.elem(&i.id)).collect();
        let list = cx.views.settings.list(ViewPage::Home);
        let rows = if list { rows } else { Vec::new() };
        let changed = self.page.cards.len() != before || list != self.page.list || rows != self.page.rows;
        self.page.list = list;
        self.page.rows = rows;
        changed
    }

    fn item<'a>(&self, cx: &Cx<'a, Bundle>, elem: u32) -> Option<&'a LibraryItem> {
        continue_items(cx).into_iter().find(|i| self.page.cards.get(&i.id) == Some(elem))
    }
}

impl LogicalState for ContinueScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.page.entry.0).u32(self.page.cards.len() as u32).bool(self.page.list);
        self.stack.write(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("continue cards={}", self.page.cards.len()));
    }
}

impl Machine<Bundle> for ContinueScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        let grew = self.sync(cx);
        let got = self.stack.on(&self.page, ev, cx, fx);
        let entry = self.page.entry;
        match ev {
            ScreenEvent::Tick(_) => {
                let group = if self.page.list { list_group(0) } else { ITEMS_GROUP };
                let target = (!continue_items(cx).is_empty()).then_some(sn_ui::screen::FocusTarget::FirstInGroup(group));
                self.seat.tick(cx.focus.current.is_some_and(|k| k.entry == entry), target, fx);
                if grew {
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _)
                if [StoreId::Library.ord(), StoreId::Account.ord(), StoreId::Settings.ord()].contains(ord) =>
            {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) | ScreenEvent::Activate(_) => {
                // a card reports its press; a list row (a plain element) is named by focus
                let elem = match (&got, ev) {
                    (Some(StackEvent::Card(_, CardEvent::Activate(e))), _) | (_, ScreenEvent::Activate(e)) => Some(*e),
                    (Some(StackEvent::Press(Sec::Head)), _) => Some(TOGGLE_ELEM),
                    (None, ScreenEvent::PressCommit(_)) => cx.focus.current.filter(|k| k.entry == entry).map(|k| k.elem),
                    _ => None,
                };
                match elem {
                    Some(TOGGLE_ELEM) => fx.push(Fx::App(AppFx::Settings(SettingsCmd::List(ViewPage::Home, !self.page.list)))),
                    Some(e) => {
                        if let Some(item) = self.item(cx, e) {
                            fx.push(Fx::Nav(NavOp::Push(item_cards::resume_arg(item, msg::app_home_continue()))));
                        }
                    }
                    None => {}
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                let held = match got {
                    Some(StackEvent::Card(_, CardEvent::Hold(elem))) => Some((elem, self.stack.focused_card(&self.page, cx, At::Drawn).map(|(_, at)| at.rect))),
                    _ => self.stack.focused_plain(&self.page, cx).and_then(|(k, rect)| match k {
                        Sec::Row(i) => self.page.rows.get(i).map(|e| (*e, Some(rect))),
                        _ => None,
                    }),
                };
                if let Some((elem, at)) = held {
                    if let Some(item) = self.item(cx, elem) {
                        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
                            host: entry,
                            reply: fx.from(),
                            focus: cx.focus.current,
                            anchor: MenuArg::anchor_of(at.unwrap_or_else(crate::menu::fallback_anchor)),
                            kind: item_cards::item_menu(item, msg::app_home_continue()),
                        }))));
                    }
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
            // YELLOW opened this page from a row (Home's See all); YELLOW closes it again
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { wcode: crate::tabs::WCODE_YELLOW, edge, .. }, .. }) => {
                if *edge == Edge::Down {
                    fx.push(Fx::Nav(NavOp::Pop));
                }
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

sn_ui::focusable_via_view!(ContinueScreen, Bundle, view);

impl Screen<Bundle> for ContinueScreen {
    fn name(&self) -> &'static str {
        word::CONTINUE
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
