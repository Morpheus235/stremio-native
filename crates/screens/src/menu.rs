//! **The popover menu** — a `table.rs` table on a panel anchored BESIDE the control it was opened
//! from (PlxNative's item menu: to the right of the card, flipped left at the screen's keep-out),
//! presented as a modal surface (`NavOp::Present`) over the live page with the compact dim.
//!
//! Two contents ([`MenuKind`]):
//!
//! - **a card's item menu** (a held OK on any catalog card): *Open*, and *Add to library* — the
//!   account's real library write (`LibraryCmd::Add`, stremio-core's `datastorePut`). Disabled,
//!   with the reason under it, while signed out ([`SessionView`]) or while the write is on its
//!   way; *In your library* (disabled) once the account holds it; after a failed write it says
//!   so and is offered again ([`AddState`]);
//! - **a choice** (the Live TV page's catalog and genre chips): one row per option, the current
//!   one checked; OK sends the pick back to the presenting page as [`AppMsg::Chose`].
//!
//! OK on a row acts and dismisses; BACK dismisses. Rows are focused by identity ([`MenuRow`]).

use std::borrow::Cow;
use std::convert::Infallible;

use sn_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind, Key, LogicalState, Machine,
    NavOp,
};
use sn_platform::i18n::msg;
use sn_ui::consts::{CARD_H, CARD_W, MARGIN_X, SCR_H, SCR_W};
use sn_ui::form::{Activation, Form, FormId, FormSection, FormTable, RowKey, RowKind};
use sn_ui::screen::{
    Activate, At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Focusable, GroupKind, GroupSpec, Hover, Placed,
    RenderStrategy, Scrim, Screen, ScreenEvent, Seat, Step, Stop,
};
use sn_ui::table::{Row, MENU_MAX_W, MENU_MIN_W};
use sn_ui::{theme, Rect};

use crate::registry::{word, AppArg, AppFx, AppMsg, Bundle, ItemRef, LibraryCmd, MenuArg, MenuKind, SessionView};
use crate::stores::library::AddState;

const PANEL_RAD: f32 = 20.0;
/// The air between the opener and the panel.
const CARD_GAP: f32 = theme::space::MD;
/// The panel's keep-out from the top and bottom of the screen.
const EDGE: f32 = theme::space::XL;
const EDGE_X: f32 = MARGIN_X;
/// Every row key is below this.
const KEY_BAND: u32 = 0x1000;

/// A menu row's identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRow {
    Open,
    AddToLibrary,
    Option(usize),
}

impl FormId for MenuRow {
    fn key(&self) -> RowKey {
        RowKey(match self {
            MenuRow::Open => 1,
            MenuRow::AddToLibrary => 2,
            MenuRow::Option(i) => 0x10 + (*i as u32).min(KEY_BAND - 0x11),
        })
    }
}

type MenuForm = Form<MenuRow, MenuRow, Infallible>;

/// The rows for `kind`. Pure, so the fit tests build the real menu.
pub fn build(kind: &MenuKind, session: SessionView, add: AddState) -> MenuForm {
    match kind {
        MenuKind::Item { .. } => {
            let (lib, disabled) = match (session.signed_in, add) {
                (false, _) => (Row::new(msg::app_menu_add_to_library()).detail(msg::app_menu_sign_in_first()), true),
                (true, AddState::In) => (Row::new(msg::app_menu_in_library()), true),
                (true, AddState::Adding) => (Row::new(msg::app_menu_add_to_library()).detail(msg::app_menu_adding()), true),
                (true, AddState::Failed) => (Row::new(msg::app_menu_add_to_library()).detail(msg::app_menu_add_failed()), false),
                (true, AddState::Absent) => (Row::new(msg::app_menu_add_to_library()), false),
            };
            Form::new().section(
                FormSection::new("")
                    .item(MenuRow::Open, RowKind::Button, MenuRow::Open, Row::new(msg::app_menu_open()))
                    .item(MenuRow::AddToLibrary, RowKind::Button, MenuRow::AddToLibrary, lib)
                    .disabled(disabled),
            )
        }
        MenuKind::Choice { title, options, current, .. } => {
            let mut s = FormSection::from_head(sn_ui::table::Section::new(title.clone()).server_header());
            for (i, o) in options.iter().enumerate() {
                s = s.choice(MenuRow::Option(i), MenuRow::Option(i), Row::new(o.clone()).server_label(), |id| *id == MenuRow::Option(*current));
            }
            Form::new().section(s)
        }
    }
}

