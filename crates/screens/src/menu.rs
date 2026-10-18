//! **The popover menu** — a `table.rs` table on a panel anchored BESIDE the control it was opened
//! from (PlxNative's item menu: to the right of the card, flipped left at the screen's keep-out),
//! presented as a modal surface (`NavOp::Present`) over the live page with the compact dim.
//!
//! Two contents ([`MenuKind`]):
//!
//! - **a card's item menu** (a held OK on any catalog card): *Open*, then *Add to library* or,
//!   once the account holds it, *Remove from library* — the account's real library writes
//!   (`LibraryCmd::Add` / `Remove`, stremio-core's `AddToLibrary` / `RemoveFromLibrary`). Disabled,
//!   with the reason under it, while signed out ([`SessionView`]) or while a write for the item is
//!   on its way ([`AddState`]). A title in Continue Watching also offers *Remove from Continue
//!   Watching* (`LibraryCmd::Rewind`, core's `RewindLibraryItem`), whichever card it was held on.
//!   A movie also offers *Mark as Watched* or, once its entry is watched (`timesWatched > 0`),
//!   *Mark as Unwatched* (`LibraryCmd::Mark`, core's `LibraryItemMarkAsWatched` /
//!   `MetaItemMarkAsWatched`), signed in only.
//!   A refused write is the application's toast; the library store has already rolled it back;
//! - **a choice** (the player's audio and subtitle
//!   tracks and the subtitle size/position ladders): one row per option, the current one checked;
//!   OK sends the pick back to the presenting page as [`AppMsg::Chose`];
//! - **a list** (the player's `…` menu, the subtitle delay steps): labelled rows with a trailing value,
//!   OK sends the row back the same way.
//!
//! A player menu rises from the transport's control row, right-aligned on its edge, rather than
//! hanging beside a card (every choice and list menu is the player's, [`player_panel_at`]).
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

use crate::registry::{word, AppArg, AppFx, AppMsg, Bundle, ChoiceTarget, ItemRef, LibraryCmd, MenuArg, MenuKind, SessionView};
use crate::stores::account::AccountCmd;
use crate::stores::library::AddState;

const PANEL_RAD: f32 = theme::radius::PANEL;
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
    /// Add to library, or Remove from library once it is in it.
    AddToLibrary,
    RemoveFromContinue,
    /// Mark as Watched, or Mark as Unwatched once it is.
    Watched,
    Option(usize),
    /// The account menu's rows ([`account_form`]).
    Settings,
    Addons,
    /// The account line: who is signed in. It reads; OK on it does nothing.
    Account,
    SignIn,
    SignOut,
    /// The sign-out confirmation's two answers.
    Cancel,
    ConfirmSignOut,
}

impl FormId for MenuRow {
    fn key(&self) -> RowKey {
        RowKey(match self {
            MenuRow::Open => 1,
            MenuRow::AddToLibrary => 2,
            MenuRow::RemoveFromContinue => 3,
            MenuRow::Settings => 4,
            MenuRow::Addons => 5,
            MenuRow::Account => 6,
            MenuRow::SignIn => 7,
            MenuRow::SignOut => 8,
            MenuRow::Cancel => 9,
            MenuRow::ConfirmSignOut => 10,
            MenuRow::Watched => 11,
            MenuRow::Option(i) => 0x10 + (*i as u32).min(KEY_BAND - 0x11),
        })
    }
}

type MenuForm = Form<MenuRow, MenuRow, Infallible>;

/// The rows for `kind`. Pure, so the fit tests build the real menu.
pub fn build(kind: &MenuKind, session: SessionView, add: AddState) -> MenuForm {
    build_with(kind, session, add, false)
}

/// [`build`], for an item `continuing` in Continue Watching (its *Remove from Continue Watching*).
pub fn build_with(kind: &MenuKind, session: SessionView, add: AddState, continuing: bool) -> MenuForm {
    build_item(kind, session, add, continuing, None)
}

