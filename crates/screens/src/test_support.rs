//! A rig for driving the screens through the REAL dispatcher with no SDL, no GL and no network:
//! the real stores behind [`Views`], the real top bar ([`ChromeBar`]), the effects the screens
//! emit recorded (and, for the stores that need no network to move — search, grid, settings —
//! applied the way the application's bridge applies them), a virtual 60 Hz clock. The
//! application's own rig (`app::bridge`) executes the same effects against workers; this one lets
//! a test do by hand what those workers would.
#![cfg(any(test, feature = "test-support"))]

use std::sync::Arc;

use sn_machine::machine::{Edge, Effects, Handled, InputEvent, InputKind, Key, MachineId, NavOp, Source, TextEdit, Tick, TimerId};
use sn_machine::present::Present;
use sn_ui::dispatch::{CxParts, Dispatcher, FrameReport, NoTap, Rig, Split};
use sn_ui::fixture::{key, key_up, FixtureMeasure};
use sn_ui::frame::Budget;

use crate::chrome::ChromeBar;
use crate::registry::{AppArg, AppFx, AppMounter, AppMsg, Bundle, GridCmd, PlaybackView, SearchCmd, SessionView, SettingsCmd, Views};
use crate::stores::catalog::CatalogStore;
use crate::stores::account::AccountStore;
use crate::stores::addons::AddonsStore;
use crate::stores::detail::DetailStore;
use crate::stores::library::LibraryStore;
use crate::stores::grid::{GridJob, GridStore};
use crate::stores::search::{SearchJob, SearchStore};
use crate::stores::settings::SettingsStore;
use crate::stores::StoreId;
use crate::tabs::{route_hotkey, HotkeyRoute};

pub struct TestRig {
    mounter: AppMounter,
    pub catalog: CatalogStore,
    pub account: AccountStore,
    pub addons: AddonsStore,
    pub library: LibraryStore,
    pub detail: DetailStore,
    pub playback: PlaybackView,
    pub search: SearchStore,
    pub grid: GridStore,
    pub settings: SettingsStore,
    pub chrome: ChromeBar,
    /// Every application effect the screens emitted, in order.
    pub effects: Vec<AppFx>,
    /// The jobs the applied search and grid commands handed out (a test lands them by hand).
    pub search_jobs: Vec<SearchJob>,
    pub grid_jobs: Vec<GridJob>,
    pub log: Vec<String>,
    pub back_at_root: u32,
    /// The keyboard requests the dispatcher passed on (`Rig::system_keyboard`): every raise
    /// (`true`) and dismissal (`false`), in order.
    pub keyboard: Vec<bool>,
    now: u32,
    measure: FixtureMeasure,
}

impl TestRig {
    pub fn new(catalog: CatalogStore) -> Self {
        TestRig {
            mounter: AppMounter,
            catalog,
            detail: DetailStore::new(),
            // a rig is a returning, signed-out viewer unless a test says otherwise
            account: AccountStore::signed_out(true),
            addons: AddonsStore::new(),
            library: LibraryStore::new(),
            playback: PlaybackView::IDLE,
            search: SearchStore::default(),
            grid: GridStore::new(),
            settings: SettingsStore::default(),
            chrome: ChromeBar::new(),
            effects: Vec::new(),
            search_jobs: Vec::new(),
            grid_jobs: Vec::new(),
            log: Vec::new(),
            back_at_root: 0,
            keyboard: Vec::new(),
            now: 0,
            measure: FixtureMeasure,
        }
    }

    /// The application bridge's handling of the network-free stores' commands.
    fn apply(&mut self, fx: &AppFx) {
        let addons = self.catalog.addons().to_vec();
        match fx {
            AppFx::Search(cmd) => {
                let jobs = match cmd {
                    SearchCmd::Type(t) => {
                        self.search.type_query(t, self.now);
                        Vec::new()
                    }
                    SearchCmd::Submit(t) => self.search.submit(t, &addons, self.addons.filter()),
                    // THE filter is the addons store's (as the bridge applies it)
                    SearchCmd::ToggleAddon(id) => {
                        self.addons.toggle_search(id);
                        self.search.replan(&addons, self.addons.filter())
                    }
                    SearchCmd::Retry(r) => self.search.retry(*r),
                    SearchCmd::Remember(t) => {
                        self.search.remember(t);
                        Vec::new()
                    }
                    SearchCmd::ClearRecents => {
                        self.search.clear_recents();
                        Vec::new()
                    }
                };
                self.search_jobs.extend(jobs);
            }
            AppFx::Grid(cmd) => {
                let jobs = match cmd {
                    GridCmd::Open(k) => {
                        let seed = self.search.pager_for(k).cloned();
                        self.grid.open(&addons, k.clone(), seed)
                    }
                    GridCmd::Close(k) => {
                        self.grid.close(k);
                        Vec::new()
                    }
                    GridCmd::More(k) => self.grid.more(k),
                    GridCmd::Retry(k) => self.grid.retry(k),
                };
                self.grid_jobs.extend(jobs);
            }
            AppFx::Settings(SettingsCmd::Set(p, r)) => {
                self.settings.set(*p, *r);
            }
            _ => {}
        }
    }
}

