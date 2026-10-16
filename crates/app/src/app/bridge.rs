//! **The bridge**: the application's `Rig` for the dispatcher — the stores the screens read, the
//! effects they ask for executed against the workers and the player, the shared top bar
//! (`sn_screens::chrome`), the television keyboard, the settings file, the render cache's upload
//! step, and the three privileged platform calls.
//!
//! **The installed addons are read through ONE accessor, [`Bridge::addons`]** — Search plans
//! against them, Live TV and the grids resolve against them, Detail asks their meta and streams.
//! Today they are the catalog store's (the signed-out defaults); the sign-in work switches every
//! page to the user's own collection by changing what it answers (`Views::addons` is its
//! screen-side twin).
//!
//! The stores are pure (`sn_screens::stores`): a screen's command changes what a store WANTS,
//! the store hands back jobs, [`Work`] runs them off the frame thread, and their landings are
//! folded back in [`Bridge::land`] at the top of the next frame. After every change the loop calls
//! [`Bridge::notify`], which tells the tree which store moved (`Dispatcher::store_changed`), and
//! the mounted screens re-read their views.

use std::path::PathBuf;

use sn_base::eventlog::log;
use sn_machine::machine::{Effects, Fx, Handled, MachineId, NavOp, PresentHandle, TimerId};
use sn_machine::present::Present;
use sn_screens::chrome::ChromeBar;
use sn_screens::registry::{
    AppArg, AppFx, AppMounter, AppMsg, Bundle, CatalogCmd, DetailCmd, GridCmd, PlayerCmd, SearchCmd, SessionView, SettingsCmd,
    Views,
};
use sn_screens::stores::catalog::CatalogStore;
use sn_screens::stores::detail::DetailStore;
use sn_screens::stores::grid::GridStore;
use sn_screens::stores::search::SearchStore;
use sn_screens::stores::settings::SettingsStore;
use sn_screens::stores::StoreId;
use sn_stremio::manifest::Addon;
use sn_ui::dispatch::{CxParts, Dispatcher, Rig, Split};
use sn_ui::frame::Budget;
use sn_ui::tex::{self, Decoded, Tex, Uploader};

use super::playback::Playback;
use crate::art::Art;
use crate::work::{Done, Runner, Threads, Work};

pub struct Bridge {
    mounter: AppMounter,
    pub catalog: CatalogStore,
    pub detail: DetailStore,
    pub search: SearchStore,
    pub grid: GridStore,
    pub settings: SettingsStore,
    /// Signed out until the sign-in work lands.
    pub session: SessionView,
    pub chrome: ChromeBar,
    pub playback: Playback,
    measure: Box<dyn sn_machine::machine::Measure>,
    work: Work,
    runner: Box<dyn Runner>,
    art: Option<&'static Art>,
    /// The generation each store last announced to the tree: catalog, detail, search, grid,
    /// settings.
    told: [Option<u32>; 5],
    /// The playback view changed since the tree was last told.
    playback_moved: bool,
    /// Where the settings are persisted (`None`: not at all, a host test).
    settings_path: Option<PathBuf>,
    /// The `(settings, search settings)` generations last written.
    saved: (u32, u32),
    /// The frame clock, for the search store's debounce.
    now_ms: u32,
}

impl Bridge {
    pub fn new(catalog: CatalogStore, playback: Playback, measure: Box<dyn sn_machine::machine::Measure>, art: Option<&'static Art>) -> Self {
        Self::with_runner(catalog, playback, measure, art, Box::new(Threads))
    }

    pub fn with_runner(
        catalog: CatalogStore,
        playback: Playback,
        measure: Box<dyn sn_machine::machine::Measure>,
        art: Option<&'static Art>,
        runner: Box<dyn Runner>,
    ) -> Self {
        Bridge {
            mounter: AppMounter,
            catalog,
            detail: DetailStore::new(),
            search: SearchStore::default(),
            grid: GridStore::new(),
            settings: SettingsStore::default(),
            session: SessionView::default(),
            chrome: ChromeBar::new(),
            playback,
            measure,
            work: Work::new(),
            runner,
            art,
            told: [None; 5],
            playback_moved: false,
            settings_path: None,
            saved: (0, 0),
            now_ms: 0,
        }
    }