/// [`build_with`], for a movie whose watched state is `watched` (`None`: no watched row).
pub fn build_item(kind: &MenuKind, session: SessionView, add: AddState, continuing: bool, watched: Option<bool>) -> MenuForm {
    match kind {
        MenuKind::Item { .. } => {
            let (lib, disabled) = match (session.signed_in, add) {
                // signed out the row is the way in: a library needs an account
                (false, _) => (Row::new(msg::app_menu_sign_in_to_add()), false),
                (true, AddState::In) => (Row::new(msg::app_menu_remove_from_library()), false),
                (true, AddState::Adding) => (Row::new(msg::app_menu_add_to_library()).detail(msg::app_menu_adding()), true),
                (true, AddState::Removing) => (Row::new(msg::app_menu_remove_from_library()).detail(msg::app_menu_removing()), true),
                (true, AddState::Absent) => (Row::new(msg::app_menu_add_to_library()), false),
            };
            let lib_row = if session.signed_in { MenuRow::AddToLibrary } else { MenuRow::SignIn };
            let mut s = FormSection::new("")
                .item(MenuRow::Open, RowKind::Button, MenuRow::Open, Row::new(msg::app_menu_open()))
                .item(lib_row.clone(), RowKind::Button, lib_row, lib)
                .disabled(disabled);
            // signed out there is no Continue Watching to leave
            if continuing && session.signed_in {
                let row = Row::new(msg::app_menu_remove_from_continue());
                s = s.item(MenuRow::RemoveFromContinue, RowKind::Button, MenuRow::RemoveFromContinue, row);
            }
            if let Some(w) = watched.filter(|_| session.signed_in) {
                let row = Row::new(if w { sn_ui::widgets::mark_unwatched_verb() } else { sn_ui::widgets::mark_watched_verb() });
                s = s.item(MenuRow::Watched, RowKind::Button, MenuRow::Watched, row);
            }
            Form::new().section(s)
        }
        MenuKind::Choice { title, options, current, .. } => {
            let mut s = FormSection::from_head(sn_ui::table::Section::new(title.clone()).server_header());
            for (i, o) in options.iter().enumerate() {
                s = s.choice(MenuRow::Option(i), MenuRow::Option(i), Row::new(o.clone()).server_label(), |id| *id == MenuRow::Option(*current));
            }
            Form::new().section(s)
        }
        // its own screen (`crate::player::subtitle_menu`); never built here
        MenuKind::Subtitles | MenuKind::Player(_) => Form::new(),
        // the address is the screen's (`MenuScreen::with_account`), read when it mounts
        MenuKind::Account => account_form(session.signed_in.then_some(""), false),
        MenuKind::List { title, rows, .. } => {
            let mut s = FormSection::new(title.clone());
            for (i, r) in rows.iter().enumerate() {
                if let Some((_, on)) = &r.switch {
                    s = s.choice(MenuRow::Option(i), MenuRow::Option(i), Row::new(r.label.clone()).server_label(), |_| *on);
                    continue;
                }
                let mut row = Row::new(r.label.clone()).value(r.value.clone()).chevron(r.opens);
                if r.server_value {
                    row = row.server_value();
                }
                s = s.item(MenuRow::Option(i), RowKind::Button, MenuRow::Option(i), row);
            }
            Form::new().section(s)
        }
    }
}

/// **The account menu's rows** (the top bar's account pill): *Settings*, *Addons*, then signed in
/// the account line (`email`, the one place the address is drawn) and *Sign out*, signed out
/// *Sign in*. `confirm` is Sign out's confirmation in its place: *Cancel* first, where focus
/// lands, so a stray OK keeps the account, then *Sign out*.
pub fn account_form(email: Option<&str>, confirm: bool) -> MenuForm {
    if confirm {
        let s = FormSection::new(msg::app_avatar_confirm_sign_out())
            .item(MenuRow::Cancel, RowKind::Button, MenuRow::Cancel, Row::new(msg::app_avatar_cancel()))
            .item(MenuRow::ConfirmSignOut, RowKind::Button, MenuRow::ConfirmSignOut, Row::new(msg::app_addons_sign_out()).destructive(true));
        return Form::new().section(s);
    }
    let s = FormSection::new("")
        .item(MenuRow::Settings, RowKind::Button, MenuRow::Settings, Row::new(msg::app_tab_settings()))
        .item(MenuRow::Addons, RowKind::Button, MenuRow::Addons, Row::new(msg::app_addons_title()));
    let s = match email {
        // the address is user text, on the line under its label: a long one elides
        Some(email) => s
            .item(MenuRow::Account, RowKind::Button, MenuRow::Account, Row::new(msg::app_addons_signed_in()).detail(email).server_detail())
            .item(MenuRow::SignOut, RowKind::Button, MenuRow::SignOut, Row::new(msg::app_addons_sign_out()).destructive(true)),
        None => s.item(MenuRow::SignIn, RowKind::Button, MenuRow::SignIn, Row::new(msg::app_addons_sign_in())),
    };
    Form::new().section(s)
}

