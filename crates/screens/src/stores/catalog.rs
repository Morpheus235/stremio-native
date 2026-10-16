//! **The catalog store** — the installed addons' manifests and one row per browsable catalog, each
//! row filled page by page by its own [`Pager`], with every page request queued through the
//! viewport-first [`Scheduler`].
//!
//! The store is a pure state machine: it never touches a socket. It hands out [`Job`]s (a URL, and
//! for a page a [`Cancel`] token), the application runs them off the frame thread over
//! `sn_stremio::Fetch`, and the outcome comes back as a [`Landing`] on the frame thread. That split
//! is what lets the row model and the scheduler wiring be graded by the host suite with no network.
//!
//! - **Rows come from the manifests**, in install order and, within an addon, in the order its
//!   manifest lists its catalogs. A catalog that cannot be shown as a plain row (one that requires
//!   `search`, or a genre it offers no option for — [`Catalog::is_browsable`]) is not a row.
//! - **Viewport-first.** The screen reports the rows it shows ([`CatalogStore::set_view`]); only
//!   rows inside that window plus [`LOOKAHEAD_ROWS`] each side are asked for, nearest first, at
//!   most [`PAGE_SLOTS`] at once. A row scrolled past is cancelled (its token is set, so a request
//!   that has not reached the transport never leaves) and its late landing is dropped, so it is
//!   simply asked again when it comes back into view — never drained top-down.
//! - **A failed page keeps what loaded.** The pager stays on the skip it failed at; the row is
//!   asked again the next time it enters the window.

use std::ops::Range;

use sn_stremio::catalog::{More, Pager};
use sn_stremio::manifest::{Addon, Catalog, Manifest};
use sn_stremio::sched::{Cancel, Scheduler};
use sn_stremio::types::{CatalogPage, MetaPreview};
use sn_stremio::FetchError;

/// Catalog page requests in flight at once.
pub const PAGE_SLOTS: usize = 3;
/// Rows beyond the visible ones, each side, still worth asking for.
pub const LOOKAHEAD_ROWS: usize = 2;

/// A row's identity: its addon's position in the install list and its catalog's position in that
/// addon's manifest. Stable for as long as the install list is, which is what the scheduler keys
/// on and what a screen interns its card elements under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowId {
    pub source: u16,
    pub catalog: u16,
}

/// One installed addon, as far as the store has got with its manifest.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceState {
    /// Not asked yet.
    Pending,
    /// Its manifest request is out.
    Loading,
    Ready(Addon),
    /// Its manifest could not be fetched or read. The addon contributes no rows, and the failure
    /// is kept (never folded into "no rows").
    Failed(FetchError),
}

#[derive(Clone, Debug)]
pub struct Source {
    /// What the user installed: `https://host/[config/]manifest.json`. Never logged (a configured
    /// addon's URL carries its configuration).
    pub transport_url: String,
    pub state: SourceState,
}

/// One browsable catalog of one addon, and what of it has loaded.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: RowId,
    /// The addon's display name (its manifest `name`, else its id).
    pub addon_name: String,
    pub pager: Pager,
}

/// What a row can show right now.
#[derive(Clone, Debug, PartialEq)]
pub enum RowStatus {
    /// Nothing has landed yet and more is to come (its first page is queued, in flight or not yet
    /// asked because the row is out of view).
    Loading,
    /// Cards are shown; `more` says whether the catalog goes on.
    Ready { more: bool },
    /// The catalog ended with nothing in it: the row is not shown.
    Empty,
    /// The last page failed. `shown` cards (if any) stay.
    Failed(FetchError),
}

impl Row {
    pub fn kind(&self) -> &str {
        &self.pager.catalog.kind
    }

    pub fn title(&self) -> &str {
        self.pager.catalog.display_name()
    }

    pub fn items(&self) -> &[MetaPreview] {
        &self.pager.items
    }