    /// Seed the settings and the search store from the settings file, and persist every later
    /// change to `path`. The player is handed the subtitle preferences at once.
    pub fn with_settings(mut self, loaded: super::prefs::Loaded, path: PathBuf) -> Self {
        self.settings = loaded.settings;
        self.search = loaded.search;
        self.saved = (self.settings.gen(), self.search.settings_gen());
        self.settings_path = Some(path);
        super::prefs::publish(&self.settings);
        self
    }

    /// **The installed addons** — the one accessor every lookup plans against (see the module
    /// doc).
    pub fn addons(&self) -> &[Addon] {
        self.catalog.addons()
    }

    /// The frame's clock: the search store's debounce, and the settings written when they moved.
    pub fn tick(&mut self, now_ms: u32) {
        self.now_ms = now_ms;
        let jobs = {
            let addons = self.catalog.addons();
            self.search.tick(now_ms, addons)
        };
        if !jobs.is_empty() {
            self.work.search(&*self.runner, jobs);
        }
        self.persist();
    }

    /// Write the settings file when the settings or the search filter / recents moved.
    fn persist(&mut self) {
        let now = (self.settings.gen(), self.search.settings_gen());
        if now == self.saved {
            return;
        }
        self.saved = now;
        if let Some(path) = &self.settings_path {
            super::prefs::save(path.clone(), super::prefs::snapshot(&self.settings, &self.search));
        }
    }

    /// Publish the top bar for the page being shown, before the dispatcher's frame.
    pub fn capture_chrome(&mut self, pages: &mut Dispatcher<Bundle>) {
        self.chrome.capture(pages, &*self.measure);
    }

    /// Step the bar's motion after the frame, and publish the transition's presentation for the
    /// readers outside a page's own frame (the bar, the popovers).
    pub fn update_chrome(&mut self, pages: &mut Dispatcher<Bundle>, glass: &mut sn_ui::frame::glass::GlassPlan, dt: f32) {
        self.chrome.update(pages, Some(glass), dt);
        let tab = pages.nav.tabs.stack.pending_dest().and_then(sn_screens::tabs::Tab::of_arg).map(sn_screens::tabs::Tab::index);
        sn_ui::nav::publish(pages.nav.tabs.stack.page_alpha(), pages.nav.tabs.stack.chrome_alpha(), tab);
    }

    /// Start whatever the catalog store wants now (its manifests at boot, its pages after a view
    /// change or a landing freed a slot).
    pub fn start_catalog_jobs(&mut self) {
        let jobs = self.catalog.take_jobs();
        if !jobs.is_empty() {
            self.work.catalog(&*self.runner, jobs);
        }
    }

    /// Fold every worker outcome in, then start what the landings made possible. `true` when a
    /// store changed.
    pub fn land(&mut self) -> bool {
        let mut changed = false;
        for done in self.work.drain() {
            changed |= match done {
                Done::Catalog(l) => self.catalog.land(l),
                Done::Detail(l) => self.detail.land(l),
                Done::Search(l) => self.search.land(l),
                Done::Grid(l) => self.grid.land(l),
            };
        }
        if let Some(art) = self.art {
            art.drain();
        }
        self.start_catalog_jobs();
        changed
    }

    /// The frame's playback pump; the video plane's edge goes to the present gate.
    pub fn pump_playback(&mut self, now_ms: u32, present: &mut Present) {
        let (moved, edge) = self.playback.tick(now_ms);
        self.playback_moved |= moved;
        if let Some(bound) = edge {
            present.note(sn_machine::present::PresentEvent::VideoPlane(bound));
        }
    }

    /// Tell the tree which stores moved since it was last told.
    pub fn notify(&mut self, pages: &mut Dispatcher<Bundle>) {
        let gens = [
            (StoreId::Catalog, self.catalog.gen()),
            (StoreId::Detail, self.detail.gen()),
            (StoreId::Search, self.search.gen()),
            (StoreId::Grid, self.grid.gen()),
            (StoreId::Settings, self.settings.gen()),
        ];
        for (i, (id, gen)) in gens.into_iter().enumerate() {
            if self.told[i] != Some(gen) {
                self.told[i] = Some(gen);
                pages.store_changed(id.ord(), gen);
            }
        }
        if std::mem::take(&mut self.playback_moved) {
            pages.store_changed(StoreId::Playback.ord(), 0);
        }
    }

