//! **The off-frame half of the stores**: the jobs `sn_screens`' catalog, detail, search and grid
//! stores hand out, run on small-stack workers over [`NetFetch`], and their outcomes handed back to the frame
//! thread through one mailbox.
//!
//! The stores never block and never fetch: they say what they want (`Job`, `DetailJob`) and fold
//! in what came back (`Landing`, `DetailLanding`). This module is the part in between. One worker
//! per request — the stores bound how many are out at once (the catalog scheduler's slots, one
//! stream request per addon, one meta per open page) — and each wakes the present gate when it
//! posts, so a settled screen draws the landing on the next frame.
//!
//! A worker the OS refuses to start is not dropped silently: its job lands at once as a transport
//! failure, so the row or the stream group reads "failed" instead of loading forever.

use std::sync::mpsc::{channel, Receiver, Sender};

use sn_base::task::OffFrame;
use sn_screens::stores::account::{AccountJob, AccountLanding};
use sn_screens::stores::addons::{AddonsJob, AddonsLanding};
use sn_screens::stores::catalog::{Job, Landing};
use sn_screens::stores::detail::{DetailJob, DetailLanding};
use sn_screens::stores::library::{LibraryJob, LibraryLanding};
use sn_stremio::account::ApiError;
use sn_screens::stores::grid::{GridJob, GridLanding};
use sn_screens::stores::search::{SearchJob, SearchLanding};
use sn_stremio::streams::GroupState;
use sn_stremio::types::CatalogPage;
use sn_stremio::{get_json, Fetch, FetchError};

use crate::account::Services;
use crate::fetch::{NetFetch, Profile};

/// One outcome for the frame thread.
pub enum Done {
    /// A catalog landing, and the install list (`CatalogStore::epoch`) its job was planned against:
    /// a landing for a list that has since been replaced is dropped.
    Catalog(u32, Landing),
    Detail(DetailLanding),
    Account(AccountLanding),
    /// `None`: a write, which lands nothing.
    Addons(Option<AddonsLanding>),
    Library(LibraryLanding),
    Search(SearchLanding),
    Grid(GridLanding),
    /// A playback's skip markers (`gen` names the playback), from `source`.
    Skips { gen: u32, source: &'static str, result: Result<sn_stremio::skip::Skips, FetchError> },
    /// One subtitles addon's answer for a playback (`gen` names it), by the addon's name.
    Subtitles { gen: u32, addon: String, result: Result<Vec<sn_stremio::types::Subtitle>, FetchError> },
}

/// The mailbox: workers post, the frame thread drains.
pub struct Work {
    tx: Sender<Done>,
    rx: Receiver<Done>,
}

impl Default for Work {
    fn default() -> Self {
        Self::new()
    }
}

/// How a job is executed: on a real worker in the product, inline in the tests that drive a store
/// end to end against a fake transport.
pub trait Runner {
    fn run(&self, what: &'static str, job: Box<dyn FnOnce(&OffFrame) -> Done + Send>, tx: Sender<Done>, fail: Done);
}

/// Workers, one per job.
pub struct Threads;

impl Runner for Threads {
    fn run(&self, what: &'static str, job: Box<dyn FnOnce(&OffFrame) -> Done + Send>, tx: Sender<Done>, fail: Done) {
        let posted = tx.clone();
        let started = sn_base::task::spawn_off_frame(what, move |off| {
            let _ = posted.send(job(off));
            sn_machine::present::wake_from_worker();
        });
        if !started {
            let _ = tx.send(fail);
        }
    }
}

impl Work {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Work { tx, rx }
    }

    /// Start every catalog job, planned against install list `epoch`.
    pub fn catalog(&self, runner: &dyn Runner, epoch: u32, jobs: Vec<Job>) {
        for job in jobs {
            let (what, fail) = match &job {
                Job::Manifest { source, .. } => ("addon-manifest", Landing::Manifest { source: *source, result: Err(refused()) }),
                Job::Page { row, .. } => ("addon-catalog", Landing::Page { row: *row, result: Err(refused()) }),
            };
            runner.run(what, Box::new(move |off| Done::Catalog(epoch, run_catalog(off, job))), self.tx.clone(), Done::Catalog(epoch, fail));
        }
    }