    pub fn status(&self) -> RowStatus {
        match (&self.pager.more, self.pager.items.is_empty()) {
            (More::Failed { error, .. }, _) => RowStatus::Failed(error.clone()),
            (More::End, true) => RowStatus::Empty,
            (More::End, false) => RowStatus::Ready { more: false },
            (More::Next(_), true) => RowStatus::Loading,
            (More::Next(_), false) => RowStatus::Ready { more: true },
        }
    }

    /// Asked for while it is in the window: no card yet and the catalog may still answer one
    /// (a pending first page, or a failure the reader is looking at again).
    fn wants_first_page(&self) -> bool {
        self.pager.items.is_empty() && !matches!(self.pager.more, More::End)
    }
}

/// Work the application runs off the frame thread.
#[derive(Clone, Debug)]
pub enum Job {
    /// Fetch source `source`'s manifest at `url`.
    Manifest { source: usize, url: String },
    /// Fetch `row`'s next page at `url`. `cancel` is set when the row leaves the window; a fetch
    /// that sees it set before it starts answers `FetchError::Cancelled`.
    Page { row: RowId, url: String, cancel: Cancel },
}

/// A job's outcome, handed back on the frame thread.
#[derive(Clone, Debug)]
pub enum Landing {
    Manifest { source: usize, result: Result<Vec<u8>, FetchError> },
    Page { row: RowId, result: Result<CatalogPage, FetchError> },
}

/// Where the install list as a whole stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddonsState {
    /// No manifest has landed yet and at least one is still out.
    Loading,
    /// At least one addon is ready (others may have failed or still be loading).
    Ready,
    /// Every manifest failed.
    Failed,
}

pub struct CatalogStore {
    sources: Vec<Source>,
    /// The ready addons in install order — what stream and meta lookups are planned against.
    addons: Vec<Addon>,
    rows: Vec<Row>,
    sched: Scheduler<RowId>,
    /// The rows the screen shows, as indices into [`Self::rows`].
    view: Range<usize>,
    gen: u32,
}

impl CatalogStore {
    /// A store over these transport URLs, in install order. Nothing is asked until [`take_jobs`].
    ///
    /// [`take_jobs`]: Self::take_jobs
    pub fn new<S: Into<String>>(transport_urls: impl IntoIterator<Item = S>) -> Self {
        CatalogStore {
            sources: transport_urls
                .into_iter()
                .map(|u| Source { transport_url: u.into(), state: SourceState::Pending })
                .collect(),
            addons: Vec::new(),
            rows: Vec::new(),
            sched: Scheduler::new(PAGE_SLOTS, LOOKAHEAD_ROWS),
            view: 0..0,
            gen: 0,
        }
    }

    /// Signed out: the addons every Stremio install starts with (Cinemeta).
    pub fn signed_out() -> Self {
        Self::new(sn_stremio::account::DEFAULT_ADDONS.iter().copied())
    }

