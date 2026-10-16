//! **Library** — the user's Stremio library, a top-bar destination.
//!
//! Signed out (every build until the sign-in work lands) it is a real page with one honest
//! read-out under the bar: "Sign in to see your library", and why. Nothing on it is focusable, so
//! the remote stays on the bar: LEFT/RIGHT walk the other destinations, BACK returns to Home.
//! The sign-in work replaces the read-out with the library's shelves; the tab, its pill and its
//! place in `crate::tabs::TABS` stay.

use std::borrow::Cow;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, FocusKey, GroupId, Handled, InputEvent, InputKind, Key, LogicalState, Machine,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_ui::consts::{SCR_H, SCR_W};
use sn_ui::screen::{At, Dir, DrawFrame, FocusTarget, Focusable, GroupSpec, Placed, RenderStrategy, Screen, ScreenEvent, Step};
use sn_ui::widgets::{StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect};

use crate::registry::{word, Bundle, FirstSeat};
use crate::tabs::Tab;

/// The read-out's frame: the screen under the top bar.
pub fn readout_frame() -> Rect {
    let top = sn_ui::widgets::TOP_BAR_BOTTOM;
    Rect::new(0.0, top, SCR_W, SCR_H - top)
}

pub struct LibraryScreen {
    entry: EntryId,
    /// With nothing focusable on the page, the remote rests on the page's own pill.
    seat: FirstSeat,
}

impl LibraryScreen {
    pub fn new(entry: EntryId) -> Self {
        LibraryScreen { entry, seat: FirstSeat::new() }
    }

    /// The signed-out read-out: its verdict and the line under it.
    pub fn readout() -> (&'static str, &'static str) {
        (msg::app_library_sign_in(), msg::app_library_sign_in_reason())
    }
}

impl LogicalState for LibraryScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0);
    }
    fn probe(&self, out: &mut String) {
        out.push_str("library signed-out");
    }
}

impl Machine<Bundle> for LibraryScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Tick(_) => {
                let seated = crate::tabs::rests(self.entry, Tab::Library, cx, false, false);
                self.seat.tick(seated, Some(FocusTarget::Elem(FocusKey { entry: self.entry, elem: Tab::Library.elem() })), fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, Tab::Library, None, fx);
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry && crate::tabs::on_strip(k.elem)) {
                    crate::tabs::on_pill(k.elem, Tab::Library, None, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => {
                crate::tabs::on_back(self.entry, Tab::Library, cx, fx)
            }
            ScreenEvent::Enter(_) | ScreenEvent::Uncover => {
                fx.invalidate(Provenance::Nav);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

/// Nothing on the page itself takes focus: the container's bar is the page's only stop.
impl Focusable<Bundle> for LibraryScreen {
    fn groups(&self, _cx: &Cx<'_, Bundle>, _out: &mut Vec<GroupSpec>) {}
    fn group_of(&self, _key: &u32, _cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        None
    }
    fn neighbour(&self, _key: FocusKey<u32>, _dir: Dir, _cx: &Cx<'_, Bundle>) -> Step<u32> {
        Step::Edge
    }
    fn place(&self, _key: &u32, _cx: &Cx<'_, Bundle>, _at: At) -> Option<Placed> {
        None
    }
    fn reconcile(&self, want: FocusKey<u32>, _cx: &Cx<'_, Bundle>) -> FocusKey<u32> {
        want
    }
    fn seat(&self, _g: GroupId, _from: Placed, _cx: &Cx<'_, Bundle>) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem: Tab::Library.elem() }
    }
}

impl Screen<Bundle> for LibraryScreen {
    fn name(&self) -> &'static str {
        word::LIBRARY
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
        StatusOverlay::new(readout_frame(), msg::app_library_sign_in_c(), StatusKind::Empty)
            .reason(msg::app_library_sign_in_reason_c())
            .draw_measured(&Env::inert(), f.painter.alpha(f.page_alpha), f.measure);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}