    /// Start every account job.
    pub fn account(&self, runner: &dyn Runner, services: &Services, jobs: Vec<AccountJob>) {
        for job in jobs {
            let fail = match &job {
                AccountJob::Restore => AccountLanding::Restored(sn_screens::stores::account::Stored::Unavailable),
                AccountJob::Login { .. } => AccountLanding::LoggedIn(Err(api_refused())),
                AccountJob::LinkCreate => AccountLanding::LinkCreated(Err(api_refused())),
                AccountJob::LinkRead { code } => AccountLanding::LinkRead { code: code.clone(), result: Err(api_refused()) },
                AccountJob::Confirm { .. } => AccountLanding::Confirmed(Err(api_refused())),
                AccountJob::Save { .. } => AccountLanding::Saved(false),
                AccountJob::Forget { .. } => AccountLanding::Forgotten,
                AccountJob::Skip => AccountLanding::Skipped,
            };
            let services = services.clone();
            runner.run(
                "account",
                Box::new(move |off| Done::Account(crate::account::run_account(off, &services, job))),
                self.tx.clone(),
                Done::Account(fail),
            );
        }
    }

    /// Start every addons job.
    pub fn addons(&self, runner: &dyn Runner, services: &Services, jobs: Vec<AddonsJob>) {
        for job in jobs {
            let fail = match &job {
                AddonsJob::ReadCache { user_id } => Some(AddonsLanding::CacheRead { user_id: user_id.clone(), addons: None }),
                AddonsJob::Fetch { user_id, .. } => Some(AddonsLanding::Fetched { user_id: user_id.clone(), result: Err(api_refused()) }),
                AddonsJob::ReadFilter => Some(AddonsLanding::FilterRead(Default::default())),
                AddonsJob::ReadStreamFilter => Some(AddonsLanding::StreamFilterRead(Default::default())),
                AddonsJob::WriteCache { .. } | AddonsJob::DropCache { .. } | AddonsJob::WriteFilter(_) | AddonsJob::WriteStreamFilter(_) => None,
            };
            let services = services.clone();
            runner.run(
                "addons",
                Box::new(move |off| Done::Addons(crate::account::run_addons(off, &services, job))),
                self.tx.clone(),
                Done::Addons(fail),
            );
        }
    }

    /// Start every library job.
    pub fn library(&self, runner: &dyn Runner, services: &Services, jobs: Vec<LibraryJob>) {
        for job in jobs {
            let fail = match &job {
                LibraryJob::Fetch { user_id, .. } => LibraryLanding::Fetched { user_id: user_id.clone(), result: Err(api_refused()) },
                LibraryJob::Add { user_id, entry, .. } => {
                    LibraryLanding::Added { user_id: user_id.clone(), id: entry.id.clone(), result: Err(api_refused()) }
                }
                LibraryJob::Edit { user_id, id, .. } => {
                    LibraryLanding::Edited { user_id: user_id.clone(), id: id.clone(), result: Err(api_refused()) }
                }
                LibraryJob::Mark { user_id, entry, .. } => {
                    LibraryLanding::Edited { user_id: user_id.clone(), id: entry.id.clone(), result: Err(api_refused()) }
                }
                LibraryJob::Progress { user_id, last, .. } => {
                    LibraryLanding::Progressed { user_id: user_id.clone(), last: *last, result: Err(api_refused()) }
                }
            };
            let services = services.clone();
            runner.run(
                "library",
                Box::new(move |off| Done::Library(crate::account::run_library(off, &services, job))),
                self.tx.clone(),
                Done::Library(fail),
            );
        }
    }