    /// Moves whenever anything a screen reads here changes.
    pub fn gen(&self) -> u32 {
        self.gen
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// The ready addons, in install order.
    pub fn addons(&self) -> &[Addon] {
        &self.addons
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row(&self, id: RowId) -> Option<&Row> {
        self.index_of(id).map(|i| &self.rows[i])
    }

    pub fn index_of(&self, id: RowId) -> Option<usize> {
        self.rows.binary_search_by(|r| r.id.cmp(&id)).ok()
    }

    pub fn addons_state(&self) -> AddonsState {
        if !self.addons.is_empty() {
            return AddonsState::Ready;
        }
        if self.sources.iter().any(|s| matches!(s.state, SourceState::Pending | SourceState::Loading)) {
            AddonsState::Loading
        } else {
            AddonsState::Failed
        }
    }

    /// The first manifest failure, for a read-out when every addon failed.
    pub fn first_failure(&self) -> Option<&FetchError> {
        self.sources.iter().find_map(|s| match &s.state {
            SourceState::Failed(e) => Some(e),
            _ => None,
        })
    }

    /// The rows the screen shows moved: re-aim the scheduler at them (cancelling what left the
    /// window) and ask for the first page of every row in the window that has none.
    pub fn set_view(&mut self, view: Range<usize>) {
        if view == self.view {
            return;
        }
        self.view = view.clone();
        self.sched.set_view(view);
        self.want_window();
    }

    /// The screen wants more of `row` (its shelf reached the end of what is loaded).
    pub fn want_more(&mut self, row: RowId) {
        let Some(i) = self.index_of(row) else { return };
        if self.rows[i].pager.has_more() {
            self.sched.want(row, i);
        }
    }

    /// Ask again for a row whose last page failed (a press on its read-out).
    pub fn retry(&mut self, row: RowId) {
        let Some(i) = self.index_of(row) else { return };
        if matches!(self.rows[i].pager.more, More::Failed { .. }) {
            self.sched.want(row, i);
        }
    }

    /// Every manifest that failed is asked again.
    pub fn retry_addons(&mut self) {
        for s in &mut self.sources {
            if matches!(s.state, SourceState::Failed(_)) {
                s.state = SourceState::Pending;
            }
        }
        self.gen = self.gen.wrapping_add(1);
    }

    fn want_window(&mut self) {
        let lo = self.view.start.saturating_sub(LOOKAHEAD_ROWS);
        let hi = self.view.end.saturating_add(LOOKAHEAD_ROWS).min(self.rows.len());
        for i in lo..hi {
            if self.rows[i].wants_first_page() {
                self.sched.want(self.rows[i].id, i);
            }
        }
    }

    /// The work due now: every manifest not yet asked, then the scheduler's page requests,
    /// nearest the viewport first, as many as there are free slots.
    pub fn take_jobs(&mut self) -> Vec<Job> {
        let mut jobs = Vec::new();
        for (source, s) in self.sources.iter_mut().enumerate() {
            if s.state == SourceState::Pending {
                s.state = SourceState::Loading;
                jobs.push(Job::Manifest { source, url: s.transport_url.clone() });
            }
        }
        while let Some((row, cancel)) = self.sched.next() {
            match self.row(row).and_then(|r| r.pager.next_url()) {
                Some(url) => jobs.push(Job::Page { row, url, cancel }),
                None => {
                    self.sched.done(&row);
                }
            }
        }
        jobs
    }

    /// Fold a job's outcome in. `false` when it changed nothing a screen reads (a page whose row
    /// was cancelled while it was out, a manifest for a source that is not loading).
    pub fn land(&mut self, landing: Landing) -> bool {
        match landing {
            Landing::Manifest { source, result } => self.land_manifest(source, result),
            Landing::Page { row, result } => {
                if !self.sched.done(&row) {
                    return false; // cancelled while it was out: the row is asked again in view
                }
                let Some(i) = self.index_of(row) else { return false };
                self.rows[i].pager.land(result);
                self.gen = self.gen.wrapping_add(1);
                true
            }
        }
    }

    fn land_manifest(&mut self, source: usize, result: Result<Vec<u8>, FetchError>) -> bool {
        let Some(s) = self.sources.get_mut(source) else { return false };
        if s.state != SourceState::Loading {
            return false;
        }
        s.state = match result.and_then(|body| Manifest::parse(&body).map_err(FetchError::Decode)) {
            Ok(manifest) => SourceState::Ready(Addon::new(&s.transport_url, manifest)),
            Err(e) => SourceState::Failed(e),
        };
        self.rebuild();
        self.gen = self.gen.wrapping_add(1);
        true
    }

    /// The ready addons and their rows, from the sources as they stand. Rows already loading keep
    /// their pagers; a row's position may move (an earlier addon's rows land later), so the
    /// scheduler is re-aimed at the same window.
    fn rebuild(&mut self) {
        self.addons = self
            .sources
            .iter()
            .filter_map(|s| match &s.state {
                SourceState::Ready(a) => Some(a.clone()),
                _ => None,
            })
            .collect();
        let mut old: Vec<Row> = std::mem::take(&mut self.rows);
        for (si, s) in self.sources.iter().enumerate() {
            let SourceState::Ready(addon) = &s.state else { continue };
            for (ci, catalog) in browsable(&addon.manifest.catalogs) {
                let id = RowId { source: si as u16, catalog: ci as u16 };
                let row = match old.iter().position(|r| r.id == id) {
                    Some(at) => old.swap_remove(at),
                    None => Row {
                        id,
                        addon_name: if addon.manifest.name.is_empty() {
                            addon.manifest.id.clone()
                        } else {
                            addon.manifest.name.clone()
                        },
                        pager: Pager::new(&addon.base, catalog.clone(), None, None),
                    },
                };
                self.rows.push(row);
            }
        }
        self.rows.sort_by_key(|r| r.id);
        self.sched.set_view(self.view.clone());
        self.want_window();
    }
}

/// The catalogs of a manifest that are rows, with their position in the manifest.
fn browsable(catalogs: &[Catalog]) -> impl Iterator<Item = (usize, &Catalog)> {
    catalogs.iter().enumerate().filter(|(_, c)| c.is_browsable())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const CINEMETA_URL: &str = "https://v3-cinemeta.strem.io/manifest.json";
    pub(crate) const CINEMETA: &str = r#"{
        "id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta",
        "resources":["catalog","meta"],
        "types":["movie","series"],"idPrefixes":["tt"],
        "catalogs":[
            {"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"genre","options":["Action"]},{"name":"skip"}]},
            {"type":"series","id":"top","name":"Popular","extraSupported":["search","genre","skip"]},
            {"type":"movie","id":"search-only","extra":[{"name":"search","isRequired":true}]},
            {"type":"movie","id":"year","name":"By year","extra":[{"name":"genre","isRequired":true,"options":["2026"]},{"name":"skip"}]}
        ]}"#;
    const LIVE_URL: &str = "https://live.example/manifest.json";
    const LIVE: &str = r#"{"id":"org.example.live","name":"Live TV","resources":["catalog","stream"],"types":["tv"],
        "catalogs":[{"type":"tv","id":"channels","name":"Channels"}]}"#;

    fn page(ids: std::ops::Range<usize>) -> CatalogPage {
        CatalogPage {
            metas: ids.map(|i| MetaPreview { id: format!("tt{i}"), kind: "movie".into(), name: format!("M{i}"), ..Default::default() }).collect(),
        }
    }

    fn manifest_jobs(s: &mut CatalogStore) -> Vec<(usize, String)> {
        s.take_jobs()
            .into_iter()
            .filter_map(|j| match j {
                Job::Manifest { source, url } => Some((source, url)),
                Job::Page { .. } => None,
            })
            .collect()
    }

    fn page_jobs(jobs: &[Job]) -> Vec<(RowId, String)> {
        jobs.iter()
            .filter_map(|j| match j {
                Job::Page { row, url, .. } => Some((*row, url.clone())),
                Job::Manifest { .. } => None,
            })
            .collect()
    }

    /// A store with `n` one-catalog addons, all landed, nothing asked.
    fn many_rows(n: usize) -> CatalogStore {
        let urls: Vec<String> = (0..n).map(|i| format!("https://a{i}.example/manifest.json")).collect();
        let mut s = CatalogStore::new(urls);
        for (source, _) in manifest_jobs(&mut s) {
            let body = format!(r#"{{"id":"a{source}","name":"A{source}","resources":["catalog"],"types":["movie"],
                "catalogs":[{{"type":"movie","id":"top","extra":[{{"name":"skip"}}]}}]}}"#);
            assert!(s.land(Landing::Manifest { source, result: Ok(body.into_bytes()) }));
        }
        s
    }