/// Beside the opener, never over it: to its right, flipped to its left when that would cross the
/// keep-out; hanging off its top edge, pulled back inside the safe band.
pub fn panel_at(a: Rect, content_w: f32, content_h: f32) -> Rect {
    let width = content_w.clamp(MENU_MIN_W, MENU_MAX_W);
    let h = content_h.clamp(120.0, SCR_H - 2.0 * EDGE);
    let right = a.x + a.w + CARD_GAP;
    let x = if right + width <= SCR_W - EDGE_X { right } else { a.x - CARD_GAP - width };
    let x = x.clamp(EDGE_X, (SCR_W - EDGE_X - width).max(EDGE_X));
    let y = a.y.clamp(EDGE, (SCR_H - EDGE - h).max(EDGE));
    Rect::new(x, y, width, h)
}

/// The anchor for a menu with nothing to hang off: a centred card.
pub fn fallback_anchor() -> Rect {
    Rect::new((SCR_W - CARD_W) * 0.5 - MENU_MIN_W * 0.5, (SCR_H - CARD_H) * 0.5, CARD_W, CARD_H)
}

pub struct MenuScreen {
    entry: EntryId,
    arg: MenuArg,
    form: FormTable<MenuRow, MenuRow, Infallible>,
}

impl MenuScreen {
    pub fn new(entry: EntryId, arg: MenuArg, session: SessionView, add: AddState) -> Self {
        let mut form = FormTable::new(KEY_BAND);
        form.table.compact = true;
        let initial = match &arg.kind {
            MenuKind::Choice { current, .. } => Some(MenuRow::Option(*current)),
            MenuKind::Item { .. } => None,
        };
        form.set(build(&arg.kind, session, add), initial.as_ref());
        MenuScreen { entry, arg, form }
    }

    pub fn kind(&self) -> &MenuKind {
        &self.arg.kind
    }

    /// The rows as the tests read them.
    pub fn table(&self) -> &sn_ui::table::TableView {
        &self.form.table
    }

    fn frame(&self, measure: &dyn sn_machine::machine::Measure) -> Rect {
        panel_at(self.arg.anchor_rect(), self.form.table.measured_width(measure), self.form.table.measured_height())
    }

    fn opening(&self) -> u32 {
        self.form.opening_key().map_or(0, |k| k.0)
    }

    fn focusable(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.form.table.n_rows() as usize).filter_map(|i| self.form.key_at(i).map(|k| k.0))
    }

    /// OK on a row: act, then dismiss. A disabled row does neither.
    fn activate(&mut self, elem: u32, fx: &mut Effects<'_, Bundle>) {
        let Some(i) = self.form.index_of_key(RowKey(elem)) else { return };
        let Some(Activation::Action(row)) = self.form.activate(i) else { return };
        fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
        match (&self.arg.kind, row) {
            (MenuKind::Item { item, .. }, MenuRow::Open) => fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item.clone())))),
            (MenuKind::Item { item, poster }, MenuRow::AddToLibrary) => {
                let entry = sn_stremio::library::NewEntry {
                    id: item.id.clone(),
                    kind: item.kind.clone(),
                    name: item.name.clone(),
                    poster: poster.clone(),
                };
                fx.push(Fx::App(AppFx::Library(LibraryCmd::Add(entry))));
            }
            (MenuKind::Choice { target, .. }, MenuRow::Option(i)) => {
                fx.push(Fx::Deliver(self.arg.reply, Delivery::Screen(ScreenEvent::App(AppMsg::Chose(*target, i)))));
            }
            _ => {}
        }
    }
}