    /// Start every detail job.
    pub fn detail(&self, runner: &dyn Runner, jobs: Vec<DetailJob>) {
        for job in jobs {
            let (what, fail) = match &job {
                DetailJob::Meta { key, .. } => ("addon-meta", DetailLanding::Meta { key: key.clone(), result: Err(refused()) }),
                DetailJob::Streams { key, index, .. } => {
                    ("addon-streams", DetailLanding::Streams { key: key.clone(), index: *index, state: GroupState::Failed(refused()) })
                }
            };
            runner.run(what, Box::new(move |off| Done::Detail(run_detail(off, job))), self.tx.clone(), Done::Detail(fail));
        }
    }

    /// Start every search row's first page.
    pub fn search(&self, runner: &dyn Runner, jobs: Vec<SearchJob>) {
        for job in jobs {
            let fail = SearchLanding { qgen: job.qgen, row: job.row, result: Err(refused()) };
            runner.run(
                "addon-search",
                Box::new(move |off| {
                    let fetch = NetFetch::new(off, Profile::Api).cancelled_by(job.cancel);
                    let began = std::time::Instant::now();
                    let result = get_json::<CatalogPage>(&fetch, &job.url, &[]);
                    let outcome = match &result {
                        Ok(p) => format!("{} items", p.metas.len()),
                        Err(e) => format!("failed ({})", sn_base::eventlog::scrub::scrub_local(&e.to_string())),
                    };
                    sn_base::eventlog::log(&format!("addon: search {} in {} ms: {outcome}", job.label, began.elapsed().as_millis()));
                    Done::Search(SearchLanding { qgen: job.qgen, row: job.row, result })
                }),
                self.tx.clone(),
                Done::Search(fail),
            );
        }
    }

    /// Start every grid page.
    pub fn grid(&self, runner: &dyn Runner, jobs: Vec<GridJob>) {
        for job in jobs {
            let fail = GridLanding { key: job.key.clone(), result: Err(refused()) };
            runner.run(
                "addon-grid",
                Box::new(move |off| {
                    let fetch = NetFetch::new(off, Profile::Api).cancelled_by(job.cancel);
                    Done::Grid(GridLanding { key: job.key, result: get_json::<CatalogPage>(&fetch, &job.url, &[]) })
                }),
                self.tx.clone(),
                Done::Grid(fail),
            );
        }
    }

    /// Look up one playback's skip markers.
    pub fn skips(&self, runner: &dyn Runner, job: crate::app::playback::SkipLookup) {
        let (gen, source) = (job.gen, job.lookup.source());
        runner.run(
            "skip-markers",
            Box::new(move |off| {
                let fetch = NetFetch::new(off, Profile::Api);
                Done::Skips { gen, source, result: sn_stremio::skip::fetch_skips(&fetch, &job.lookup, job.dur_ms) }
            }),
            self.tx.clone(),
            Done::Skips { gen, source, result: Err(refused()) },
        );
    }

    /// Ask every subtitles addon in `sources` for playback `gen`'s subtitles, each on its own
    /// worker (a slow addon holds only its own answer).
    pub fn subtitles(&self, runner: &dyn Runner, gen: u32, sources: Vec<sn_stremio::subtitles::SubtitleSource>) {
        for source in sources {
            let addon = source.addon_name.clone();
            runner.run(
                "addon-subtitles",
                Box::new(move |off| {
                    let fetch = NetFetch::new(off, Profile::Api);
                    run_subtitles(&fetch, gen, &source)
                }),
                self.tx.clone(),
                Done::Subtitles { gen, addon, result: Err(refused()) },
            );
        }
    }

