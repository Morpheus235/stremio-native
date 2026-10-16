//! **The player page** — the playing video, which is the television's hardware plane composited
//! UNDER this surface. The page punches its hole (a transparent clear) and draws only what the
//! playback's phase calls for: the working read-out while it connects or buffers, the failure
//! read-out when it fails, nothing at all while it plays.
//!
//! Transport is the remote's: OK and the PLAY / PAUSE keys pause and resume, BACK and STOP end
//! the playback and leave the page. Leaving the page by any road ends the playback (`Unmount`).
//! The page names no media type: what it shows arrives as [`PlaybackView`] and what it asks for
//! leaves as [`PlayerCmd`].
//!
//! [`PlaybackView`]: crate::registry::PlaybackView

use std::borrow::Cow;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputEvent, InputKind, Leave, LogicalState, Machine, NavOp,
};
use sn_machine::present::Provenance;
use sn_ui::consts::{classify_input, Key as UiKey};
use sn_ui::screen::{At, Dir, DrawFrame, FocusSource, Focusable, GroupSpec, HitSource, Placed, RenderStrategy, Screen, ScreenEvent, Step};
use sn_ui::widgets::{StatusKind, StatusOverlay};
use sn_ui::{Env, Rect};

use crate::registry::{word, AppFx, Bundle, PlayPhase, PlaybackView, PlayerCmd};
use crate::stores::StoreId;

pub struct PlayerScreen {
    entry: EntryId,
    /// What the viewer chose. Server text, never logged.
    title: String,
    /// The phase last drawn, so a change is a frame.
    shown: PlaybackView,
    stopped: bool,
}

impl PlayerScreen {
    pub fn new(entry: EntryId, title: String) -> Self {
        PlayerScreen { entry, title, shown: PlaybackView::IDLE, stopped: false }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    fn stop(&mut self, fx: &mut Effects<'_, Bundle>) {
        if !std::mem::replace(&mut self.stopped, true) {
            fx.push(Fx::App(AppFx::Player(PlayerCmd::Stop)));
        }
    }

    fn overlay(view: &PlaybackView, ms: u32) -> Option<StatusOverlay<'static>> {
        match view.phase {
            PlayPhase::Working => Some(StatusOverlay::new(Rect::FULL, view.caption, StatusKind::Working).phase(ms)),
            PlayPhase::Failed => {
                let mut o = StatusOverlay::new(Rect::FULL, view.caption, StatusKind::Failed).page(sn_ui::icons::Icon::Alert);
                if !view.reason.is_empty() {
                    o = o.reason(view.reason);
                }
                Some(o)
            }
            PlayPhase::Idle | PlayPhase::Playing | PlayPhase::Paused => None,
        }
    }
}

impl LogicalState for PlayerScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).bool(self.stopped);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("player phase={:?} stopped={}", self.shown.phase, self.stopped));
    }
}

impl Machine<Bundle> for PlayerScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Tick(_) | ScreenEvent::StoreChanged(..) => {
                let now = *cx.views.playback;
                if now != self.shown {
                    self.shown = now;
                    fx.invalidate(Provenance::Landing(fx.from()));
                }
                // a working read-out's spinner turns from the frame clock
                if now.phase == PlayPhase::Working {
                    fx.note(sn_machine::present::PresentEvent::Motion);
                }
                match ev {
                    ScreenEvent::StoreChanged(o, _) if *o != StoreId::Playback.ord() => Handled::No,
                    _ => Handled::Yes,
                }
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key, sym, wcode, edge: Edge::Down, .. }, .. }) => {
                match classify_input(*key, *sym, *wcode) {
                    UiKey::Ok | UiKey::Play | UiKey::Pause | UiKey::PlayPause => {
                        fx.push(Fx::App(AppFx::Player(PlayerCmd::TogglePause)));
                        Handled::Yes
                    }
                    UiKey::Back | UiKey::Stop => {
                        self.stop(fx);
                        fx.push(Fx::Nav(NavOp::Pop));
                        Handled::Yes
                    }
                    _ => Handled::No,
                }
            }
            ScreenEvent::WillLeave(Leave::ForGood) | ScreenEvent::Unmount => {
                self.stop(fx);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

impl Focusable<Bundle> for PlayerScreen {
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
        FocusKey { entry: self.entry, elem: 0 }
    }
}

impl Screen<Bundle> for PlayerScreen {
    fn name(&self) -> &'static str {
        word::PLAYER
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        // a hole the plane shows through, never a sheet over it
        sn_gfx::gfx::frame_clear_through();
        if let Some(overlay) = Self::overlay(f.cx.views.playback, f.cx.tick.ms) {
            overlay.draw_measured(&Env::inert(), f.painter.alpha(f.page_alpha), f.measure);
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::VideoPlane
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
}