impl Machine<Bundle> for MenuScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Mount => sn_ui::hold_hint::mark_learned(),
            ScreenEvent::Tick(t) => {
                if let Some(i) = cx.focus.current.filter(|k| k.entry == self.entry).and_then(|k| self.form.index_of_key(RowKey(k.elem))) {
                    self.form.table.sel = i as i32;
                }
                self.form.table.update(t.dt(), self.frame(cx.measure).h);
            }
            ScreenEvent::FocusMoved { to, .. } => {
                if let Some(i) = self.form.index_of_key(RowKey(to.elem)) {
                    self.form.table.sel = i as i32;
                }
                fx.invalidate(sn_machine::present::Provenance::Input);
            }
            ScreenEvent::Activate(elem) => self.activate(*elem, fx),
            ScreenEvent::PressCommit(_) => {
                if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry) {
                    self.activate(k.elem, fx);
                }
            }
            ScreenEvent::Input(input) => {
                if let InputKind::Key { key: Key::Back, edge: Edge::Down, .. } = input.kind {
                    fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                    return Handled::Yes;
                }
                return Handled::No;
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }
}

impl Focusable<Bundle> for MenuScreen {
    fn groups(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<GroupSpec>) {
        out.push(GroupSpec {
            id: GroupId(0),
            kind: GroupKind::Column,
            seat: Seat::Remembered,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Stop; 4],
            extent: self.frame(cx.measure),
            len: self.focusable().count(),
            elem: ElemKind::Bare,
        });
    }
    fn group_of(&self, elem: &u32, _: &Cx<'_, Bundle>) -> Option<GroupId> {
        self.form.index_of_key(RowKey(*elem)).map(|_| GroupId(0))
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, _: &Cx<'_, Bundle>) -> Step<u32> {
        let delta = match dir {
            Dir::Up => -1,
            Dir::Down => 1,
            _ => return Step::Edge,
        };
        match self.form.step_key(RowKey(key.elem), delta) {
            Some(k) => Step::Move(FocusKey { entry: self.entry, elem: k.0 }),
            None => Step::Edge,
        }
    }
    fn place(&self, elem: &u32, cx: &Cx<'_, Bundle>, _: At) -> Option<Placed> {
        let index = self.form.index_of_key(RowKey(*elem))?;
        let frame = self.frame(cx.measure);
        let rect = self.form.table.row_frame(frame, index as i32)?;
        Some(Placed { rect, rest_rect: rect, clip: frame, index: Some(index as u32) })
    }
    fn reconcile(&self, want: FocusKey<u32>, _: &Cx<'_, Bundle>) -> FocusKey<u32> {
        if self.form.index_of_key(RowKey(want.elem)).is_some() {
            want
        } else {
            FocusKey { entry: self.entry, elem: self.opening() }
        }
    }
    fn seat(&self, _: GroupId, _: Placed, _: &Cx<'_, Bundle>) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem: self.form.selected_key().map_or_else(|| self.opening(), |k| k.0) }
    }
}

impl Screen<Bundle> for MenuScreen {
    fn name(&self) -> &'static str {
        word::MENU
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
    }
    fn opener(&self) -> Option<(EntryId, Option<FocusKey<u32>>)> {
        Some((self.arg.host, self.arg.focus))
    }
    /// The lightest dim: the card and its shelf stay legible behind the panel.
    fn scrim(&self) -> Scrim {
        Scrim::dim(theme::underlay::DIM_COMPACT)
    }
    fn prepare(&mut self, _: &mut sn_ui::frame::Budget, _: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        let p = f.painter.alpha(f.page_alpha);
        let r = self.frame(f.measure);
        sn_ui::widgets::panel_ground(p, r, PANEL_RAD, f.underlay);
        self.form.table.draw(p, r, f.measure);
        for elem in self.focusable().collect::<Vec<_>>() {
            if let Some(placed) = self.place(&elem, f.cx, At::Drawn) {
                f.stop(
                    p,
                    Stop {
                        key: FocusKey { entry: self.entry, elem },
                        rect: placed.rect,
                        rest_rect: placed.rest_rect,
                        clip: placed.clip,
                        hover: Hover::Focus,
                        activate: Activate::Immediate,
                    },
                );
            }
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

impl LogicalState for MenuScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.form.table.sel as u32);
        self.form.table.write_motion(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("menu rows={} sel={}", self.form.table.n_rows(), self.form.table.sel));
    }
}