    /// Everything posted since the last drain.
    pub fn drain(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }
}

fn refused() -> FetchError {
    FetchError::Transport("no worker".into())
}

fn api_refused() -> ApiError {
    ApiError::Fetch(refused())
}

/// One catalog job, on a worker.
pub fn run_catalog(off: &OffFrame, job: Job) -> Landing {
    match job {
        Job::Manifest { source, url } => Landing::Manifest { source, result: NetFetch::new(off, Profile::Api).get(&url, &[]) },
        Job::Page { row, url, cancel } => {
            let fetch = NetFetch::new(off, Profile::Api).cancelled_by(cancel);
            Landing::Page { row, result: get_json::<CatalogPage>(&fetch, &url, &[]) }
        }
    }
}

/// One detail job, on a worker.
pub fn run_detail(off: &OffFrame, job: DetailJob) -> DetailLanding {
    match job {
        DetailJob::Meta { key, urls } if urls.len() > 1 => {
            let began = std::time::Instant::now();
            let result = race_meta(&urls);
            sn_base::eventlog::log(&format!(
                "addon: meta {} from {} source(s) asked at once in {} ms",
                if result.is_ok() { "ok" } else { "failed" },
                urls.len(),
                began.elapsed().as_millis()
            ));
            DetailLanding::Meta { key, result }
        }
        job => run_detail_with(&NetFetch::new(off, Profile::Api), job),
    }
}

/// Ask every meta source at once, each on its own worker, and take the first success in addon
/// order ([`sn_stremio::meta::pick_in_order`]): a slow or failing source ahead of a good one no
/// longer holds the page (one took 13.1 s on the TV before the next source was even asked).
/// Sources still running when the answer is decided finish on their own; their results drop.
fn race_meta(urls: &[String]) -> Result<sn_stremio::types::Meta, sn_stremio::FetchError> {
    use sn_stremio::FetchError;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut slots: Vec<Option<Result<sn_stremio::types::Meta, FetchError>>> = vec![None; urls.len()];
    for (i, url) in urls.iter().enumerate() {
        let (tx, url) = (tx.clone(), url.clone());
        let started = sn_base::task::spawn_off_frame("addon-meta", move |off| {
            let fetch = NetFetch::new(off, Profile::Api);
            let r = sn_stremio::Fetch::get(&fetch, &url, &[]).and_then(|b| sn_stremio::meta::decode_meta(&b));
            let _ = tx.send((i, r));
        });
        if !started {
            slots[i] = Some(Err(FetchError::Transport("no worker thread".into())));
        }
    }
    drop(tx);
    // once any source succeeded, the ones ahead of it get META_GRACE_MS more
    let mut grace_until: Option<std::time::Instant> = None;
    loop {
        if let Some(r) = sn_stremio::meta::pick_in_order(&slots) {
            return r;
        }
        if grace_until.is_some_and(|t| std::time::Instant::now() >= t) {
            if let Some(r) = sn_stremio::meta::pick_after_grace(&slots) {
                return r;
            }
        }
        let got = match grace_until {
            Some(t) => match rx.recv_timeout(t.saturating_duration_since(std::time::Instant::now())) {
                Ok(v) => Ok(v),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(()),
            },
            None => rx.recv().map_err(|_| ()),
        };
        match got {
            Ok((i, r)) => {
                if grace_until.is_none() && r.is_ok() {
                    grace_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(sn_stremio::meta::META_GRACE_MS));
                }
                slots[i] = Some(r);
            }
            // every worker gone without deciding: fill what never answered as failed
            Err(_) => {
                for s in slots.iter_mut().filter(|s| s.is_none()) {
                    *s = Some(Err(FetchError::Transport("no answer".into())));
                }
            }
        }
    }
}

/// One subtitles addon's answer, over any transport. Logged by the addon's name and its timing,
/// never the URL (the addon's base, and its subtitles' URLs, can carry its configuration).
pub fn run_subtitles(fetch: &dyn Fetch, gen: u32, source: &sn_stremio::subtitles::SubtitleSource) -> Done {
    let began = std::time::Instant::now();
    let result = sn_stremio::subtitles::fetch(fetch, source);
    let outcome = match &result {
        Ok(s) => format!("{} subtitles", s.len()),
        Err(e) => format!("failed ({})", sn_base::eventlog::scrub::scrub_local(&e.to_string())),
    };
    sn_base::eventlog::log(&format!("addon: subtitles from {} in {} ms: {outcome}", source.addon_name, began.elapsed().as_millis()));
    Done::Subtitles { gen, addon: source.addon_name.clone(), result }
}