    fn rid(source: u16) -> RowId {
        RowId { source, catalog: 0 }
    }

    #[test]
    fn signed_out_asks_for_cinemeta_and_nothing_else() {
        let mut s = CatalogStore::signed_out();
        assert_eq!(manifest_jobs(&mut s), vec![(0, CINEMETA_URL.to_string())]);
        assert!(manifest_jobs(&mut s).is_empty(), "a manifest is asked once");
        assert_eq!(s.addons_state(), AddonsState::Loading);
    }

    #[test]
    fn rows_are_every_browsable_catalog_in_install_then_manifest_order() {
        let mut s = CatalogStore::new([LIVE_URL, CINEMETA_URL]);
        let jobs = manifest_jobs(&mut s);
        assert_eq!(jobs.len(), 2);
        // Cinemeta (installed second) answers first: its rows must still come after Live TV's.
        assert!(s.land(Landing::Manifest { source: 1, result: Ok(CINEMETA.as_bytes().to_vec()) }));
        assert_eq!(s.addons_state(), AddonsState::Ready);
        assert!(s.land(Landing::Manifest { source: 0, result: Ok(LIVE.as_bytes().to_vec()) }));
        let rows: Vec<(String, String, String)> = s
            .rows()
            .iter()
            .map(|r| (r.addon_name.clone(), r.kind().to_string(), r.title().to_string()))
            .collect();
        assert_eq!(
            rows,
            [
                ("Live TV", "tv", "Channels"),
                ("Cinemeta", "movie", "Popular"),
                ("Cinemeta", "series", "Popular"),
                ("Cinemeta", "movie", "By year"),
            ]
            .map(|(a, k, t)| (a.to_string(), k.to_string(), t.to_string()))
            .to_vec(),
            "a search-only catalog is no row"
        );
        assert_eq!(s.addons().len(), 2);
        assert_eq!(s.addons()[0].manifest.id, "org.example.live", "addons stay in install order");
    }

