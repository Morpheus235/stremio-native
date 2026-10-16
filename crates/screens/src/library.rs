//! **Library** — the signed-in account's library, a shelf per type (movies, series, then the
//! rest, `stores::library::TYPE_ORDER`), most recently changed first, each poster carrying its
//! resume bar when it is in progress. OK opens the item's Detail page.
//!
//! The page is a `sn_ui::cards::Stack` like Home: a status section (signed out, loading, failed,
//! empty — the shared `StatusOverlay` read-out, at the shared placement) and one shelf per type.
//! Signed out, the read-out's action opens the sign-in page; a failed load's action asks again.
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
use sn_stremio::library::LibraryItem;
use sn_ui::cards::{CardEvent, Kind, RowStyle, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{MARGIN_X, SCR_W};
use sn_ui::screen::{DrawFrame, FocusSource, FocusTarget, Focusable, HitSource, Link, RenderStrategy, Screen, ScreenEvent};
use sn_ui::widgets::{StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect};

use crate::item_cards::{Interned, ItemCards};
use crate::registry::{kind_label, word, AppArg, AppFx, Bundle, FirstSeat, ItemRef, LibraryCmd};
use crate::stores::account::SessionState;
use crate::stores::library::{LibraryState, LibraryStore};
use crate::stores::StoreId;
use crate::tabs::Tab;

const FIRST_CARD_ELEM: u32 = 0x1000;
const ACTION_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 2;
const STATUS_GROUP: GroupId = GroupId(0);
/// The shelves start under the top bar (Home's placement).
const CONTENT_TOP: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
const HEAD_TO_ROW: f32 = 64.0;
const HEADING_TOP: f32 = 10.0;
static POSTER: RowStyle = RowStyle::HOME;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    Status,
    /// The shelf of the store's `by_type()` group at this position.
    Type(usize),
}

struct Page {
    entry: EntryId,
    cards: Interned,
}

fn type_group(i: usize) -> GroupId {
    GroupId(1 + i as u32)
}

fn status(cx: &Cx<'_, Bundle>) -> Option<StatusKind> {
    match cx.views.account.session() {
        SessionState::Restoring => return Some(StatusKind::Working),
        SessionState::SignedOut { .. } => return Some(StatusKind::Failed),
        SessionState::SignedIn(_) => {}
    }
    match cx.views.library.state() {
        LibraryState::None | LibraryState::Loading => Some(StatusKind::Working),
        LibraryState::Failed(_) => Some(StatusKind::Failed),
        LibraryState::Ready(_) if cx.views.library.by_type().is_empty() => Some(StatusKind::Empty),
        LibraryState::Ready(_) => None,
    }
}