/// A player menu's panel: above the transport's control row, its right edge on the row's.
pub fn player_panel_at(content_w: f32, content_h: f32) -> Rect {
    let width = content_w.clamp(theme::layout::PLAYER_MENU_MIN_W, MENU_MAX_W);
    let bottom = theme::layout::PLAYER_MENU_BOTTOM;
    let h = content_h.clamp(120.0, bottom - EDGE);
    Rect::new(SCR_W - EDGE_X - width, bottom - h, width, h)
}

/// Is `kind` one of the player's menus?
fn is_player(kind: &MenuKind) -> bool {
    match kind {
        // a Settings language's menu hangs beside its row; every other choice and list menu is the
        // player's (Live TV's chips are a chip row now)
        MenuKind::List { target: ChoiceTarget::SettingsLanguage, .. } => false,
        MenuKind::Choice { .. } | MenuKind::List { .. } | MenuKind::Subtitles | MenuKind::Player(_) => true,
        MenuKind::Item { .. } | MenuKind::Account => false,
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
    /// Where the item stood when the menu opened: what its library row does.
    add: AddState,
    form: FormTable<MenuRow, MenuRow, Infallible>,
    /// The account menu's address (`None`: signed out). User text, never logged.
    account: Option<String>,
    /// A movie's watched state when the menu opened (what its watched row does).
    watched: Option<bool>,
}

impl MenuScreen {
    pub fn new(entry: EntryId, arg: MenuArg, session: SessionView, add: AddState, continuing: bool) -> Self {
        let mut form = FormTable::new(KEY_BAND);
        form.table.compact = true;
        let initial = match &arg.kind {
            MenuKind::Choice { current, .. } => Some(MenuRow::Option(*current)),
            MenuKind::Item { .. } | MenuKind::List { .. } | MenuKind::Subtitles | MenuKind::Account | MenuKind::Player(_) => None,
        };
        form.set(build_with(&arg.kind, session, add, continuing), initial.as_ref());
        MenuScreen { entry, arg, add, form, account: None, watched: None }
    }

    /// An item menu for a movie whose entry is `watched` (or not): its *Mark as Watched /
    /// Unwatched* row. Nothing for any other menu.
    pub fn with_watched(mut self, session: SessionView, continuing: bool, watched: Option<bool>) -> Self {
        if matches!(self.arg.kind, MenuKind::Item { .. }) && watched.is_some() {
            self.form.set(build_item(&self.arg.kind, session, self.add, continuing, watched), None);
            self.watched = watched;
        }
        self
    }

    /// The signed-in account's address for the account menu's account line (`None`: signed out).
    pub fn with_account(mut self, account: Option<String>) -> Self {
        if self.arg.kind == MenuKind::Account {
            self.form.set(account_form(account.as_deref(), false), None);
        }
        self.account = account;
        self
    }

    pub fn kind(&self) -> &MenuKind {
        &self.arg.kind
    }

    /// The rows as the tests read them.
    pub fn table(&self) -> &sn_ui::table::TableView {
        &self.form.table
    }

    fn frame(&self, measure: &dyn sn_machine::machine::Measure) -> Rect {
        let (w, h) = (self.form.table.measured_width(measure), self.form.table.measured_height());
        if let Some(r) = crate::dropdown::panel(&self.arg, w, h) {
            return r;
        }
        if self.arg.kind == MenuKind::Account {
            // hung under the account pill, as a dropdown under its button
            return crate::dropdown::under(self.arg.anchor_rect(), w, h);
        }
        if is_player(&self.arg.kind) {
            player_panel_at(w, h)
        } else {
            panel_at(self.arg.anchor_rect(), w, h)
        }
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
        if self.arg.kind == MenuKind::Account {
            self.account_action(row, fx);
            return;
        }
        if !crate::dropdown::stays_open(&self.arg.kind) {
            fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
        }
        match (&self.arg.kind, row) {
            (MenuKind::Item { item, .. }, MenuRow::Open) => fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item.clone())))),
            (MenuKind::Item { item, poster }, MenuRow::AddToLibrary) => {
                let entry = sn_stremio::library::NewEntry {
                    id: item.id.clone(),
                    kind: item.kind.clone(),
                    name: item.name.clone(),
                    poster: poster.clone(),
                };
                let cmd = if self.add == AddState::In { LibraryCmd::Remove(entry.id) } else { LibraryCmd::Add(entry) };
                fx.push(Fx::App(AppFx::Library(cmd)));
            }
            (MenuKind::Item { .. }, MenuRow::SignIn) => fx.push(Fx::Nav(NavOp::Push(AppArg::SignIn))),
            (MenuKind::Item { item, .. }, MenuRow::RemoveFromContinue) => {
                fx.push(Fx::App(AppFx::Library(LibraryCmd::Rewind(item.id.clone()))));
            }
            (MenuKind::Item { item, poster }, MenuRow::Watched) => {
                let entry = sn_stremio::library::NewEntry { id: item.id.clone(), kind: item.kind.clone(), name: item.name.clone(), poster: poster.clone() };
                let edit = sn_stremio::library::Edit::MarkWatched(!self.watched.unwrap_or(false));
                fx.push(Fx::App(AppFx::Library(LibraryCmd::Mark(entry, edit))));
            }
            (MenuKind::Choice { target, .. } | MenuKind::List { target, .. }, MenuRow::Option(i)) => {
                fx.push(Fx::Deliver(self.arg.reply, Delivery::Screen(ScreenEvent::App(AppMsg::Chose(*target, i)))));
            }
            _ => {}
        }
    }

    /// OK on an account menu row. *Sign out* asks first, in place: the rows become the
    /// confirmation with focus on *Cancel*; only its *Sign out* sends `AccountCmd::SignOut`.
    fn account_action(&mut self, row: MenuRow, fx: &mut Effects<'_, Bundle>) {
        let dismiss = Fx::Nav(NavOp::Dismiss(self.entry));
        match row {
            MenuRow::Settings => {
                fx.push(dismiss);
                fx.push(Fx::Nav(NavOp::SelectTab(AppArg::Settings)));
            }
            MenuRow::Addons => {
                fx.push(dismiss);
                fx.push(Fx::Nav(NavOp::Push(AppArg::Addons)));
            }
            MenuRow::SignIn => {
                fx.push(dismiss);
                fx.push(Fx::Nav(NavOp::Push(AppArg::SignIn)));
            }
            MenuRow::SignOut => {
                self.form.set(account_form(self.account.as_deref(), true), Some(&MenuRow::Cancel));
                let cancel = FocusKey { entry: self.entry, elem: MenuRow::Cancel.key().0 };
                crate::registry::reseat(sn_ui::screen::FocusTarget::Elem(cancel), fx);
                fx.invalidate(sn_machine::present::Provenance::Input);
            }
            MenuRow::Cancel => fx.push(dismiss),
            MenuRow::ConfirmSignOut => {
                fx.push(dismiss);
                fx.push(Fx::App(AppFx::Account(AccountCmd::SignOut)));
            }
            // the account line reads; it does not act
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
                if let Some(kind) = crate::dropdown::live(&self.arg.kind, &cx.views).filter(|k| *k != self.arg.kind) {
                    let keep = self.form.selected_id().cloned();
                    self.arg.kind = kind;
                    self.form.set_sliding(build(&self.arg.kind, cx.views.session, AddState::Absent), keep.as_ref());
                }
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
                // the player's audio menu is a toggle on its disc's key: RED opened it, RED closes it
                if let InputKind::Key { wcode: crate::tabs::WCODE_RED, edge, .. } = input.kind {
                    if matches!(self.arg.kind, MenuKind::Choice { target: ChoiceTarget::PlayerAudio, .. }) {
                        if edge == Edge::Down {
                            fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                        }
                        return Handled::Yes;
                    }
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
        MenuKind::Choice { .. } | MenuKind::List { .. } | MenuKind::Subtitles | MenuKind::Account | MenuKind::Player(_) => None,
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
        let item = MenuKind::Item { item: ItemRef { kind: "movie".into(), id: "tt1".into(), name: "Film".into(), from: String::new() }, poster: None };
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            for (signed_in, add) in [
                (false, AddState::Absent),
                (true, AddState::Absent),
                (true, AddState::Adding),
                (true, AddState::Removing),
                (true, AddState::In),
            ] {
                let mut t = FormTable::<MenuRow, MenuRow, Infallible>::new(KEY_BAND);
                t.table.compact = true;
                t.set(build_item(&item, SessionView { signed_in }, add, true, Some(add == AddState::In)), None);
                out.extend(t.table.app_fit_failures_hugged(&format!("{} signed_in={signed_in} {add:?}", language.tag())));
                if let Some(why) = t.table.menu_cap_failure(&sn_base::fontcov::advances::ShippedMeasure, "item menu") {
                    out.push(why);
                }
            }
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }

    /// The player's menus — the tracks, the `…` menu (with its aspect and Stats for nerds rows),
    /// the subtitle ladders, the aspect modes — fit
    /// their panel in every shipped language.
    #[test]
    fn the_player_menus_fit_their_panel_in_every_shipped_language() {
        use crate::player::tracks;
        use crate::registry::{MediaFacts, TrackFacts, VideoFacts};
        use crate::stores::settings::{Pref, SettingsStore};
        use sn_machine::machine::Measure as _;
        use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
        let t = |lang: &str, name: &str, codec: &str, channels| TrackFacts {
            language: lang.into(),
            name: name.into(),
            codec: codec.into(),
            channels,
            atmos: false,
            // the widest conversion the menu can name
            fed: if codec == "truehd" { "Dolby Digital" } else { "" },
        };
        let media = MediaFacts {
            audio: vec![t("eng", "", "eac3", 6), t("", "", "truehd", 8), t("fre", "Commentaire", "ac3", 2)],
            audio_sel: Some(0),
            subs: vec![t("eng", "SDH", "", 0), t("", "", "", 0)],
            addon_subs: vec![crate::registry::AddonSubFacts { language: "ger".into(), addon: "OpenSubtitles v3".into(), ..Default::default() }],
            sub_sel: Some(1),
            video: Some(VideoFacts { codec: "hevc".into(), width: 3840, height: 2160, fps_milli: 23_976, dovi_profile: Some(8) }),
            chapters: Vec::new(),
        };
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let mut kinds = vec![
                ("audio", tracks::audio_menu(&crate::registry::PlaybackView { media: media.clone(), audio_delay_ms: -3000, ..crate::registry::PlaybackView::IDLE })),
                ("audio (none)", tracks::audio_menu(&crate::registry::PlaybackView::IDLE)),
                ("audio sync", crate::dropdown::audio_sync_menu(&crate::registry::PlaybackView { audio_delay_ms: -3000, ..crate::registry::PlaybackView::IDLE })),
                ("delay", crate::dropdown::subtitle_delay_menu(&crate::registry::PlaybackView { sub_delay_ms: -12_750, ..crate::registry::PlaybackView::IDLE })),
                ("aspect", tracks::aspect_menu(&crate::registry::PlaybackView::IDLE)),
            ];
            for size in 0..Pref::SubtitleSize.rungs() {
                for position in 0..Pref::SubtitlePosition.rungs() {
                    let settings = SettingsStore::with(size, position, crate::stores::settings::SKIP_BUTTON);
                    for aspect in 0..tracks::aspect_labels().len() as u8 {
                        for stats in [false, true] {
                            let view = crate::registry::PlaybackView { aspect, stats, ..crate::registry::PlaybackView::IDLE };
                            kinds.push(("more", tracks::more_menu(&view)));
                        }
                    }
                    kinds.push(("size", tracks::pref_menu(&settings, Pref::SubtitleSize)));
                    kinds.push(("position", tracks::pref_menu(&settings, Pref::SubtitlePosition)));
                }
            }
            for (what, kind) in kinds {
                let mut t = FormTable::<MenuRow, MenuRow, Infallible>::new(KEY_BAND);
                t.table.compact = true;
                t.set(build(&kind, SessionView::default(), AddState::Absent), None);
                let tag = format!("{} {what}", language.tag());
                out.extend(t.table.app_fit_failures_hugged(&tag));
                // a track's name is the file's and may be any length (it elides); the app's own
                // menus stay inside the panel's cap
                let files_text = what == "audio";
                if let Some(why) = t.table.menu_cap_failure(&sn_base::fontcov::advances::ShippedMeasure, &tag).filter(|_| !files_text) {
                    out.push(why);
                }
            }
            // the subtitle menu's Off row is the app's word among the file's names
            let off = sn_base::fontcov::advances::ShippedMeasure.width_str(msg::app_player_subtitles_off(), sn_ui::theme::size::BODY, false);
            if off > sn_ui::theme::layout::PLAYER_MENU_MIN_W - 120.0 {
                out.push(format!("{}: Off is {off} px", language.tag()));
            }
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }

    #[test]
    fn a_player_menu_rises_from_the_control_row_on_its_right_edge() {
        let r = player_panel_at(300.0, 400.0);
        assert_eq!(r.w, sn_ui::theme::layout::PLAYER_MENU_MIN_W, "a narrow menu is still a panel");
        assert_eq!(r.x + r.w, SCR_W - EDGE_X, "on the row's right edge");
        assert_eq!(r.y + r.h, sn_ui::theme::layout::PLAYER_MENU_BOTTOM, "above the row");
        let tall = player_panel_at(500.0, 5000.0);
        assert!(tall.y >= EDGE, "a long list is clipped inside the safe band");
    }

    #[test]
    fn add_to_library_signs_in_first_while_signed_out() {
        let item = MenuKind::Item { item: ItemRef { kind: "movie".into(), id: "tt1".into(), name: "Film".into(), from: String::new() }, poster: None };
        let mut t = FormTable::<MenuRow, MenuRow, Infallible>::new(KEY_BAND);
        t.set(build(&item, SessionView { signed_in: false }, AddState::Absent), None);
        assert_eq!(t.activate(0), Some(Activation::Action(MenuRow::Open)));
        assert_eq!(t.activate(1), Some(Activation::Action(MenuRow::SignIn)), "signed out, the row signs in (no dead row)");
        assert!(!t.table.sections[0].rows[1].dim);
        assert_eq!(t.table.sections[0].rows[1].label, msg::app_menu_sign_in_to_add());
        let signed_in = SessionView { signed_in: true };
        t.set(build(&item, signed_in, AddState::Absent), None);
        assert_eq!(t.activate(1), Some(Activation::Action(MenuRow::AddToLibrary)));
        t.set(build(&item, signed_in, AddState::In), None);
        assert_eq!(t.activate(1), Some(Activation::Action(MenuRow::AddToLibrary)), "in the library: the row removes it");
        assert_eq!(t.table.sections[0].rows[1].label, msg::app_menu_remove_from_library());
        for (add, detail) in [(AddState::Adding, msg::app_menu_adding()), (AddState::Removing, msg::app_menu_removing())] {
            t.set(build(&item, signed_in, add), None);
            assert_eq!(t.activate(1), None, "{add:?}: nothing to press while the write is on its way");
            assert_eq!(t.table.sections[0].rows[1].detail, detail);
        }
    }

    #[test]
    fn a_continued_title_offers_remove_from_continue_watching_signed_in_only() {
        let item = MenuKind::Item { item: ItemRef { kind: "series".into(), id: "tt2".into(), name: "Show".into(), from: String::new() }, poster: None };
        let labels = |f: MenuForm| -> Vec<String> {
            let mut t = FormTable::<MenuRow, MenuRow, Infallible>::new(KEY_BAND);
            t.set(f, None);
            t.table.sections[0].rows.iter().map(|r| r.label.clone()).collect()
        };
        let signed_in = SessionView { signed_in: true };
        assert_eq!(labels(build_with(&item, signed_in, AddState::In, true)).last().map(String::as_str), Some(msg::app_menu_remove_from_continue()));
        assert_eq!(labels(build_with(&item, signed_in, AddState::In, false)).len(), 2, "not continued: no row");
        assert_eq!(labels(build_with(&item, SessionView { signed_in: false }, AddState::Absent, true)).len(), 2, "signed out: no row");
    }

    #[test]
    fn a_movie_offers_its_watched_verb_signed_in_only() {
        let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        let item = MenuKind::Item { item: ItemRef { kind: "movie".into(), id: "tt1".into(), name: "Film".into(), from: String::new() }, poster: None };
        let labels = |f: MenuForm| -> Vec<String> {
            let mut t = FormTable::<MenuRow, MenuRow, Infallible>::new(KEY_BAND);
            t.set(f, None);
            t.table.sections[0].rows.iter().map(|r| r.label.clone()).collect()
        };
        let on = SessionView { signed_in: true };
        assert_eq!(labels(build_item(&item, on, AddState::In, false, Some(false))).last().map(String::as_str), Some("Mark as Watched"));
        assert_eq!(labels(build_item(&item, on, AddState::In, false, Some(true))).last().map(String::as_str), Some("Mark as Unwatched"));
        assert_eq!(labels(build_item(&item, SessionView { signed_in: false }, AddState::Absent, false, Some(false))).len(), 2, "signed out: no row");
        assert_eq!(labels(build_item(&item, on, AddState::In, false, None)).len(), 2);
    }
}