    fn catalog_cmd(&mut self, cmd: CatalogCmd) {
        match cmd {
            CatalogCmd::View(rows) => self.catalog.set_view(rows),
            CatalogCmd::More(row) => self.catalog.want_more(row),
            CatalogCmd::Retry(row) => self.catalog.retry(row),
            CatalogCmd::RetryAddons => self.catalog.retry_addons(),
        }
        self.start_catalog_jobs();
    }

    fn search_cmd(&mut self, cmd: SearchCmd) {
        let jobs = {
            let addons = self.catalog.addons();
            match cmd {
                SearchCmd::Type(text) => {
                    self.search.type_query(&text, self.now_ms);
                    Vec::new()
                }
                SearchCmd::Submit(text) => self.search.submit(&text, addons),
                SearchCmd::ToggleAddon(id) => self.search.toggle_addon(&id, addons),
                SearchCmd::Retry(row) => self.search.retry(row),
                SearchCmd::Remember(term) => {
                    self.search.remember(&term);
                    Vec::new()
                }
                SearchCmd::ClearRecents => {
                    self.search.clear_recents();
                    Vec::new()
                }
            }
        };
        if !jobs.is_empty() {
            self.work.search(&*self.runner, jobs);
        }
        self.persist();
    }

    fn grid_cmd(&mut self, cmd: GridCmd) {
        let jobs = match cmd {
            GridCmd::Open(key) => {
                // a search row's grid opens on the cards the row already holds
                let seed = self.search.pager_for(&key).cloned();
                self.grid.open(self.catalog.addons(), key, seed)
            }
            GridCmd::Close(key) => {
                self.grid.close(&key);
                Vec::new()
            }
            GridCmd::More(key) => self.grid.more(&key),
            GridCmd::Retry(key) => self.grid.retry(&key),
        };
        if !jobs.is_empty() {
            self.work.grid(&*self.runner, jobs);
        }
    }

    fn settings_cmd(&mut self, cmd: SettingsCmd) {
        match cmd {
            SettingsCmd::Set(pref, rung) => {
                if self.settings.set(pref, rung) {
                    super::prefs::publish(&self.settings);
                }
            }
        }
        self.persist();
    }

    fn detail_cmd(&mut self, cmd: DetailCmd) {
        let jobs = match cmd {
            DetailCmd::OpenMeta(key) => self.detail.open_meta(self.catalog.addons(), key),
            DetailCmd::CloseMeta(key) => {
                self.detail.close_meta(&key);
                Vec::new()
            }
            DetailCmd::OpenStreams(key) => self.detail.open_streams(self.catalog.addons(), key),
            DetailCmd::CloseStreams(key) => {
                self.detail.close_streams(&key);
                Vec::new()
            }
        };
        if !jobs.is_empty() {
            self.work.detail(&*self.runner, jobs);
        }
    }
}

