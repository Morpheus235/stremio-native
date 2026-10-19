//! **The bridge**: the application's `Rig` for the dispatcher — the stores the screens read, the
//! effects they ask for executed against the workers and the player, the shared top bar
//! (`sn_screens::chrome`), the television keyboard, the settings file, the render cache's upload
//! step, and the three privileged platform calls.
//!
//! **The installed addons are read through ONE accessor, [`Bridge::addons`]** — Search plans
//! against them, Discover and the grids resolve against them, Detail asks their meta and streams.
//! They are the catalog store's, which `Bridge::follow_addons` rebuilds from
//! `AddonsStore::in_use` — the account's collection signed in, the defaults signed out — so every
//! page switches with the session (`Views::addons` is its screen-side twin).
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
    AddonsCmd, AppArg, AppFx, AppMounter, AppMsg, Bundle, CatalogCmd, DetailCmd, GridCmd, LibraryCmd, PlayerArg, SearchCmd,
    SessionView, SettingsCmd, Views,
};
use sn_screens::stores::account::{AccountJob, AccountLanding, AccountStore, SessionState};
use sn_screens::stores::addons::{AddonsJob, AddonsStore};
use sn_screens::stores::catalog::CatalogStore;
use sn_screens::stores::detail::DetailStore;
use sn_screens::stores::grid::GridStore;
use sn_screens::stores::library::{LibraryJob, LibraryStore};
use sn_screens::stores::search::SearchStore;
use sn_screens::stores::settings::SettingsStore;
use sn_screens::stores::StoreId;
use sn_stremio::manifest::Addon;
use sn_ui::dispatch::{CxParts, Dispatcher, Rig, Split};
use sn_ui::frame::Budget;
use sn_ui::tex::{self, Decoded, Tex, Uploader};

use super::playback::Playback;
use crate::account::Services;
use crate::art::Art;
use crate::work::{Done, Runner, Threads, Work};

/// The stores the tree hears about, in [`Bridge::notify`]'s order.
const TOLD: [StoreId; 8] = [
    StoreId::Catalog,
    StoreId::Detail,
    StoreId::Search,
    StoreId::Grid,
    StoreId::Settings,
    StoreId::Account,
    StoreId::Addons,
    StoreId::Library,
];