impl Rig<Bundle> for TestRig {
    fn split(&mut self) -> Split<'_, Bundle> {
        Split {
            mounter: &mut self.mounter,
            views: Views {
                catalog: &self.catalog,
                detail: &self.detail,
                playback: &self.playback,
                account: &self.account,
                addons: &self.addons,
                library: &self.library,
                search: &self.search,
                grid: &self.grid,
                settings: &self.settings,
                session: SessionView { signed_in: self.account.account().is_some() },
            },
            measure: &self.measure,
        }
    }
    fn deliver(&mut self, _to: MachineId, _msg: &AppMsg, _parts: &CxParts<u32>, _fx: &mut Effects<'_, Bundle>) -> Handled {
        Handled::No
    }
    fn timer(&mut self, _owner: MachineId, _id: TimerId, _parts: &CxParts<u32>, _fx: &mut Effects<'_, Bundle>) {}
    fn app_fx(&mut self, _from: MachineId, fx: AppFx, parts: &CxParts<u32>, _out: &mut Effects<'_, Bundle>) {
        self.now = parts.tick.ms;
        self.apply(&fx);
        self.effects.push(fx);
    }
    fn system_keyboard(&mut self, up: bool) {
        self.keyboard.push(up);
    }
    fn draw_chrome(&mut self, arg: &AppArg, _parts: &CxParts<u32>, nav: sn_ui::screen::NavPresentation, glass: Option<&mut sn_ui::frame::glass::GlassPlan>) {
        self.chrome.draw(arg, nav, glass);
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
    /// The generation each network-free store last announced.
    told: [Option<u32>; 4],
}

impl TestApp {
    pub fn boot(catalog: CatalogStore) -> Self {
        let mut app = TestApp { d: Dispatcher::new(), rig: TestRig::new(catalog), ms: 0, told: [None; 4] };
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

    pub fn now(&self) -> u32 {
        self.ms
    }

    fn run(&mut self, inputs: Vec<InputEvent<u32>>) -> FrameReport {
        let t = self.tick();
        sn_machine::idle::frame_begin(1.0 / 60.0);
        // the search store's debounce runs on the frame clock, as the application's loop runs it
        let addons = self.rig.catalog.addons().to_vec();
        let jobs = self.rig.search.tick(t.ms, &addons, self.rig.addons.filter());
        self.rig.search_jobs.extend(jobs);
        self.notify_stores();
        self.rig.chrome.capture(&mut self.d, &self.rig.measure);
        let report = self.d.frame_with(&mut self.rig, t, inputs, Vec::new(), &mut NoTap, false);
        self.rig.chrome.update(&mut self.d, None, 1.0 / 60.0);
        // every frame draws, as a presenting frame does: the stops a screen registers are the
        // ones it drew, so a run that never drew would navigate an empty hit map
        self.d.draw(&mut self.rig, true);
        let unmounted = report.unmounted.clone();
        self.d.prune(&unmounted);
        report
    }

    /// Tell the tree about the network-free stores that moved (the bridge's `notify`).
    fn notify_stores(&mut self) {
        let gens = [
            (StoreId::Search, self.rig.search.gen()),
            (StoreId::Grid, self.rig.grid.gen()),
            (StoreId::Settings, self.rig.settings.gen()),
            (StoreId::Addons, self.rig.addons.gen()),
        ];
        for (i, (id, gen)) in gens.into_iter().enumerate() {
            if self.told[i] != Some(gen) {
                self.told[i] = Some(gen);
                self.d.store_changed(id.ord(), gen);
            }
        }
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

    /// OK held past the hold threshold, then released.
    pub fn hold_ok(&mut self) {
        let t = self.peek();
        self.run(vec![key(Key::Ok, t)]);
        self.frames(60);
        let t = self.peek();
        self.run(vec![key_up(Key::Ok, t)]);
        self.frames(40);
    }

    /// A raw remote key `(sym, wcode)` as the application's loop delivers it: through the
    /// hotkeys first, and to the dispatcher only when they do not take it.
    pub fn press_raw(&mut self, sym: u32, wcode: u32) -> HotkeyRoute {
        let mut route = HotkeyRoute::NotHotkey;
        for edge in [Edge::Down, Edge::Up] {
            let r = route_hotkey(&mut self.d, sym, wcode, edge);
            if edge == Edge::Down {
                route = r;
            }
            if !matches!(r, HotkeyRoute::Went(_)) {
                let t = self.peek();
                let ev = InputEvent { at: t, source: Source::Script, kind: InputKind::Key { key: Key::Other, sym, wcode, edge, at_edge: false } };
                self.run(vec![ev]);
            } else {
                self.frame();
            }
        }
        self.frames(40);
        route
    }

    /// The television keyboard committing `text`, as `app::run` delivers an `SDL_TEXTINPUT`.
    pub fn type_text(&mut self, text: &str) {
        let t = self.peek();
        self.run(vec![InputEvent { at: t, source: Source::Script, kind: InputKind::Text(TextEdit::Commit(Arc::from(text))) }]);
        self.frames(2);
    }

    /// Tell every mounted page that `store` moved, as the application's loop does after a landing.
    pub fn notify(&mut self, store: StoreId, gen: u32) {
        self.d.store_changed(store.ord(), gen);
        self.frame();
    }

    /// The screen on top: a surface (a menu) over the page, else the page.
    pub fn top_name(&self) -> &'static str {
        self.d.top_surface_name().unwrap_or_else(|| self.d.top_screen().map_or("", |s| s.name()))
    }

    /// The topmost surface's screen, if one is up.
    pub fn top_surface(&self) -> Option<&dyn sn_ui::screen::Screen<Bundle>> {
        self.d.nav.modals.surfaces.last().and_then(|s| s.entry.inst.as_ref()).map(|i| &*i.screen)
    }

    pub fn top_arg(&self) -> Option<&AppArg> {
        self.d.nav.top_page().map(|e| &e.arg)
    }

    pub fn take_effects(&mut self) -> Vec<AppFx> {
        std::mem::take(&mut self.rig.effects)
    }
}