/// The item a menu was opened on, for the tests.
pub fn item_of(kind: &MenuKind) -> Option<&ItemRef> {
    match kind {
        MenuKind::Item { item, .. } => Some(item),
        MenuKind::Choice { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_hangs_beside_its_opener_and_flips_at_the_edge() {
        let left = Rect::new(200.0, 400.0, CARD_W, CARD_H);
        let p = panel_at(left, 400.0, 200.0);
        assert!(p.x >= left.x + left.w, "to the right of the card");
        let right = Rect::new(SCR_W - MARGIN_X - CARD_W, 400.0, CARD_W, CARD_H);
        let p = panel_at(right, 400.0, 200.0);
        assert!(p.x + p.w <= right.x, "flipped to its left at the screen's edge");
        let low = Rect::new(200.0, SCR_H - 100.0, CARD_W, CARD_H);
        assert!(panel_at(low, 400.0, 300.0).y + 300.0 <= SCR_H - EDGE, "pulled back inside the safe band");
    }

    #[test]
    fn the_item_menu_fits_its_panel_in_every_shipped_language() {
        use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
        let item = MenuKind::Item { item: ItemRef { kind: "movie".into(), id: "tt1".into(), name: "Film".into() }, poster: None };
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            for (signed_in, add) in [
                (false, AddState::Absent),
                (true, AddState::Absent),
                (true, AddState::Adding),
                (true, AddState::Failed),
                (true, AddState::In),
            ] {
                let mut t = FormTable::<MenuRow, MenuRow, Infallible>::new(KEY_BAND);
                t.table.compact = true;
                t.set(build(&item, SessionView { signed_in }, add), None);
                out.extend(t.table.app_fit_failures_hugged(&format!("{} signed_in={signed_in} {add:?}", language.tag())));
                if let Some(why) = t.table.menu_cap_failure(&sn_base::fontcov::advances::ShippedMeasure, "item menu") {
                    out.push(why);
                }
            }
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }

    #[test]
    fn add_to_library_is_disabled_until_signed_in() {
        let item = MenuKind::Item { item: ItemRef { kind: "movie".into(), id: "tt1".into(), name: "Film".into() }, poster: None };
        let mut t = FormTable::<MenuRow, MenuRow, Infallible>::new(KEY_BAND);
        t.set(build(&item, SessionView { signed_in: false }, AddState::Absent), None);
        assert_eq!(t.activate(0), Some(Activation::Action(MenuRow::Open)));
        assert_eq!(t.activate(1), None, "disabled while signed out");
        assert!(t.table.sections[0].rows[1].dim);
        let signed_in = SessionView { signed_in: true };
        t.set(build(&item, signed_in, AddState::Absent), None);
        assert_eq!(t.activate(1), Some(Activation::Action(MenuRow::AddToLibrary)));
        t.set(build(&item, signed_in, AddState::Failed), None);
        assert_eq!(t.activate(1), Some(Activation::Action(MenuRow::AddToLibrary)), "a failed write is offered again");
        assert_eq!(t.table.sections[0].rows[1].detail, msg::app_menu_add_failed());
        for add in [AddState::Adding, AddState::In] {
            t.set(build(&item, signed_in, add), None);
            assert_eq!(t.activate(1), None, "{add:?}: nothing to press");
        }
        assert_eq!(t.table.sections[0].rows[1].label, msg::app_menu_in_library());
    }
}