pub struct Bridge {
    mounter: AppMounter,
    pub catalog: CatalogStore,
    pub detail: DetailStore,
    pub account: AccountStore,
    /// THE addons in use (`AddonsStore::in_use`); the catalog store is rebuilt from it.
    pub addons: AddonsStore,
    pub library: LibraryStore,
    pub search: SearchStore,
    pub grid: GridStore,
    pub settings: SettingsStore,
    pub chrome: ChromeBar,
    pub playback: Playback,
    measure: Box<dyn sn_machine::machine::Measure>,
    work: Work,
    runner: Box<dyn Runner>,
    services: Services,
    art: Option<&'static Art>,
    /// The generation each store last announced to the tree ([`TOLD`]'s order).
    told: [Option<u32>; 8],
    /// The playback view changed since the tree was last told.
    playback_moved: bool,
    /// The last stream started, for the failure read-out's *Try again*.
    last_play: Option<sn_screens::registry::PlayRequest>,
    /// The addons store's epoch the catalog store was last built from.
    built_epoch: u32,
    /// The frame clock: the search store's debounce, and what a landed sign-in code is timed from.
    now_ms: u32,
    /// Navigation the bridge itself asks for (the first-run sign-in), sent at the next notify.
    nav: Vec<NavOp<AppArg>>,
    first_run_offered: bool,
    /// A sign-out reset the pages to Home; the sign-in page follows once Home stands.
    sign_in_after_root: bool,
    /// Where the settings are persisted (`None`: not at all, a host test).
    settings_path: Option<PathBuf>,
    /// The `(settings, search settings)` generations last written.
    saved: (u32, u32),
    /// The television keyboard's raises and dismissals (the tests' record of `system_keyboard`).
    #[cfg(test)]
    pub keyboard: Vec<bool>,
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
            // a bridge that is never booted is a signed-out one; `boot` reads the stored session
            account: AccountStore::signed_out(true),
            addons: AddonsStore::new(),
            library: LibraryStore::new(),
            search: SearchStore::default(),
            grid: GridStore::new(),
            settings: SettingsStore::default(),
            chrome: ChromeBar::new(),
            playback,
            measure,
            work: Work::new(),
            runner,
            services: Services::device(),
            art,
            told: [None; 8],
            playback_moved: false,
            last_play: None,
            built_epoch: 0,
            now_ms: 0,
            nav: Vec::new(),
            first_run_offered: false,
            sign_in_after_root: false,
            settings_path: None,
            saved: (0, 0),
            #[cfg(test)]
            keyboard: Vec::new(),
        }
    }

    /// Replace what the account's jobs reach (the tests' fakes).
    pub fn with_services(mut self, services: Services) -> Self {
        self.services = services;
        self
    }

    /// Boot: read the stored session and the search switches; the catalog starts on the defaults
    /// and is rebuilt when the account's addons arrive.
    pub fn boot(&mut self) {
        self.account = AccountStore::new();
        let jobs = self.account.boot();
        self.run_account(jobs);
        let jobs = self.addons.boot();
        self.run_addons(jobs);
        self.start_catalog_jobs();
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

    /// **The addons the app uses**, manifests loaded — the one accessor every lookup plans
    /// against (Home's rows, Search, Discover, the grids, Detail's meta and streams). Built from
    /// `AddonsStore::in_use` (the account's collection signed in, the defaults otherwise);
    /// `Views::addons` is its screen-side twin.
    pub fn addons(&self) -> &[Addon] {
        self.catalog.addons()
    }

    /// The frame's clock: the search store's debounce, and the settings written when they moved.
    pub fn tick(&mut self, now_ms: u32) {
        self.now_ms = now_ms;
        let jobs = self.search.tick(now_ms, self.catalog.addons(), self.addons.filter());
        if !jobs.is_empty() {
            self.work.search(&*self.runner, jobs);
        }
        self.persist();
    }

    /// *Try again*: the last stream once more, from where it stopped (its own start when it never
    /// got under way), over the player page that is already up.
    fn retry(&mut self) {
        let Some(mut req) = self.last_play.clone() else { return };
        let pos = self.playback.view.clock.pos_ms;
        if pos > 0 {
            req.start_ms = pos;
        }
        self.playback.start(&req, self.settings.track_prefs());
        self.playback.set_audio_offset(self.settings.audio_offset(&audio_key(&req)));
        self.ask_subtitles(&req);
        self.playback_moved = true;
    }

    /// Write the settings file when the settings or the recent searches moved.
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
        self.chrome.capture(pages, &self.account, &*self.measure);
    }

    /// Step the bar's motion after the frame, and publish the transition's presentation for the
    /// readers outside a page's own frame (the bar, the popovers).
    pub fn update_chrome(&mut self, pages: &mut Dispatcher<Bundle>, glass: &mut sn_ui::frame::glass::GlassPlan, dt: f32) {
        self.chrome.update(pages, Some(glass), dt);
        let tab = pages.nav.tabs.stack.pending_dest().and_then(sn_screens::tabs::Tab::of_arg).and_then(sn_screens::tabs::Tab::index);
        sn_ui::nav::publish(pages.nav.tabs.stack.page_alpha(), pages.nav.tabs.stack.chrome_alpha(), tab);
    }

    /// Start whatever the catalog store wants now (its manifests at boot, its pages after a view
    /// change or a landing freed a slot).
    pub fn start_catalog_jobs(&mut self) {
        let jobs = self.catalog.take_jobs();
        if !jobs.is_empty() {
            self.work.catalog(&*self.runner, self.catalog.epoch(), jobs);
        }
    }

    fn run_account(&mut self, jobs: Vec<AccountJob>) {
        if !jobs.is_empty() {
            self.work.account(&*self.runner, &self.services, jobs);
        }
    }

    fn run_addons(&mut self, jobs: Vec<AddonsJob>) {
        if !jobs.is_empty() {
            self.work.addons(&*self.runner, &self.services, jobs);
        }
    }

    fn run_library(&mut self, jobs: Vec<LibraryJob>) {
        if !jobs.is_empty() {
            self.work.library(&*self.runner, &self.services, jobs);
        }
    }

    /// A page's word to the account (sign in, the device link, sign out…). Settings' sign-out
    /// sends `AccountCmd::SignOut` through the same door.
    pub fn account_cmd(&mut self, cmd: sn_screens::stores::account::AccountCmd) {
        let jobs = self.account.command(cmd);
        self.run_account(jobs);
        self.follow_account();
    }

    /// A page's word to the addons store.
    pub fn app_addons(&mut self, cmd: AddonsCmd) {
        let jobs = match cmd {
            AddonsCmd::ToggleSearch(id) => self.addons.toggle_search(&id),
            AddonsCmd::Retry => self.addons.retry(),
            AddonsCmd::ToggleStreams(id) => {
                let jobs = self.addons.toggle_streams(&id);
                // the stream lists open now drop the addon's answer, or ask it again
                let asks = self.detail.set_addon(&id, self.addons.stream_filter().allows(&id));
                if !asks.is_empty() {
                    self.work.detail(&*self.runner, asks);
                }
                jobs
            }
        };
        self.run_addons(jobs);
    }

    /// Fold one account landing in (and what it set off).
    pub fn land_account(&mut self, landing: AccountLanding) {
        let jobs = self.account.land(landing, self.now_ms);
        self.run_account(jobs);
        self.follow_account();
    }

    /// The account moved: the addons and the library follow it, and a first run is offered the
    /// sign-in page once.
    fn follow_account(&mut self) {
        match self.account.session().clone() {
            SessionState::SignedIn(a) => {
                if !self.addons.signed_in() {
                    log("account: signed in");
                }
                let jobs = self.addons.sign_in(&a.user_id, &a.auth_key);
                self.run_addons(jobs);
                let jobs = self.library.sign_in(&a.user_id, &a.auth_key);
                self.run_library(jobs);
            }
            SessionState::SignedOut { .. } => {
                if self.addons.signed_in() {
                    log("account: signed out");
                    self.leave_account();
                }
                let jobs = self.addons.sign_out();
                self.run_addons(jobs);
                self.library.sign_out();
                if self.account.wants_first_run_sign_in() && !std::mem::replace(&mut self.first_run_offered, true) {
                    self.nav.push(NavOp::Push(AppArg::SignIn));
                }
            }
            SessionState::Restoring => {}
        }
        self.follow_addons();
    }

    /// **A sign-out, the moment it happens** (the account's own state is already gone: the store
    /// ended the session and asked for the stored copy to be removed). What plays stops, and what
    /// it would have recorded is dropped — a sign-out writes nothing to the account beyond the
    /// API's logout. The addons and the library follow in [`Self::follow_account`] (back to the
    /// defaults, Continue Watching empty, the collection's device copy removed). The pages are
    /// reset to Home, and once Home stands the sign-in page is offered over it, as on a first run.
    fn leave_account(&mut self) {
        self.playback.stop();
        let _ = self.playback.take_progress();
        self.nav.push(NavOp::Root(AppArg::Home));
        self.sign_in_after_root = true;
    }

    /// The addons in use changed: the catalog store is rebuilt from them (rows the new list keeps
    /// keep what they loaded), and asks for what it now wants.
    fn follow_addons(&mut self) {
        if self.addons.epoch() == self.built_epoch {
            return;
        }
        self.built_epoch = self.addons.epoch();
        let next = CatalogStore::for_in_use(self.addons.in_use());
        log(&format!("addons: {} in use", next.sources().len()));
        self.catalog.replace(next);
        self.start_catalog_jobs();
        // a query on screen is asked of the addons now in use
        let jobs = self.search.replan(self.catalog.addons(), self.addons.filter());
        if !jobs.is_empty() {
            self.work.search(&*self.runner, jobs);
        }
    }

    /// A frame was drawn: the art source ages its draws by drawn frames ([`Art::drawn_frame`]).
    pub fn art_drawn(&self) {
        if let Some(art) = self.art {
            art.drawn_frame();
        }
    }

    /// Fold every worker outcome in, then start what the landings made possible. `true` when a
    /// store changed.
    pub fn land(&mut self) -> bool {
        let mut changed = false;
        for done in self.work.drain() {
            changed |= match done {
                // a landing for an install list that has since been replaced names nothing now
                Done::Catalog(epoch, l) => epoch == self.catalog.epoch() && self.catalog.land(l),
                Done::Detail(l) => self.detail.land(l),
                Done::Account(l) => {
                    self.land_account(l);
                    true
                }
                Done::Addons(Some(l)) => {
                    let jobs = self.addons.land(l);
                    self.run_addons(jobs);
                    self.follow_addons();
                    true
                }
                Done::Addons(None) => false,
                Done::Library(l) => {
                    let (moved, jobs) = self.library.land(l);
                    self.run_library(jobs);
                    self.toast_refused();
                    moved
                }
                Done::Search(l) => self.search.land(l),
                Done::Grid(l) => self.grid.land(l),
                Done::Skips { gen, source, result } => {
                    // one line per lookup: the service and what it knew, never the URL
                    match &result {
                        Ok(s) if s.is_empty() => log(&format!("skip: {source} not found")),
                        Ok(s) => log(&format!("skip: {source} found {}", sn_stremio::skip::describe(s))),
                        Err(e) => log(&format!("skip: {source} failed ({e})")),
                    }
                    self.playback.land_skips(gen, result.unwrap_or_default());
                    false
                }
                Done::Subtitles { gen, addon, result } => {
                    // a failure is that addon's nothing; the line was logged on the worker
                    let landed = self.playback.land_subtitles(gen, &addon, result.unwrap_or_default());
                    self.playback_moved |= landed;
                    false
                }
            };
        }
        if let Some(art) = self.art {
            art.drain();
        }
        self.start_catalog_jobs();
        changed
    }

    /// Ask every subtitles addon in use for what `req` plays (nothing for a playback that does not
    /// know what it is of).
    fn ask_subtitles(&mut self, req: &sn_screens::registry::PlayRequest) {
        let Some(item) = &req.item else { return };
        let sources = sn_stremio::subtitles::plan(self.catalog.addons(), &item.kind, &item.video_id, &req.hints);
        if !sources.is_empty() {
            self.work.subtitles(&*self.runner, self.playback.subs.gen, sources);
        }
    }

    /// The frame's playback pump; the video plane's edge goes to the present gate.
    pub fn pump_playback(&mut self, now_ms: u32, present: &mut Present) {
        self.now_ms = now_ms;
        let (moved, edge) = self.playback.tick(now_ms);
        self.playback_moved |= moved;
        // Skip intros Off asks no marker service anything (PRIVACY.md)
        let skip_on = self.settings.get(sn_screens::stores::settings::Pref::SkipIntros) != sn_screens::stores::settings::SKIP_OFF;
        let detail = &self.detail;
        let imdb = |i: &sn_screens::registry::PlayItem| match detail.meta(&sn_screens::stores::detail::MetaKey { kind: i.kind.clone(), id: i.id.clone() }) {
            Some(sn_screens::stores::detail::MetaState::Ready(m)) => m.imdb().map(str::to_owned),
            _ => None,
        };
        if let Some(job) = self.playback.take_skip_lookup(imdb).filter(|_| skip_on) {
            self.work.skips(&*self.runner, job);
        }
        if let Some(bound) = edge {
            present.note(sn_machine::present::PresentEvent::VideoPlane(bound));
        }
        self.send_progress();
    }

    /// A library write the account refused (already rolled back by the store): the television's
    /// toast says so, once.
    fn toast_refused(&mut self) {
        use sn_screens::stores::library::Refused;
        for refused in self.library.take_failed() {
            let text = match refused {
                Refused::Add => sn_platform::i18n::msg::app_menu_add_failed(),
                Refused::Remove => sn_platform::i18n::msg::app_menu_remove_failed(),
                Refused::Rewind => sn_platform::i18n::msg::app_menu_rewind_failed(),
                Refused::Mark => sn_platform::i18n::msg::app_watched_mark_failed(),
            }
            .to_string();
            sn_base::task::spawn("toast", move || {
                let _ = sn_platform::tv::toast::toast(&text);
            });
        }
    }

    /// Record where the playback stands in the account's library (signed out: nothing). The work's
    /// poster, for an entry the library does not hold yet, is the meta's a page holds open (the
    /// Detail page under the player, or the player's own for an episode). So are its videos, which
    /// the watched bit is laid over (stremio-core's `Player::watched`; no meta in, no bit).
    fn send_progress(&mut self) {
        for (mut p, last) in self.playback.take_progress() {
            let key = sn_screens::stores::detail::MetaKey { kind: p.kind.clone(), id: p.id.clone() };
            if let Some(sn_screens::stores::detail::MetaState::Ready(m)) = self.detail.meta(&key) {
                if p.poster.is_none() {
                    p.poster = m.preview.poster.clone();
                }
                p.videos = Some(m.videos.clone());
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            let jobs = self.library.progress(p, last, &sn_stremio::library::rfc3339_ms(now));
            self.run_library(jobs);
        }
    }

    /// Tell the tree which stores moved since it was last told, and send the navigation the
    /// bridge asked for.
    pub fn notify(&mut self, pages: &mut Dispatcher<Bundle>) {
        let gens = [
            self.catalog.gen(),
            self.detail.gen(),
            self.search.gen(),
            self.grid.gen(),
            self.settings.gen(),
            self.account.gen(),
            self.addons.gen(),
            self.library.gen(),
        ];
        for (i, (id, gen)) in TOLD.into_iter().zip(gens).enumerate() {
            if self.told[i] != Some(gen) {
                self.told[i] = Some(gen);
                pages.store_changed(id.ord(), gen);
            }
        }
        if std::mem::take(&mut self.playback_moved) {
            pages.store_changed(StoreId::Playback.ord(), 0);
        }
        // a menu the player presents is a panel over the video plane: its host stays live (the
        // clock, the subtitles), never a frozen snapshot of a picture GL cannot read
        pages.nav.next_style = if player_on_top(pages) {
            sn_ui::containers::modal::Style::PlayerPanel { survives_failure: false }
        } else {
            sn_ui::containers::modal::Style::Compact
        };
        // pushed over the root, never in its place: held until the boot's root page is up
        if pages.top_arg().is_some() {
            for op in std::mem::take(&mut self.nav) {
                pages.request(MachineId::Nav, op);
            }
        }
        // a sign-out's sign-in page: over the Home it reset to, once that Home stands
        if self.sign_in_after_root && self.nav.is_empty() && pages.nav.modals.surfaces.is_empty() && pages.nav.tabs.stack.root_settled(&AppArg::Home) {
            self.sign_in_after_root = false;
            pages.request(MachineId::Nav, NavOp::Push(AppArg::SignIn));
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
        // a chip switches the addon in THE filter (the addons store's, which the Addons page's
        // Include in search switch also toggles and which is persisted), then the query is asked
        // again over the new set
        if let SearchCmd::ToggleAddon(id) = &cmd {
            let jobs = self.addons.toggle_search(id);
            self.run_addons(jobs);
        }
        let jobs = {
            let addons = self.catalog.addons();
            let filter = self.addons.filter();
            match cmd {
                SearchCmd::Type(text) => {
                    self.search.type_query(&text, self.now_ms);
                    Vec::new()
                }
                SearchCmd::Submit(text) => self.search.submit(&text, addons, filter),
                SearchCmd::ToggleAddon(_) => self.search.replan(addons, filter),
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
                // a search or Home row's grid opens on the cards the row already holds
                let seed = self.search.pager_for(&key).or_else(|| self.catalog.pager_for(&key)).cloned();
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
            SettingsCmd::List(page, on) => {
                self.settings.set_list(page, on);
            }
            SettingsCmd::Languages(l, op) => {
                self.settings.edit(l, op);
            }
            SettingsCmd::LibrarySort(sort) => {
                self.settings.set_library_sort(sort);
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
            DetailCmd::OpenStreams(key) => self.detail.open_streams(self.catalog.addons(), self.addons.stream_filter(), key),
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

/// The stream source an audio sync is remembered for: the item's id — a live channel's own id
/// (TvVoo's `vavoo_…`), else the work's (one offset for a show's episodes) — and nothing for a
/// playback of no item.
pub fn audio_key(req: &sn_screens::registry::PlayRequest) -> String {
    req.item.as_ref().map(|i| i.id.clone()).unwrap_or_default()
}

impl Rig<Bundle> for Bridge {
    fn split(&mut self) -> Split<'_, Bundle> {
        Split {
            mounter: &mut self.mounter,
            views: Views {
                catalog: &self.catalog,
                detail: &self.detail,
                playback: &self.playback.view,
                account: &self.account,
                addons: &self.addons,
                library: &self.library,
                search: &self.search,
                grid: &self.grid,
                settings: &self.settings,
                session: SessionView { signed_in: self.account.account().is_some() },
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
                self.playback.start(&req, self.settings.track_prefs());
                self.playback.set_audio_offset(self.settings.audio_offset(&audio_key(&req)));
                self.ask_subtitles(&req);
                self.playback_moved = true;
                out.push(Fx::Nav(NavOp::Push(AppArg::Player(PlayerArg::of(&req)))));
                self.last_play = Some(req);
            }
            AppFx::Player(sn_screens::registry::PlayerCmd::Retry) => self.retry(),
            AppFx::Player(sn_screens::registry::PlayerCmd::AudioDelay(ms)) => {
                let ms = sn_media::player::clamp_audio_offset_ms(ms);
                self.playback.set_audio_offset(ms);
                if let Some(req) = &self.last_play {
                    self.settings.set_audio_offset(&audio_key(req), ms);
                    self.persist();
                }
                self.playback_moved = true;
            }
            AppFx::Player(cmd) => {
                self.playback.command(cmd);
                self.playback_moved = true;
                self.send_progress();
            }
            AppFx::Account(cmd) => self.account_cmd(cmd),
            AppFx::Addons(cmd) => {
                self.app_addons(cmd);
                // the Addons page's switch is the search filter: the shown query follows it
                let jobs = self.search.replan(self.catalog.addons(), self.addons.filter());
                if !jobs.is_empty() {
                    self.work.search(&*self.runner, jobs);
                }
            }
            AppFx::Toast(text) => {
                sn_base::task::spawn("toast", move || {
                    let _ = sn_platform::tv::toast::toast(&text);
                });
            }
            AppFx::Library(LibraryCmd::Reload) => {
                let jobs = self.library.reload();
                self.run_library(jobs);
            }
            AppFx::Library(LibraryCmd::Add(entry)) => {
                let jobs = self.library.add(entry);
                self.run_library(jobs);
            }
            AppFx::Library(LibraryCmd::Remove(id)) => {
                let jobs = self.library.remove(&id);
                self.run_library(jobs);
            }
            AppFx::Library(LibraryCmd::Rewind(id)) => {
                let jobs = self.library.rewind(&id);
                self.run_library(jobs);
            }
            AppFx::Library(LibraryCmd::Mark(entry, edit)) => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                let jobs = self.library.mark(entry, edit, &sn_stremio::library::rfc3339_ms(now));
                self.run_library(jobs);
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
        self.keyboard.push(up);
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

/// Is the Search page on top, with no menu over it?
pub fn search_on_top(pages: &Dispatcher<Bundle>) -> bool {
    matches!(pages.top_arg(), Some(AppArg::Search)) && pages.nav.modals.surfaces.is_empty()
}

/// Is a page with a text field (Search, Sign in) on top, with no menu over it? Text goes only
/// to one of those.
pub fn text_field_on_top(pages: &Dispatcher<Bundle>) -> bool {
    matches!(pages.top_arg(), Some(AppArg::Search | AppArg::SignIn)) && pages.nav.modals.surfaces.is_empty()
}

#[cfg(test)]
mod tests {
    /// The audio sync is remembered per stream source: a live channel by its own id (TvVoo's
    /// `vavoo_…`), anything else by its item; a playback of no item remembers nothing.
    #[test]
    fn the_audio_sync_key_is_the_channel_or_the_item() {
        use sn_screens::registry::{PlayItem, PlayRequest};
        let req = |item: Option<PlayItem>| PlayRequest {
            url: String::new(),
            headers: Vec::new(),
            title: String::new(),
            start_ms: 0,
            addon: String::new(),
            item,
            hints: Default::default(),
        };
        let item = |kind: &str, id: &str, video: &str| PlayItem { kind: kind.into(), id: id.into(), name: String::new(), video_id: video.into(), episode: String::new(), release: None };
        assert_eq!(super::audio_key(&req(Some(item("tv", "vavoo_SKY1", "vavoo_SKY1")))), "vavoo_SKY1");
        assert_eq!(super::audio_key(&req(Some(item("series", "tt9", "tt9:1:2")))), "tt9", "one offset for a show's episodes");
        assert_eq!(super::audio_key(&req(None)), "");
    }

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
        sn_screens::test_support::headless_gl();
        let mut pages: Dispatcher<Bundle> = Dispatcher::new();
        pages.request(MachineId::Nav, NavOp::Root(AppArg::Home));
        bridge.start_catalog_jobs();

        let mut ms = 0u32;
        let frame = |bridge: &mut Bridge, pages: &mut Dispatcher<Bundle>, inputs: Vec<InputEvent<u32>>, ms: &mut u32| {
            // the workers' half, against the fake transport
            let jobs: Vec<Done> = std::mem::take(&mut *scripted.pending.lock().unwrap());
            for done in jobs {
                match done {
                    Done::Catalog(_, Landing::Manifest { source, .. }) => {
                        let url = bridge.catalog.sources()[source].transport_url.clone();
                        bridge.catalog.land(Landing::Manifest { source, result: net.get(&url, &[]) });
                    }
                    Done::Catalog(_, Landing::Page { row, .. }) => {
                        let r = bridge.catalog.row(row).unwrap();
                        let url = r.pager.next_url().unwrap();
                        let result = sn_stremio::get_json(&net, &url, &[]);
                        bridge.catalog.land(Landing::Page { row, result });
                    }
                    Done::Detail(landing) => {
                        let job = match landing {
                            sn_screens::stores::detail::DetailLanding::Meta { key, .. } => {
                                let url = format!("https://addons.example/cinemeta/meta/{}/{}.json", key.kind, key.id);
                                sn_screens::stores::detail::DetailJob::Meta { key, urls: vec![url] }
                            }
                            sn_screens::stores::detail::DetailLanding::Streams { key, index, .. } => {
                                let group = bridge.detail.streams(&key).unwrap()[index].clone();
                                sn_screens::stores::detail::DetailJob::Streams { key, index, group }
                            }
                        };
                        bridge.detail.land(crate::work::run_detail_with(&net, job));
                    }
                    Done::Account(_) | Done::Addons(_) | Done::Library(_) | Done::Skips { .. } | Done::Subtitles { .. } => {}
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
        sn_screens::test_support::headless_gl();
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

    // -----------------------------------------------------------------------------------------
    // The account, end to end: the real stores, the real dispatcher and screens, the account's
    // jobs run inline against a fake API, vault and data directory; addon requests are served
    // from the fake addons.

    use crate::account::tests::{FakeApi, MemFiles, MemVault};
    use crate::account::{collection_file, Files, Services};
    use sn_machine::machine::TextEdit;
    use sn_screens::stores::account::SessionState;
    use sn_screens::stores::addons::InUse;
    use std::sync::Arc;

    /// Runs the account, addons and library jobs at once (against the fakes in `Services`); keeps
    /// the addon requests for the test to serve.
    struct Inline(Arc<Scripted>);
    impl Runner for Inline {
        fn run(&self, what: &'static str, job: Box<dyn FnOnce(&OffFrame) -> Done + Send>, tx: Sender<Done>, fail: Done) {
            if what.starts_with("addon-") {
                self.0.pending.lock().unwrap().push(fail);
            } else {
                let _ = tx.send(job(&OffFrame::for_test()));
            }
        }
    }

    const COLLECTION: &str = r#"{"result":{"addons":[
        {"transportUrl":"https://addons.example/live/cfg-SECRET/manifest.json","manifest":{
            "id":"org.example.live","version":"2.1.0","name":"Live TV","resources":["catalog","stream"],"types":["tv"],
            "behaviorHints":{"configurable":true},
            "catalogs":[{"type":"tv","id":"channels","name":"Channels","extra":[{"name":"search"}]}]}},
        {"transportUrl":"https://addons.example/cinemeta/manifest.json","manifest":{
            "id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta","resources":["catalog","meta"],
            "types":["movie","series"],"idPrefixes":["tt"],
            "catalogs":[{"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]}]}}]}}"#;

    const LIBRARY: &str = r#"{"result":[
        {"_id":"tt0903747","type":"series","name":"Breaking Bad","poster":"https://img.example/bb.jpg",
         "state":{"timeOffset":1260000,"duration":2820000,"video_id":"tt0903747:2:5","lastWatched":"2026-10-01T00:00:00Z"}},
        {"_id":"tt0111161","type":"movie","name":"The Shawshank Redemption","state":{"timeOffset":0}}]}"#;

    struct Rigged {
        bridge: Bridge,
        pages: Dispatcher<Bundle>,
        api: Arc<FakeApi>,
        vault: Arc<MemVault>,
        files: Arc<MemFiles>,
        addons: FakeNet,
        scripted: Arc<Scripted>,
        ms: u32,
    }

    impl Rigged {
        fn new(api: FakeApi, vault: MemVault, files: MemFiles) -> Self {
            let (api, vault, files) = (Arc::new(api), Arc::new(vault), Arc::new(files));
            let scripted = Arc::new(Scripted::default());
            // SAFETY: the test thread stands in for the main thread; nothing native is reached.
            let playback = Playback::new(unsafe { MainThread::assume() });
            let services = Services { net: api.clone(), vault: vault.clone(), files: files.clone() };
            let bridge = Bridge::with_runner(
                CatalogStore::new(["https://addons.example/cinemeta/manifest.json"]),
                playback,
                Box::new(sn_ui::fixture::FixtureMeasure),
                None,
                Box::new(Inline(scripted.clone())),
            )
            .with_services(services);
            sn_screens::test_support::headless_gl();
            let mut pages: Dispatcher<Bundle> = Dispatcher::new();
            pages.request(MachineId::Nav, NavOp::Root(AppArg::Home));
            let mut r = Rigged { bridge, pages, api, vault, files, addons: FakeNet { seen: Default::default() }, scripted, ms: 0 };
            r.bridge.boot();
            r
        }

        /// One frame: the addon requests answered, the landings folded in, the tree told, a frame.
        fn frame(&mut self, inputs: Vec<InputEvent<u32>>) {
            for done in std::mem::take(&mut *self.scripted.pending.lock().unwrap()) {
                if let Done::Catalog(epoch, landing) = done {
                    if epoch != self.bridge.catalog.epoch() {
                        continue;
                    }
                    match landing {
                        Landing::Manifest { source, .. } => {
                            let url = self.bridge.catalog.sources()[source].transport_url.clone();
                            self.bridge.catalog.land(Landing::Manifest { source, result: self.addons.get(&url, &[]) });
                        }
                        Landing::Page { row, .. } => {
                            let Some(url) = self.bridge.catalog.row(row).and_then(|r| r.pager.next_url()) else { continue };
                            let result = sn_stremio::get_json(&self.addons, &url, &[]);
                            self.bridge.catalog.land(Landing::Page { row, result });
                        }
                    }
                }
            }
            self.bridge.land();
            self.bridge.notify(&mut self.pages);
            self.ms += 16;
            sn_machine::idle::frame_begin(1.0 / 60.0);
            let report = self.pages.frame_with(&mut self.bridge, Tick { ms: self.ms, dt_us: 16_000 }, inputs, Vec::new(), &mut NoTap, false);
            self.pages.draw(&mut self.bridge, true);
            self.bridge.art_drawn();
            self.pages.prune(&report.unmounted);
            self.bridge.notify(&mut self.pages);
        }

        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.frame(Vec::new());
            }
        }

        fn press(&mut self, k: Key) {
            self.frame(vec![key(k, Edge::Down, self.ms + 16)]);
            self.frame(vec![key(k, Edge::Up, self.ms + 16)]);
            self.frames(30);
        }

        /// Typed on the panel: one commit per key, as LG's keyboard sends them.
        fn type_text(&mut self, s: &str) {
            for c in s.chars() {
                let at = Tick { ms: self.ms + 16, dt_us: 0 };
                self.frame(vec![InputEvent { at, source: Source::Sdl, kind: InputKind::Text(TextEdit::Commit(c.to_string().into())) }]);
            }
        }

        fn top(&self) -> &'static str {
            self.pages.top_screen().map_or("", |s| s.name())
        }

        fn row_titles(&self) -> Vec<String> {
            self.bridge.catalog.rows().iter().map(|r| format!("{}/{}", r.addon_name, r.title())).collect()
        }
    }

    fn signing_in_api() -> FakeApi {
        let api = FakeApi::default();
        api.answer("login", r#"{"result":{"authKey":"k-SECRET","user":{"_id":"u1","email":"viewer@example.com"}}}"#);
        api.answer("addonCollectionGet", COLLECTION);
        api.answer("datastoreGet", LIBRARY);
        api.answer("logout", r#"{"result":{}}"#);
        api
    }

    /// The first run: the sign-in page is offered; email and password are typed on the TV's
    /// keyboard; the session is kept; Home's rows switch to the account's addons, in the
    /// collection's order, with Continue Watching; signing out forgets it all and returns to Cinemeta.
    #[test]
    fn signing_in_switches_home_to_the_accounts_addons_and_signing_out_returns_to_the_defaults() {
        let _g = sn_base::testlock::serial();
        let mut r = Rigged::new(signing_in_api(), MemVault::default(), MemFiles::default());
        r.frames(20);
        assert_eq!(r.top(), "signin", "a first run is offered the sign-in page");
        assert_eq!(r.row_titles(), ["Cinemeta/Popular", "Cinemeta/Popular"], "signed out, the defaults browse underneath");
        // the email field: OK raises the keyboard, the panel's commits type into it
        r.press(Key::Ok);
        assert_eq!(r.bridge.keyboard, [true], "OK on the field raised the television's keyboard");
        r.type_text("viewer@example.com");
        r.press(Key::Ok); // on to the password
        r.type_text("hunter2");
        r.press(Key::Ok); // sign in
        r.frames(30);
        let seen = r.api.seen.lock().unwrap().clone();
        let login = seen.iter().find(|(u, _)| u.ends_with("/api/login")).expect("the login request went out");
        assert!(login.1.contains(r#""email":"viewer@example.com""#) && login.1.contains(r#""password":"hunter2""#), "{}", login.1);
        assert!(matches!(r.bridge.account.session(), SessionState::SignedIn(_)));
        assert!(r.vault.saved.lock().unwrap().as_deref().is_some_and(|s| s.contains("k-SECRET")), "the session is kept in the vault");
        assert_eq!(r.top(), "home", "the sign-in page leaves once signed in");
        assert!(r.bridge.keyboard.last() == Some(&false), "and the keyboard is put away");
        // THE accessor switched, and Home's rows with it: the collection's order
        assert!(matches!(r.bridge.addons.in_use(), InUse::Installed(a) if a.len() == 2));
        assert_eq!(r.row_titles(), ["Live TV/Channels", "Cinemeta/Popular"]);
        assert_eq!(r.bridge.catalog.addons()[0].manifest.id, "org.example.live", "detail and streams plan against the same list");
        assert!(r.files.read(&collection_file("u1")).is_some(), "the collection is kept on the device");
        assert_eq!(r.bridge.library.continue_watching().len(), 1, "the library landed");
        r.frames(5);
        let pulled: Vec<String> = r.addons.seen.lock().unwrap().clone();
        assert!(pulled.iter().any(|u| u.contains("/live/cfg-SECRET/catalog/tv/channels")), "the installed addon's own catalog is browsed: {pulled:?}");
        // sign out: the device forgets the session and the collection, Home returns to Cinemeta
        r.bridge.account_cmd(sn_screens::stores::account::AccountCmd::SignOut);
        r.frames(5);
        assert!(r.vault.saved.lock().unwrap().is_none(), "the stored session is removed");
        assert!(r.files.read(&collection_file("u1")).is_none(), "and the account's addon list");
        assert!(r.api.seen.lock().unwrap().iter().any(|(u, b)| u.ends_with("/api/logout") && b.contains("k-SECRET")), "the API session is ended");
        assert!(matches!(r.bridge.addons.in_use(), InUse::Defaults(_)));
        assert!(r.bridge.library.continue_watching().is_empty());
        let urls: Vec<&str> = r.bridge.catalog.sources().iter().map(|s| s.transport_url.as_str()).collect();
        assert_eq!(urls, sn_stremio::account::DEFAULT_ADDONS, "the catalog is the defaults' again");
    }

    /// The item menu's Add to library, end to end: the write is the account's own
    /// (`datastorePut` with its key), the library is asked again after it, and a refused write is
    /// reported as failed.
    #[test]
    fn add_to_library_writes_the_entry_then_reads_the_library_again() {
        use crate::account::Vault;
        use sn_screens::stores::account::{Account, Secret};
        use sn_screens::stores::library::AddState;
        let _g = sn_base::testlock::serial();
        let api = signing_in_api();
        api.answer("datastorePut", r#"{"result":{"success":true}}"#);
        let vault = MemVault::default();
        vault.save(&Account { user_id: "u1".into(), email: "viewer@example.com".into(), auth_key: Secret::new("k-SECRET") }).unwrap();
        let mut r = Rigged::new(api, vault, MemFiles::default());
        r.frames(20);
        assert!(matches!(r.bridge.account.session(), SessionState::SignedIn(_)), "the kept session is restored");
        let entry = sn_stremio::library::NewEntry { id: "tt9".into(), kind: "movie".into(), name: "Nine".into(), poster: None };
        let gets = |r: &Rigged| r.api.seen.lock().unwrap().iter().filter(|(u, _)| u.ends_with("/api/datastoreGet")).count();
        let before = gets(&r);
        let jobs = r.bridge.library.add(entry.clone());
        r.bridge.run_library(jobs);
        r.frames(5);
        let seen = r.api.seen.lock().unwrap().clone();
        let put = seen.iter().find(|(u, _)| u.ends_with("/api/datastorePut")).expect("the write went out");
        assert!(
            put.1.contains(r#""authKey":"k-SECRET""#) && put.1.contains(r#""_id":"tt9""#) && put.1.contains(r#""collection":"libraryItem""#),
            "{}",
            put.1
        );
        assert!(gets(&r) >= before + 2, "the entry is read before the write and the library after it");
        assert_ne!(r.bridge.library.add_state("tt9"), AddState::Adding, "the write landed");
        // a refused write says so
        r.api.fail("datastorePut", sn_stremio::FetchError::Status(500));
        let jobs = r.bridge.library.add(sn_stremio::library::NewEntry { id: "tt8".into(), ..entry });
        r.bridge.run_library(jobs);
        r.frames(5);
        assert_eq!(r.bridge.library.add_state("tt8"), AddState::Absent, "rolled back");
        assert!(r.bridge.library.take_failed().is_empty(), "the refusal was handed to the toast");
        // a refused removal puts the entry back
        assert_eq!(r.bridge.library.add_state("tt0111161"), AddState::In);
        let jobs = r.bridge.library.remove("tt0111161");
        assert_eq!(r.bridge.library.add_state("tt0111161"), AddState::Removing);
        r.bridge.run_library(jobs);
        r.frames(5);
        assert_eq!(r.bridge.library.add_state("tt0111161"), AddState::In, "rolled back");
        let seen = r.api.seen.lock().unwrap().clone();
        assert!(seen.iter().any(|(u, b)| u.ends_with("/api/datastorePut") && b.contains(r#""removed":true"#)), "the removal was the account's write");
    }

    /// **A sign-out leaves nothing of the account behind**: the stored session, the collection's
    /// device copy, the library and Continue Watching, the addons (back to the defaults); nothing is
    /// written to the account but the API's logout; the pages reset to Home and the sign-in page
    /// is offered over it, as on a first run.
    #[test]
    fn signing_out_forgets_the_account_and_offers_the_sign_in_page() {
        use crate::account::{Files, Vault, SEARCH_FILE, SKIPPED_FILE, STREAMS_FILE};
        use sn_screens::stores::account::{Account, Secret};
        let _g = sn_base::testlock::serial();
        let vault = MemVault::default();
        vault.save(&Account { user_id: "u1".into(), email: "viewer@example.com".into(), auth_key: Secret::new("k-SECRET") }).unwrap();
        let mut r = Rigged::new(signing_in_api(), vault, MemFiles::default());
        r.frames(20);
        assert!(matches!(r.bridge.account.session(), SessionState::SignedIn(_)), "the kept session is restored");
        assert_eq!(r.bridge.library.continue_watching().len(), 1, "the library landed");
        assert!(r.files.read(&collection_file("u1")).is_some(), "the collection is kept on the device");
        r.pages.request(MachineId::Nav, NavOp::SelectTab(AppArg::Settings));
        r.frames(10);
        assert_eq!(r.top(), "settings");
        let before = r.api.seen.lock().unwrap().len();
        r.bridge.account_cmd(sn_screens::stores::account::AccountCmd::SignOut);
        r.frames(30);
        // nothing of the account is left
        assert!(r.bridge.account.account().is_none());
        assert!(r.vault.load().unwrap().is_none(), "the stored session is removed");
        let left: Vec<String> = r.files.0.lock().unwrap().keys().cloned().collect();
        assert!(left.iter().all(|f| f == SKIPPED_FILE || f == SEARCH_FILE || f == STREAMS_FILE), "only the device's own choices stay in the data dir: {left:?}");
        assert!(r.bridge.library.continue_watching().is_empty(), "Continue Watching is empty");
        assert!(matches!(r.bridge.addons.in_use(), InUse::Defaults(_)), "the addons are the defaults");
        let urls: Vec<&str> = r.bridge.catalog.sources().iter().map(|s| s.transport_url.as_str()).collect();
        assert_eq!(urls, sn_stremio::account::DEFAULT_ADDONS);
        // the one call to the account's API is the logout (the sign-in page's own device-link code
        // is a new sign-in's, not the old account's)
        let after: Vec<String> = r.api.seen.lock().unwrap()[before..].iter().map(|(u, _)| u.clone()).filter(|u| u.starts_with(sn_stremio::account::API)).collect();
        assert_eq!(after, [format!("{}/api/logout", sn_stremio::account::API)], "a sign-out writes nothing but the logout");
        assert_eq!(r.bridge.playback.view.phase, sn_screens::registry::PlayPhase::Idle, "nothing plays");
        // the pages: the sign-in page over a fresh Home
        assert_eq!(r.top(), "signin", "the sign-in page is offered");
        let stack: Vec<&AppArg> = r.pages.nav.tabs.stack.entries.iter().map(|e| &e.arg).collect();
        assert_eq!(stack, [&AppArg::Home, &AppArg::SignIn], "over a fresh Home");
    }

    /// A wrong password reads the API's message; nothing is kept; the page stays.
    #[test]
    fn a_refused_sign_in_keeps_nothing() {
        let _g = sn_base::testlock::serial();
        let api = signing_in_api();
        api.answer("login", r#"{"error":{"message":"Wrong passphrase","code":3}}"#);
        let mut r = Rigged::new(api, MemVault::default(), MemFiles::default());
        r.frames(20);
        r.press(Key::Ok);
        r.type_text("viewer@example.com");
        r.press(Key::Ok);
        r.type_text("nope");
        r.press(Key::Ok);
        r.frames(10);
        assert_eq!(*r.bridge.account.sign_in(), sn_screens::stores::account::SignIn::Rejected("Wrong passphrase".into()));
        assert_eq!(r.top(), "signin");
        assert!(r.vault.saved.lock().unwrap().is_none());
        assert!(matches!(r.bridge.addons.in_use(), InUse::Defaults(_)));
    }

    /// A boot with a stored session shows the device's copy of the collection at once, then the
    /// account's answer; a failed refresh keeps the copy. No sign-in page is offered.
    #[test]
    fn a_stored_session_boots_on_the_cached_addons_and_refreshes_them() {
        let _g = sn_base::testlock::serial();
        let vault = MemVault::default();
        let acct = sn_screens::stores::account::Account {
            user_id: "u1".into(),
            email: "viewer@example.com".into(),
            auth_key: sn_screens::stores::account::Secret::new("k-SECRET"),
        };
        let _ = crate::account::Vault::save(&vault, &acct);
        let files = MemFiles::default();
        // the cache holds only the Live TV addon
        let cached = r#"{"addons":[{"transportUrl":"https://addons.example/live/cfg-SECRET/manifest.json","manifest":{
            "id":"org.example.live","name":"Live TV","resources":["catalog"],"types":["tv"],
            "catalogs":[{"type":"tv","id":"channels","name":"Channels"}]}}]}"#;
        crate::account::Files::write(&files, &collection_file("u1"), cached.as_bytes());
        let api = signing_in_api();
        api.fail("addonCollectionGet", sn_stremio::FetchError::Transport("timed out".into()));
        let mut r = Rigged::new(api, vault, files);
        r.frames(10);
        assert_eq!(r.top(), "home", "a stored session is not asked to sign in");
        assert!(matches!(r.bridge.account.session(), SessionState::SignedIn(_)));
        assert_eq!(r.row_titles(), ["Live TV/Channels"], "the device's copy, while the account cannot be reached");
        assert!(matches!(r.bridge.addons.refresh(), sn_screens::stores::addons::Refresh::Failed(_)));
        // the account answers on the retry: its list replaces the copy, and is kept
        r.api.answer("addonCollectionGet", COLLECTION);
        r.bridge.app_addons(sn_screens::registry::AddonsCmd::Retry);
        r.frames(5);
        assert_eq!(r.row_titles(), ["Live TV/Channels", "Cinemeta/Popular"]);
        let kept = String::from_utf8(r.files.read(&collection_file("u1")).unwrap()).unwrap();
        assert!(kept.contains("com.linvo.cinemeta"), "the fresh list replaced the device's copy");
    }

    /// The device link: a code shown as a QR, read until it is confirmed, the key confirmed with
    /// `getUser` before it is kept.
    #[test]
    fn the_device_link_signs_in_once_the_code_is_confirmed() {
        let _g = sn_base::testlock::serial();
        let api = signing_in_api();
        let create = format!("{}/api/v2/create?type=Create", sn_stremio::account::LINK_API);
        let read = format!("{}/api/v2/read?type=Read&code=AB12", sn_stremio::account::LINK_API);
        api.answer_url(&create, r#"{"result":{"code":"AB12","link":"https://link.stremio.com/AB12","qrcode":""}}"#);
        api.answer_url(&read, r#"{"error":{"message":"Not yet","code":1}}"#);
        api.answer("getUser", r#"{"result":{"_id":"u7","email":"phone@example.com"}}"#);
        let mut r = Rigged::new(api, MemVault::default(), MemFiles::default());
        r.frames(20);
        assert!(matches!(r.bridge.account.link(), sn_screens::stores::account::Link::Waiting { .. }), "the page asked for a code as it opened");
        r.frames(400); // several polls, all unconfirmed
        let reads = r.api.seen.lock().unwrap().iter().filter(|(u, _)| *u == read).count();
        assert!(reads >= 2, "the code is read again while it is shown ({reads})");
        assert_eq!(r.top(), "signin");
        r.api.answer_url(&read, r#"{"result":{"authKey":"k-LINKED"}}"#);
        r.frames(400);
        assert!(matches!(r.bridge.account.session(), SessionState::SignedIn(a) if a.user_id == "u7"));
        assert!(r.api.seen.lock().unwrap().iter().any(|(u, b)| u.ends_with("/api/getUser") && b.contains("k-LINKED")));
        assert!(r.vault.saved.lock().unwrap().as_deref().is_some_and(|s| s.contains("k-LINKED")));
        assert_eq!(r.top(), "home");
    }
}