/// [`run_detail`] over any transport (the tests' fake).
pub fn run_detail_with(fetch: &dyn Fetch, job: DetailJob) -> DetailLanding {
    match job {
        DetailJob::Meta { key, urls } => {
            let began = std::time::Instant::now();
            let result = sn_stremio::meta::fetch_meta(fetch, &urls);
            // No URL: an addon's path carries its configuration.
            sn_base::eventlog::log(&format!(
                "addon: meta {} from {} source(s) in {} ms",
                if result.is_ok() { "ok" } else { "failed" },
                urls.len(),
                began.elapsed().as_millis()
            ));
            DetailLanding::Meta { key, result }
        }
        DetailJob::Streams { key, index, mut group } => {
            let began = std::time::Instant::now();
            group.load(fetch);
            let outcome = match &group.state {
                GroupState::Ready(s) => format!("{} streams", s.len()),
                GroupState::Failed(e) => format!("failed ({})", sn_base::eventlog::scrub::scrub_local(&e.to_string())),
                GroupState::Loading => "loading".into(),
                GroupState::Off => "off".into(),
            };
            sn_base::eventlog::log(&format!(
                "addon: streams from {} for {} {} in {} ms: {outcome}",
                group.addon_name,
                key.kind,
                id_shape(&key.video_id),
                began.elapsed().as_millis()
            ));
            DetailLanding::Streams { key, index, state: group.state }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_screens::stores::catalog::CatalogStore;

    /// Runs nothing: every job is refused at spawn.
    struct Refusing;
    impl Runner for Refusing {
        fn run(&self, _what: &'static str, _job: Box<dyn FnOnce(&OffFrame) -> Done + Send>, tx: Sender<Done>, fail: Done) {
            let _ = tx.send(fail);
        }
    }

    #[test]
    fn a_refused_worker_lands_as_a_failure_instead_of_loading_forever() {
        let work = Work::new();
        let mut store = CatalogStore::signed_out();
        let jobs = store.take_jobs();
        assert!(!jobs.is_empty(), "a signed-out store asks for the default manifests");
        work.catalog(&Refusing, store.epoch(), jobs);
        for done in work.drain() {
            let Done::Catalog(_, landing) = done else { panic!("a catalog job lands as a catalog landing") };
            store.land(landing);
        }
        assert_eq!(store.addons_state(), sn_screens::stores::catalog::AddonsState::Failed);
        assert!(store.first_failure().is_some());
    }
}

/// A video id's scheme and episode, without the title's own number: `tmdb:…:2:4`, `tt…:1:3`,
/// `tt…` (which id scheme an addon was asked with is what an empty answer is read against).
fn id_shape(id: &str) -> String {
    let mut parts = id.split(':');
    let head = parts.next().unwrap_or("");
    let scheme = if head.starts_with("tt") && head[2..].bytes().all(|b| b.is_ascii_digit()) {
        "tt…".to_string()
    } else {
        parts.next();
        format!("{}:…", head.chars().filter(char::is_ascii_alphanumeric).take(12).collect::<String>())
    };
    std::iter::once(scheme).chain(parts.map(|p| p.chars().filter(char::is_ascii_digit).take(4).collect())).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod id_shape_tests {
    #[test]
    fn the_shape_keeps_the_scheme_and_episode_not_the_title() {
        assert_eq!(super::id_shape("tt0903747:1:3"), "tt…:1:3");
        assert_eq!(super::id_shape("tt0816692"), "tt…");
        assert_eq!(super::id_shape("tmdb:1396:2:4"), "tmdb:…:2:4");
        assert_eq!(super::id_shape("kitsu:7442:3"), "kitsu:…:3");
    }
}