    #[test]
    fn a_failed_manifest_is_kept_and_the_rest_still_browse() {
        let mut s = CatalogStore::new([LIVE_URL, CINEMETA_URL]);
        manifest_jobs(&mut s);
        s.land(Landing::Manifest { source: 0, result: Err(FetchError::Status(502)) });
        assert_eq!(s.addons_state(), AddonsState::Loading, "one is still out");
        s.land(Landing::Manifest { source: 1, result: Ok(b"not json".to_vec()) });
        assert_eq!(s.addons_state(), AddonsState::Failed);
        assert_eq!(s.first_failure(), Some(&FetchError::Status(502)));
        assert!(matches!(s.sources()[1].state, SourceState::Failed(FetchError::Decode(_))));
        s.retry_addons();
        assert_eq!(manifest_jobs(&mut s).len(), 2, "a retry asks every failed manifest again");
    }

    #[test]
    fn the_first_rows_are_asked_before_any_view_is_reported() {
        let mut s = many_rows(6);
        let jobs = s.take_jobs();
        assert_eq!(
            page_jobs(&jobs).iter().map(|(r, _)| *r).collect::<Vec<_>>(),
            vec![rid(0), rid(1)],
            "with no view yet the lookahead below the top is the window"
        );
        assert_eq!(page_jobs(&jobs)[0].1, "https://a0.example/catalog/movie/top.json");
    }

    #[test]
    fn on_screen_rows_load_first_bounded_by_the_slots() {
        let mut s = many_rows(12);
        s.set_view(5..8);
        let order: Vec<RowId> = page_jobs(&s.take_jobs()).iter().map(|(r, _)| *r).collect();
        assert_eq!(order.len(), PAGE_SLOTS, "never more than the slots in flight");
        assert_eq!(order, vec![rid(5), rid(6), rid(7)], "the visible rows go first, top-down among equals");
        // nothing more until something lands
        assert!(page_jobs(&s.take_jobs()).is_empty());
        assert!(s.land(Landing::Page { row: rid(6), result: Ok(page(0..20)) }));
        let next: Vec<RowId> = page_jobs(&s.take_jobs()).iter().map(|(r, _)| *r).collect();
        assert_eq!(next.len(), 1);
        assert!(next[0] == rid(4) || next[0] == rid(8), "then the nearest lookahead row ({next:?})");
    }

