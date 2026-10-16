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
use sn_screens::stores::catalog::{Job, Landing};
use sn_screens::stores::detail::{DetailJob, DetailLanding};
use sn_screens::stores::grid::{GridJob, GridLanding};
use sn_screens::stores::search::{SearchJob, SearchLanding};
use sn_stremio::streams::GroupState;
use sn_stremio::types::{CatalogPage, MetaResponse};
use sn_stremio::{get_json, Fetch, FetchError};

use crate::fetch::{NetFetch, Profile};

/// One outcome for the frame thread.
pub enum Done {
    Catalog(Landing),
    Detail(DetailLanding),
    Search(SearchLanding),
    Grid(GridLanding),
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

    /// Start every catalog job.
    pub fn catalog(&self, runner: &dyn Runner, jobs: Vec<Job>) {
        for job in jobs {
            let (what, fail) = match &job {
                Job::Manifest { source, .. } => ("addon-manifest", Landing::Manifest { source: *source, result: Err(refused()) }),
                Job::Page { row, .. } => ("addon-catalog", Landing::Page { row: *row, result: Err(refused()) }),
            };
            runner.run(what, Box::new(move |off| Done::Catalog(run_catalog(off, job))), self.tx.clone(), Done::Catalog(fail));
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
                    Done::Search(SearchLanding { qgen: job.qgen, row: job.row, result: get_json::<CatalogPage>(&fetch, &job.url, &[]) })
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

    /// Everything posted since the last drain.
    pub fn drain(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }
}

fn refused() -> FetchError {
    FetchError::Transport("no worker".into())
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
    let fetch = NetFetch::new(off, Profile::Api);
    run_detail_with(&fetch, job)
}

/// [`run_detail`] over any transport (the tests' fake).
pub fn run_detail_with(fetch: &dyn Fetch, job: DetailJob) -> DetailLanding {
    match job {
        DetailJob::Meta { key, url } => DetailLanding::Meta { key, result: get_json::<MetaResponse>(fetch, &url, &[]).map(|r| r.meta) },
        DetailJob::Streams { key, index, mut group } => {
            group.load(fetch);
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
        work.catalog(&Refusing, jobs);
        for done in work.drain() {
            let Done::Catalog(landing) = done else { panic!("a catalog job lands as a catalog landing") };
            store.land(landing);
        }
        assert_eq!(store.addons_state(), sn_screens::stores::catalog::AddonsState::Failed);
        assert!(store.first_failure().is_some());
    }
}
