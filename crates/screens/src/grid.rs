//! **One catalog, whole** — the full paginated grid a search row's MORE opens: the row's addon and
//! catalog under the same query, every page the addon holds, loaded as the grid reaches its end.
//!
//! A `cards::Stack` of a heading, the grid, and while there are no cards the read-out for why:
//! loading (the spinner), nothing, a failure with its reason and a Retry, or an addon that is not
//! installed any more. OK on a card opens its Detail page; a held OK its item menu. The view is
//! held open in the grid store for as long as the page lives ([`GridCmd::Open`] / `Close`), and a
//! search row's grid opens on the cards the row already holds.

use std::borrow::Cow;

use sn_machine::machine::{Canon, Cx, Effects, EntryId, Fx, GroupId, Handled, Leave, LogicalState, Machine, NavOp};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_ui::cards::{CardEvent, GridSpec, Kind, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{MARGIN_X, MARGIN_Y, SCR_W};
use sn_ui::screen::{At, DrawFrame, Focusable, RenderStrategy, Screen, ScreenEvent};
use sn_ui::widgets::{StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect};

use crate::cards::{item_ref, ItemCards};
use crate::registry::{kind_label, word, AppArg, AppFx, Bundle, CatalogRef, FirstSeat, GridCmd, MenuArg, MenuKind};
use crate::stores::grid::{GridKey, GridState, GridStore};
use crate::stores::StoreId;

const CARD_BASE: u32 = 0x1000;
const RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 1;
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
}

/// The read-out a grid shows in place of its cards, if any.
pub fn readout(state: &GridState, frame: Rect) -> Option<StatusOverlay<'static>> {
    Some(match state {
        GridState::Ready { .. } => return None,
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
}

impl Page {
    fn state(&self, store: &GridStore) -> GridState {
        store.state(&self.key).unwrap_or(GridState::Loading)
    }
    fn status_frame() -> Rect {
        Rect::new(0.0, HEAD_H, SCR_W, sn_ui::consts::SCR_H - HEAD_H)
    }
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(cx.views.grid.gen()) << 32) | cx.views.grid.items(&self.key).len() as u64
    }
    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        out.push(SectionSpec::new(Sec::Head, Kind::Custom { height: HEAD_H, focusable: false }, HEAD_GROUP));
        let state = self.state(cx.views.grid);
        if let Some(o) = readout(&state, Self::status_frame()) {
            let failed = matches!(state, GridState::Failed(_));
            let rect = if failed { o.action_frame_measured(cx.measure).unwrap_or(Self::status_frame()) } else { Self::status_frame() };
            out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: failed }, STATUS_GROUP).ranked(1));
        }
        out.push(SectionSpec::new(Sec::Items, Kind::Grid { spec: GridSpec::new(0.0, MARGIN_Y) }, ITEMS_GROUP));
    }
    fn fallback(&self, _cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        out.extend([Sec::Items, Sec::Status]);
    }
    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        (k == Sec::Items).then(|| ItemCards {
            items: cx.views.grid.items(&self.key),
            base: CARD_BASE,
            more: matches!(self.state(cx.views.grid), GridState::Ready { more: true }),
        })
    }
    fn elem_of(&self, k: Sec) -> Option<u32> {
        (k == Sec::Status).then_some(RETRY_ELEM)
    }
    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        let p = f.painter.alpha(f.page_alpha);
        match k {
            Sec::Head => {
                sn_ui::cards::draw_heading(p, &self.title, &self.note, MARGIN_X, r.y + HEADING_TOP, SCR_W - 2.0 * MARGIN_X, f.measure);
            }
            Sec::Status => {
                let state = self.state(f.cx.views.grid);
                if let Some(o) = readout(&state, Self::status_frame()) {
                    let focused = f.focus.current.is_some_and(|key| key.elem == RETRY_ELEM);
                    o.phase(f.cx.tick.ms).focused(focused).draw_measured(&Env::inert(), p, f.measure);
                }
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
            page: Page { key: r.key, title: r.title, note },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Search),
            seat: FirstSeat::new(),
            open: false,
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
        let got = self.stack.on(&self.page, ev, cx, fx);
        let cards = |cx: &Cx<'_, Bundle>| cx.views.grid.items(&self.page.key).len();
        match ev {
            ScreenEvent::Tick(_) => {
                if let Some(StackEvent::Card(Sec::Items, CardEvent::Want(range))) = &got {
                    if range.end > cards(cx) {
                        fx.push(Fx::App(AppFx::Grid(GridCmd::More(self.page.key.clone()))));
                    }
                }
                let target = (cards(cx) > 0).then_some(sn_ui::screen::FocusTarget::FirstInGroup(ITEMS_GROUP)).or_else(|| {
                    matches!(self.page.state(cx.views.grid), GridState::Failed(_))
                        .then_some(sn_ui::screen::FocusTarget::FirstInGroup(STATUS_GROUP))
                });
                self.seat.tick(cx.focus.current.is_some_and(|k| k.entry == self.entry), target, fx);
                if self.page.state(cx.views.grid) == GridState::Loading {
                    fx.note(sn_machine::present::PresentEvent::Motion);
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Grid.ord() => {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) | ScreenEvent::Activate(_) => {
                match (&got, ev) {
                    (Some(StackEvent::Card(_, CardEvent::Activate(elem))), _) | (_, ScreenEvent::Activate(elem))
                        if *elem >= CARD_BASE && *elem < RETRY_ELEM =>
                    {
                        let items = ItemCards { items: cx.views.grid.items(&self.page.key), base: CARD_BASE, more: false };
                        if let Some(item) = items.item(*elem) {
                            fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(item)))));
                        }
                    }
                    (Some(StackEvent::Press(Sec::Status)), _) | (_, ScreenEvent::Activate(RETRY_ELEM)) => {
                        fx.push(Fx::App(AppFx::Grid(GridCmd::Retry(self.page.key.clone()))));
                    }
                    _ => {}
                }
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Hold(elem))) = got {
                    let items = ItemCards { items: cx.views.grid.items(&self.page.key), base: CARD_BASE, more: false };
                    if let Some(item) = items.item(elem) {
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