impl Rig<Bundle> for Bridge {
    fn split(&mut self) -> Split<'_, Bundle> {
        Split {
            mounter: &mut self.mounter,
            views: Views {
                catalog: &self.catalog,
                detail: &self.detail,
                playback: &self.playback.view,
                search: &self.search,
                grid: &self.grid,
                settings: &self.settings,
                session: self.session,
            },
            measure: &*self.measure,
        }
    }

    /// No machine but a screen hears an `AppMsg` (a menu's choice goes to the page that asked).
    fn deliver(&mut self, _to: MachineId, _msg: &AppMsg, _parts: &CxParts<u32>, _fx: &mut Effects<'_, Bundle>) -> Handled {
        Handled::No
    }

    fn timer(&mut self, _owner: MachineId, _id: TimerId, _parts: &CxParts<u32>, _fx: &mut Effects<'_, Bundle>) {}

    fn app_fx(&mut self, _from: MachineId, fx: AppFx, _parts: &CxParts<u32>, out: &mut Effects<'_, Bundle>) {
        match fx {
            AppFx::Catalog(cmd) => self.catalog_cmd(cmd),
            AppFx::Detail(cmd) => self.detail_cmd(cmd),
            AppFx::Search(cmd) => self.search_cmd(cmd),
            AppFx::Grid(cmd) => self.grid_cmd(cmd),
            AppFx::Settings(cmd) => self.settings_cmd(cmd),
            AppFx::Play(req) => {
                self.playback.start(&req);
                self.playback_moved = true;
                out.push(Fx::Nav(NavOp::Push(AppArg::Player(req.title))));
            }
            AppFx::Player(PlayerCmd::TogglePause) => {
                self.playback.toggle_pause();
                self.playback_moved = true;
            }
            AppFx::Player(PlayerCmd::Stop) => {
                self.playback.stop();
                self.playback_moved = true;
            }
        }
    }

    fn log(&mut self, line: &str) {
        log(line);
    }

    /// The shared top bar over a page that wears it.
    fn draw_chrome(
        &mut self,
        arg: &AppArg,
        _parts: &CxParts<u32>,
        nav: sn_ui::screen::NavPresentation,
        glass: Option<&mut sn_ui::frame::glass::GlassPlan>,
    ) {
        self.chrome.draw(arg, nav, glass);
    }

    fn page_alpha(&self) -> f32 {
        sn_ui::nav::page_alpha()
    }

    fn navigation_presentation(&self) -> sn_ui::screen::NavPresentation {
        sn_ui::screen::NavPresentation {
            page_alpha: sn_ui::nav::page_alpha(),
            chrome_alpha: sn_ui::nav::chrome_alpha(),
            view_tab: u32::try_from(sn_ui::nav::view_tab(-1)).ok(),
            blur_amount: sn_ui::nav::blur_amount(),
        }
    }

    /// The television's keyboard, for the field that asked (`textinput`).
    fn system_keyboard(&mut self, up: bool) {
        #[cfg(not(test))]
        if up {
            crate::textinput::start();
        } else {
            crate::textinput::stop();
        }
        #[cfg(test)]
        let _ = up;
    }

    /// Text arrived with the panel up: it is ours again.
    fn adopt_system_keyboard(&mut self) {
        #[cfg(not(test))]
        crate::textinput::adopt();
    }

    /// The render cache's upload step (§3.3 step 9), on the presenting side of the decision.
    fn prepare(&mut self, b: &mut Budget, present: &mut Present) {
        let mut up = GfxUploader;
        if let Some(art) = self.art {
            for key in art.take_freed() {
                tex::free(key, &mut up);
            }
        }
        let mut ph = PresentHandle::of(present);
        tex::prepare(b, &mut up, &mut ph, sn_base::diag::heartbeat::now_us);
    }

    fn ls2_pump(&mut self) {
        sn_platform::tv::window::pump_bus();
    }

    fn opaque_route(&mut self, video_plane_bound: bool) {
        sn_platform::tv::window::opaque_route(video_plane_bound);
    }

    fn clear_opaque_region(&mut self) {
        if self.playback.player.video_plane_bound {
            sn_platform::tv::window::clear_opaque_region();
        }
    }

    fn now_us(&self) -> u64 {
        sn_base::diag::heartbeat::now_us()
    }

    /// BACK at Home: the television's own Home, and the app keeps running. A press the cooldown
    /// already spoke for does nothing.
    fn back_at_root(&mut self) {
        if sn_platform::tv::home::take_root_press() {
            sn_platform::tv::home::go_home();
        }
    }
}

/// The GL half behind the render cache.
pub struct GfxUploader;

impl Uploader for GfxUploader {
    fn upload(&mut self, d: &Decoded) -> Tex {
        let id = sn_gfx::img::img_upload_rgba(d.rgba.as_ptr(), i32::from(d.w), i32::from(d.h));
        Tex { id, w: d.w, h: d.h }
    }
    fn warm(&mut self, t: Tex) {
        sn_gfx::gfx::warm_tex(t.id);
    }
    fn free(&mut self, t: Tex) {
        sn_gfx::gfx::delete_tex(t.id);
    }
}