fn overlay<'a>(cx: &Cx<'_, Bundle>, ms: u32) -> Option<StatusOverlay<'a>> {
    let kind = status(cx)?;
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
        _ => StatusOverlay::new(Rect::FULL, msg::app_library_empty_c(), StatusKind::Empty),
    })
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(cx.views.library.gen()) << 32) | (u64::from(cx.views.account.gen()) << 16) | self.cards.len() as u64
    }

    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        match overlay(cx, 0).filter(|_| status(cx) == Some(StatusKind::Failed)) {
            Some(o) => {
                let rect = o.action_frame_measured(cx.measure).unwrap_or(Rect::FULL);
                out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: true }, STATUS_GROUP));
            }
            None => out.push(SectionSpec::new(Sec::Status, Kind::Custom { height: CONTENT_TOP, focusable: false }, STATUS_GROUP)),
        }
        if status(cx).is_some() {
            return;
        }
        for (i, _) in cx.views.library.by_type().iter().enumerate() {
            out.push(SectionSpec::new(Sec::Type(i), Kind::Shelf { style: &POSTER, heading: HEAD_TO_ROW }, type_group(i)).ranked(i as u32));
        }
    }

    fn fallback(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        out.extend((0..cx.views.library.by_type().len()).map(Sec::Type));
        out.push(Sec::Status);
    }

    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        let Sec::Type(i) = k else { return None };
        let (_, items) = cx.views.library.by_type().into_iter().nth(i)?;
        Some(ItemCards { items, cards: &self.cards })
    }

    fn elem_of(&self, k: Sec) -> Option<u32> {
        (k == Sec::Status).then_some(ACTION_ELEM)
    }

    fn card_has_menu(&self, _cx: &Cx<'_, Bundle>, _k: Sec, _elem: &u32) -> bool {
        false
    }

    /// A shelf scrolled back into view stops under the top bar.
    fn reveal_margin(&self, _k: Sec) -> f32 {
        CONTENT_TOP
    }

    fn draw_heading(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, lift: f32) {
        let Sec::Type(i) = k else { return };
        let Some((kind, items)) = f.cx.views.library.by_type().into_iter().nth(i) else { return };
        let p = f.painter.alpha(f.page_alpha);
        let title = kind_label(&kind).map_or_else(|| kind.clone(), str::to_string);
        let count = msg::app_library_count(items.len() as i64);
        sn_ui::cards::draw_heading(p, &title, &count, MARGIN_X, r.y + HEADING_TOP - lift, SCR_W - 2.0 * MARGIN_X, f.measure);
    }

    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, _r: Rect, _scroll: f32) {
        if k != Sec::Status {
            return;
        }
        if let Some(o) = overlay(f.cx, f.cx.tick.ms) {
            let focused = f.focus.current.is_some_and(|key| key.entry == self.entry && key.elem == ACTION_ELEM);
            o.focused(focused).draw_measured(&Env::inert(), f.painter.alpha(f.page_alpha), f.measure);
        }
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
        LibraryScreen { page: Page { entry, cards: Interned::with_base(FIRST_CARD_ELEM) }, stack: Stack::new(entry), seat: FirstSeat::new(), chose: false, first: None }
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    fn intern(&mut self, store: &LibraryStore) -> bool {
        let before = self.page.cards.len();
        for (_, items) in store.by_type() {
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
            fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(ItemRef { kind: item.kind.clone(), id: item.id.clone(), name: item.name.clone() }))));
        }
    }

    /// The page's first focusable group: the first shelf, or a failed read-out's action.
    fn first_group(cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        match status(cx) {
            None => Some(type_group(0)),
            Some(StatusKind::Failed) => Some(STATUS_GROUP),
            _ => None,
        }
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
            ScreenEvent::Tick(_) | ScreenEvent::Mount | ScreenEvent::StoreChanged(..) => self.intern(cx.views.library),
            _ => false,
        };
        let got = self.stack.on(&self.page, ev, cx, fx);
        match ev {
            ScreenEvent::Mount => {
                fx.push(Fx::App(AppFx::Library(LibraryCmd::Reload)));
                Handled::Yes
            }
            ScreenEvent::Tick(_) => {
                let first = Self::first_group(cx);
                let seated = crate::tabs::rests(self.page.entry, Tab::Library, cx, self.chose, first.is_some());
                let target = first.map(FocusTarget::FirstInGroup).unwrap_or(FocusTarget::Elem(FocusKey {
                    entry: self.page.entry,
                    elem: Tab::Library.elem(),
                }));
                self.seat.tick(seated, Some(target), fx);
                self.first = first;
                if grew {
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Library.ord() || *ord == StoreId::Account.ord() => {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Activate(elem))) = got {
                    self.open(cx, elem, fx);
                } else if let Some(k) = cx.focus.current.filter(|k| k.entry == self.page.entry && crate::tabs::on_strip(k.elem)) {
                    crate::tabs::on_pill(k.elem, Tab::Library, Self::first_group(cx), fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => {
                self.chose = true;
                crate::tabs::on_back(self.page.entry, Tab::Library, cx, fx)
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, Tab::Library, Self::first_group(cx), fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if *elem == ACTION_ELEM => {
                if matches!(cx.views.account.session(), SessionState::SignedOut { .. }) {
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