    #[test]
    fn scrolling_far_cancels_rows_left_behind_and_drops_their_late_pages() {
        let mut s = many_rows(20);
        s.set_view(0..3);
        let jobs = s.take_jobs();
        let cancels: Vec<(RowId, Cancel)> = jobs
            .iter()
            .filter_map(|j| match j {
                Job::Page { row, cancel, .. } => Some((*row, cancel.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(cancels.len(), PAGE_SLOTS);
        s.set_view(15..18);
        assert!(cancels.iter().all(|(_, c)| c.is_cancelled()), "every in-flight row left the window");
        let gen = s.gen();
        assert!(!s.land(Landing::Page { row: rid(0), result: Ok(page(0..20)) }), "a cancelled page is dropped");
        assert_eq!(s.gen(), gen);
        assert!(s.row(rid(0)).unwrap().items().is_empty());
        let order: Vec<RowId> = page_jobs(&s.take_jobs()).iter().map(|(r, _)| *r).collect();
        assert_eq!(order, vec![rid(15), rid(16), rid(17)], "the new view loads first");
        // …and the row scrolled back into view is simply asked again
        s.set_view(0..1);
        for r in [15, 16, 17] {
            s.land(Landing::Page { row: rid(r), result: Ok(page(0..5)) });
        }
        let order: Vec<RowId> = page_jobs(&s.take_jobs()).iter().map(|(r, _)| *r).collect();
        assert_eq!(order.first(), Some(&rid(0)));
    }

    #[test]
    fn more_pages_follow_the_addons_own_page_size() {
        let mut s = many_rows(1);
        s.take_jobs();
        s.land(Landing::Page { row: rid(0), result: Ok(page(0..20)) });
        assert_eq!(s.row(rid(0)).unwrap().status(), RowStatus::Ready { more: true });
        s.want_more(rid(0));
        let jobs = page_jobs(&s.take_jobs());
        assert_eq!(jobs, vec![(rid(0), "https://a0.example/catalog/movie/top/skip=20.json".to_string())]);
        s.land(Landing::Page { row: rid(0), result: Ok(page(0..20)) });
        assert_eq!(s.row(rid(0)).unwrap().status(), RowStatus::Ready { more: false }, "a repeated page ends it");
        s.want_more(rid(0));
        assert!(page_jobs(&s.take_jobs()).is_empty(), "an ended row asks nothing");
    }

    #[test]
    fn a_failed_page_is_shown_as_failed_and_asked_again_in_view() {
        let mut s = many_rows(8);
        s.take_jobs();
        s.land(Landing::Page { row: rid(0), result: Err(FetchError::Transport("timed out".into())) });
        assert_eq!(s.row(rid(0)).unwrap().status(), RowStatus::Failed(FetchError::Transport("timed out".into())));
        s.set_view(5..8);
        s.take_jobs();
        s.set_view(0..2);
        let order: Vec<RowId> = page_jobs(&s.take_jobs()).iter().map(|(r, _)| *r).collect();
        assert!(order.contains(&rid(0)), "the failed row is asked again when it is back in view: {order:?}");
    }

    #[test]
    fn an_empty_catalog_is_no_row_to_show() {
        let mut s = many_rows(1);
        s.take_jobs();
        s.land(Landing::Page { row: rid(0), result: Ok(CatalogPage::default()) });
        assert_eq!(s.row(rid(0)).unwrap().status(), RowStatus::Empty);
    }
}