/// **The OS took the screen** (WILL/DID background): park the container tree — every mounted
/// body hears `Suspend` at the next commit.
pub fn background(pages: &mut Dispatcher<Bundle>) {
    pages.suspend();
}

/// **The screen came back** (WILL/DID foreground): un-park the tree. Idempotent, so both edges
/// may call it.
pub fn foreground(pages: &mut Dispatcher<Bundle>) {
    pages.resume();
}

/// Is the player page on top?
pub fn player_on_top(pages: &Dispatcher<Bundle>) -> bool {
    matches!(pages.top_arg(), Some(AppArg::Player(_)))
}

/// Is the Search page on top, with no menu over it? Its field is the only one text goes to.
pub fn search_on_top(pages: &Dispatcher<Bundle>) -> bool {
    matches!(pages.top_arg(), Some(AppArg::Search)) && pages.nav.modals.surfaces.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_base::task::{MainThread, OffFrame};
    use sn_machine::machine::{Edge, InputEvent, InputKind, Key, Source, Tick};
    use sn_screens::stores::catalog::{AddonsState, Landing};
    use sn_stremio::{Fetch, FetchError};
    use sn_ui::dispatch::NoTap;
    use std::sync::mpsc::Sender;

    const CINEMETA: &str = r#"{
        "id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta",
        "resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"],
        "catalogs":[
            {"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]},
            {"type":"series","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]}
        ]}"#;
    const STREAMER: &str = r#"{
        "id":"org.example.streams","version":"1.0.0","name":"Streamer",
        "resources":["stream"],"types":["movie"],"idPrefixes":["tt"],"catalogs":[]}"#;

    /// Addons as canned bodies, by URL suffix. Every request is recorded.
    struct FakeNet {
        seen: std::sync::Mutex<Vec<String>>,
    }
    impl Fetch for FakeNet {
        fn get(&self, url: &str, _h: &[(String, String)]) -> Result<Vec<u8>, FetchError> {
            self.seen.lock().unwrap().push(url.to_string());
            let body = if url.ends_with("cinemeta/manifest.json") {
                CINEMETA.to_string()
            } else if url.ends_with("streamer/manifest.json") {
                STREAMER.to_string()
            } else if url.contains("/catalog/") {
                let metas: Vec<String> = (0..10)
                    .map(|i| format!(r#"{{"id":"tt{i}","type":"movie","name":"Movie {i}","poster":"https://img.example/{i}.jpg"}}"#))
                    .collect();
                format!(r#"{{"metas":[{}]}}"#, metas.join(","))
            } else if url.contains("/stream/") {
                r#"{"streams":[{"url":"https://cdn.example/a.mp4","name":"1080p"},{"infoHash":"0123456789abcdef0123456789abcdef01234567"}]}"#.to_string()
            } else if url.contains("/meta/") {
                r#"{"meta":{"id":"tt0","type":"movie","name":"Movie 0"}}"#.to_string()
            } else {
                return Err(FetchError::Status(404));
            };
            Ok(body.into_bytes())
        }
        fn post(&self, _: &str, _: &[(String, String)], _: &[u8]) -> Result<Vec<u8>, FetchError> {
            Err(FetchError::Status(405))
        }
    }

    /// Records the jobs; the test lands them through the fake transport.
    #[derive(Default)]
    struct Scripted {
        pending: std::sync::Mutex<Vec<Done>>,
    }
    impl Runner for std::sync::Arc<Scripted> {
        fn run(&self, _what: &'static str, _job: Box<dyn FnOnce(&OffFrame) -> Done + Send>, _tx: Sender<Done>, fail: Done) {
            // `fail` names the job (its key and kind); the test resolves it against the fake.
            self.pending.lock().unwrap().push(fail);
        }
    }

    fn key(k: Key, edge: Edge, ms: u32) -> InputEvent<u32> {
        InputEvent { at: Tick { ms, dt_us: 0 }, source: Source::Script, kind: InputKind::Key { key: k, sym: 0, wcode: 0, edge, at_edge: false } }
    }

    /// Boot to Home over two addons: the manifests land, the first rows' pages land, the home
    /// shows cards, OK opens a Detail page whose stream groups load per addon.
    #[test]
    fn home_rows_fill_from_the_installed_addons_and_ok_reaches_the_streams() {
        let _g = sn_base::testlock::serial();
        let net = FakeNet { seen: Default::default() };
        let scripted = std::sync::Arc::new(Scripted::default());
        let catalog = CatalogStore::new(["https://addons.example/cinemeta/manifest.json", "https://addons.example/streamer/manifest.json"]);
        // SAFETY: the test thread stands in for the main thread; nothing native is reached.
        let playback = Playback::new(unsafe { MainThread::assume() });
        let mut bridge = Bridge::with_runner(catalog, playback, Box::new(sn_ui::fixture::FixtureMeasure), None, Box::new(scripted.clone()));
        let mut pages: Dispatcher<Bundle> = Dispatcher::new();
        pages.request(MachineId::Nav, NavOp::Root(AppArg::Home));
        bridge.start_catalog_jobs();

        let mut ms = 0u32;
        let frame = |bridge: &mut Bridge, pages: &mut Dispatcher<Bundle>, inputs: Vec<InputEvent<u32>>, ms: &mut u32| {
            // the workers' half, against the fake transport
            let jobs: Vec<Done> = std::mem::take(&mut *scripted.pending.lock().unwrap());
            for done in jobs {
                match done {
                    Done::Catalog(Landing::Manifest { source, .. }) => {
                        let url = bridge.catalog.sources()[source].transport_url.clone();
                        bridge.catalog.land(Landing::Manifest { source, result: net.get(&url, &[]) });
                    }
                    Done::Catalog(Landing::Page { row, .. }) => {
                        let r = bridge.catalog.row(row).unwrap();
                        let url = r.pager.next_url().unwrap();
                        let result = sn_stremio::get_json(&net, &url, &[]);
                        bridge.catalog.land(Landing::Page { row, result });
                    }
                    Done::Detail(landing) => {
                        let job = match landing {
                            sn_screens::stores::detail::DetailLanding::Meta { key, .. } => {
                                let url = format!("https://addons.example/cinemeta/meta/{}/{}.json", key.kind, key.id);
                                sn_screens::stores::detail::DetailJob::Meta { key, url }
                            }
                            sn_screens::stores::detail::DetailLanding::Streams { key, index, .. } => {
                                let group = bridge.detail.streams(&key).unwrap()[index].clone();
                                sn_screens::stores::detail::DetailJob::Streams { key, index, group }
                            }
                        };
                        bridge.detail.land(crate::work::run_detail_with(&net, job));
                    }
                    Done::Search(sn_screens::stores::search::SearchLanding { qgen, row, .. }) => {
                        let url = bridge.search.rows()[row].pager.next_url().unwrap();
                        let result = sn_stremio::get_json(&net, &url, &[]);
                        bridge.search.land(sn_screens::stores::search::SearchLanding { qgen, row, result });
                    }
                    Done::Grid(sn_screens::stores::grid::GridLanding { key, .. }) => {
                        let url = bridge.grid.next_url(&key).unwrap();
                        let result = sn_stremio::get_json(&net, &url, &[]);
                        bridge.grid.land(sn_screens::stores::grid::GridLanding { key, result });
                    }
                }
            }
            bridge.start_catalog_jobs();
            bridge.notify(pages);
            *ms += 16;
            sn_machine::idle::frame_begin(1.0 / 60.0);
            let report = pages.frame_with(bridge, Tick { ms: *ms, dt_us: 16_000 }, inputs, Vec::new(), &mut NoTap, false);
            pages.draw(bridge, true);
            pages.prune(&report.unmounted);
        };
        for _ in 0..30 {
            frame(&mut bridge, &mut pages, Vec::new(), &mut ms);
        }
        assert_eq!(bridge.catalog.addons_state(), AddonsState::Ready);
        assert_eq!(bridge.catalog.rows().len(), 2, "Cinemeta's two browsable catalogs; the stream-only addon has none");
        assert!(bridge.catalog.rows()[0].items().len() == 10, "the first row's page landed");
        let seen = net.seen.lock().unwrap().clone();
        assert!(seen.iter().any(|u| u.contains("/catalog/movie/top")), "{seen:?}");

        // OK on the seated card: Detail opens, the meta and the per-addon stream groups load
        for (k, e) in [(Key::Ok, Edge::Down), (Key::Ok, Edge::Up)] {
            frame(&mut bridge, &mut pages, vec![key(k, e, ms)], &mut ms);
        }
        for _ in 0..40 {
            frame(&mut bridge, &mut pages, Vec::new(), &mut ms);
        }
        assert!(matches!(pages.top_arg(), Some(AppArg::Detail(_))), "OK opened the item: {:?}", pages.top_arg().map(|_| ()));
        let stream_key = sn_screens::stores::detail::StreamKey { kind: "movie".into(), video_id: "tt0".into() };
        let groups = bridge.detail.streams(&stream_key).expect("a movie opens its streams at once");
        assert_eq!(groups.len(), 1, "one group per addon serving streams");
        assert_eq!(groups[0].addon_name, "Streamer");
        match &groups[0].state {
            sn_stremio::streams::GroupState::Ready(streams) => assert_eq!(streams.len(), 2, "both streams listed (one not playable here)"),
            other => panic!("the group loaded: {other:?}"),
        }
    }

    /// A Settings pick reaches the player at once and the settings file off the frame thread, and
    /// the search filter and recents ride in the same file.
    #[test]
    fn a_settings_change_is_published_to_the_player_and_written() {
        use sn_screens::registry::SettingsCmd;
        use sn_screens::stores::settings::Pref;
        let _g = sn_base::testlock::serial();
        let dir = sn_base::testscratch::process_dir("bridge-settings");
        let path = dir.join("settings.json");
        // SAFETY: as above.
        let playback = Playback::new(unsafe { MainThread::assume() });
        let loaded = super::super::prefs::load(&path);
        let mut bridge = Bridge::with_runner(
            CatalogStore::new(Vec::<String>::new()),
            playback,
            Box::new(sn_ui::fixture::FixtureMeasure),
            None,
            Box::new(std::sync::Arc::new(Scripted::default())),
        )
        .with_settings(loaded, path.clone());
        bridge.settings_cmd(SettingsCmd::Set(Pref::SubtitleSize, 4));
        assert_eq!(sn_media::prefs::subtitle_size(), sn_media::prefs::SubtitleSize::Huge, "the player reads it at once");
        bridge.search_cmd(sn_screens::registry::SearchCmd::Remember("batman".into()));
        sn_base::storage_worker::drain_for_test();
        let back = super::super::prefs::load(&path);
        assert_eq!(back.settings.get(Pref::SubtitleSize), 4);
        assert_eq!(back.search.recents(), ["batman"]);
        bridge.settings_cmd(SettingsCmd::Set(Pref::SubtitleSize, 1));
        sn_base::storage_worker::drain_for_test();
    }

    #[test]
    fn background_parks_the_tree_and_foreground_wakes_it() {
        let _g = sn_base::testlock::serial();
        let mut pages: Dispatcher<Bundle> = Dispatcher::new();
        let catalog = CatalogStore::new(Vec::<String>::new());
        // SAFETY: as above.
        let playback = Playback::new(unsafe { MainThread::assume() });
        let mut bridge = Bridge::with_runner(catalog, playback, Box::new(sn_ui::fixture::FixtureMeasure), None, Box::new(std::sync::Arc::new(Scripted::default())));
        pages.request(MachineId::Nav, NavOp::Root(AppArg::Home));
        pages.frame(&mut bridge, Tick { ms: 16, dt_us: 16_000 }, Vec::new(), Vec::new(), &mut NoTap);
        background(&mut pages);
        pages.frame(&mut bridge, Tick { ms: 32, dt_us: 16_000 }, Vec::new(), Vec::new(), &mut NoTap);
        assert!(pages.nav.suspended);
        foreground(&mut pages);
        foreground(&mut pages);
        pages.frame(&mut bridge, Tick { ms: 48, dt_us: 16_000 }, Vec::new(), Vec::new(), &mut NoTap);
        assert!(!pages.nav.suspended, "both foreground edges are safe");
        assert!(!player_on_top(&pages));
    }
}
