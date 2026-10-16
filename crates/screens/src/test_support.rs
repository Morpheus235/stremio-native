//! A rig for driving the screens through the REAL dispatcher with no SDL, no GL and no network:
//! the real stores behind [`Views`], the effects the screens emit recorded instead of executed,
//! a virtual 60 Hz clock. The application's own rig (`app::bridge`) executes the same effects
//! against workers; this one lets a test do by hand what those workers would.
#![cfg(any(test, feature = "test-support"))]

use sn_machine::machine::{Effects, Handled, Key, MachineId, NavOp, Tick, TimerId};
use sn_machine::present::Present;
use sn_ui::dispatch::{CxParts, Dispatcher, FrameReport, NoTap, Rig, Split};
use sn_ui::fixture::{key, key_up, FixtureMeasure};
use sn_ui::frame::Budget;

use crate::registry::{AppArg, AppFx, AppMounter, AppMsg, Bundle, PlaybackView, Views};
use crate::stores::catalog::CatalogStore;
use crate::stores::detail::DetailStore;
use crate::stores::StoreId;

pub struct TestRig {
    mounter: AppMounter,
    pub catalog: CatalogStore,
    pub detail: DetailStore,
    pub playback: PlaybackView,
    /// Every application effect the screens emitted, in order.
    pub effects: Vec<AppFx>,
    pub log: Vec<String>,
    pub back_at_root: u32,
    measure: FixtureMeasure,
}

impl TestRig {
    pub fn new(catalog: CatalogStore) -> Self {
        TestRig {
            mounter: AppMounter,
            catalog,
            detail: DetailStore::new(),
            playback: PlaybackView::IDLE,
            effects: Vec::new(),
            log: Vec::new(),
            back_at_root: 0,
            measure: FixtureMeasure,
        }
    }
}

impl Rig<Bundle> for TestRig {
    fn split(&mut self) -> Split<'_, Bundle> {
        Split {
            mounter: &mut self.mounter,
            views: Views { catalog: &self.catalog, detail: &self.detail, playback: &self.playback },
            measure: &self.measure,
        }
    }
    fn deliver(&mut self, _to: MachineId, msg: &AppMsg, _parts: &CxParts<u32>, _fx: &mut Effects<'_, Bundle>) -> Handled {
        match *msg {}
    }
    fn timer(&mut self, _owner: MachineId, _id: TimerId, _parts: &CxParts<u32>, _fx: &mut Effects<'_, Bundle>) {}
    fn app_fx(&mut self, _from: MachineId, fx: AppFx, _parts: &CxParts<u32>, _out: &mut Effects<'_, Bundle>) {
        self.effects.push(fx);
    }
    fn log(&mut self, line: &str) {
        self.log.push(line.to_string());
    }
    fn prepare(&mut self, _b: &mut Budget, _present: &mut Present) {}
    fn ls2_pump(&mut self) {}
    fn opaque_route(&mut self, _video_plane_bound: bool) {}
    fn clear_opaque_region(&mut self) {}
    fn now_us(&self) -> u64 {
        0
    }
    fn back_at_root(&mut self) {
        self.back_at_root += 1;
    }
}

/// The dispatcher over a [`TestRig`], booted onto Home.
pub struct TestApp {
    pub d: Dispatcher<Bundle>,
    pub rig: TestRig,
    ms: u32,
}

impl TestApp {
    pub fn boot(catalog: CatalogStore) -> Self {
        let mut app = TestApp { d: Dispatcher::new(), rig: TestRig::new(catalog), ms: 0 };
        app.d.request(MachineId::Nav, NavOp::Root(AppArg::Home));
        app.frame();
        app
    }

    fn tick(&mut self) -> Tick {
        self.ms += 16;
        Tick { ms: self.ms, dt_us: 16_000 }
    }

    /// The tick the next frame will carry (for stamping its input).
    fn peek(&self) -> Tick {
        Tick { ms: self.ms + 16, dt_us: 16_000 }
    }

    fn run(&mut self, inputs: Vec<sn_machine::machine::InputEvent<u32>>) -> FrameReport {
        let t = self.tick();
        sn_machine::idle::frame_begin(1.0 / 60.0);
        let report = self.d.frame_with(&mut self.rig, t, inputs, Vec::new(), &mut NoTap, false);
        // every frame draws, as a presenting frame does: the stops a screen registers are the
        // ones it drew, so a run that never drew would navigate an empty hit map
        self.d.draw(&mut self.rig, true);
        let unmounted = report.unmounted.clone();
        self.d.prune(&unmounted);
        report
    }

    pub fn frame(&mut self) -> FrameReport {
        self.run(Vec::new())
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    /// A key pressed and released, then frames enough for a press to commit.
    pub fn press(&mut self, k: Key) {
        let t = self.peek();
        self.run(vec![key(k, t)]);
        let t = self.peek();
        self.run(vec![key_up(k, t)]);
        self.frames(40);
    }

    /// Tell every mounted page that `store` moved, as the application's loop does after a landing.
    pub fn notify(&mut self, store: StoreId, gen: u32) {
        self.d.store_changed(store.ord(), gen);
        self.frame();
    }

    pub fn top_name(&self) -> &'static str {
        self.d.top_screen().map_or("", |s| s.name())
    }

    pub fn top_arg(&self) -> Option<&AppArg> {
        self.d.nav.top_page().map(|e| &e.arg)
    }

    pub fn take_effects(&mut self) -> Vec<AppFx> {
        std::mem::take(&mut self.rig.effects)
    }
}
