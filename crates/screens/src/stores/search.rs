//! **The search store** — the query, one result row per searchable catalog of every addon the
//! user left switched on (`sn_stremio::search::plan`), the addon filter and the recent searches.
//!
//! - **Debounced.** Typing ([`SearchStore::type_query`]) arms a deadline [`DEBOUNCE_MS`] out and
//!   asks nothing; the frame's [`SearchStore::tick`] plans the rows once the deadline passes with
//!   no newer keystroke, so a word typed at speed is one request per catalog, not one per letter.
//!   A submit (OK on the field, a recent term) plans at once.
//! - **One row per catalog, each on its own.** Every row's first page is its own job; a slow or
//!   failing addon never holds back or blanks another row. A row reads Loading, Empty, Failed with
//!   its reason, or Results with "more" when the addon holds more than the row shows.
//! - **A newer query retires the older one.** Its requests are cancelled and a late landing for a
//!   query generation that is no longer current is dropped.
//! - **The addon filter is not kept here.** It is THE per-addon switch of the addons store
//!   (`AddonsStore::filter`, also the Addons page's *Include in search*), handed to every plan
//!   as an argument; a change to it (or to the addons in use) asks the shown query again
//!   ([`SearchStore::replan`]).
//! - **The recents are the user's setting**, seeded by the application from the settings file;
//!   its persistence watches [`SearchStore::settings_gen`].
//! - **Suggestions** ([`SearchStore::suggestions`]) complete what is being typed: the recents that
//!   start with or contain it, then the titles the searches of this session found for it — the
//!   rows' own landings, kept per query ([`MAX_CACHED`]) and read back for any longer text, so no
//!   request is made for them. A separate completion request to the first searchable addon was
//!   considered and not made: the typed query's own debounced search asks that same catalog
//!   [`DEBOUNCE_MS`] after the keystroke, and its titles are what a completion would return.
//!
//! Pure like every store here: time arrives as an argument and requests leave as [`SearchJob`]s.

use sn_stremio::manifest::Addon;
use sn_stremio::sched::Cancel;
use sn_stremio::search::{plan, AddonFilter, RowState, SearchRow, MIN_QUERY};
use sn_stremio::types::CatalogPage;
use sn_stremio::FetchError;

/// How long typing must pause before the query is asked.
pub const DEBOUNCE_MS: u32 = 400;
/// Recent searches kept, newest first.
pub const MAX_RECENTS: usize = 8;
/// Suggestions offered for the typed text at most.
pub const MAX_SUGGESTIONS: usize = 6;
/// Queries whose found titles the session keeps for suggestions; past this the oldest goes.
pub const MAX_CACHED: usize = 32;

/// A row's first page, to fetch off the frame thread.
#[derive(Clone, Debug)]
pub struct SearchJob {
    /// The query generation it belongs to.
    pub qgen: u32,
    pub row: usize,
    pub url: String,
    pub cancel: Cancel,
    /// "<addon> <type>/<catalog>" for the timing log; never the URL (it carries configuration).
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct SearchLanding {
    pub qgen: u32,
    pub row: usize,
    pub result: Result<CatalogPage, FetchError>,
}

/// Where the page as a whole stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No query long enough to ask (the recents are shown).
    Idle,
    /// Typing; the query is asked when it pauses.
    Typing,
    /// The rows of the current query (each with its own state).
    Rows,
    /// A query was asked and no enabled addon has a searchable catalog.
    NoCatalogs,
}

pub struct SearchStore {
    /// The text last typed, possibly not asked yet.
    typed: String,
    /// When the typed text is asked (ms on the frame clock), while typing.
    due: Option<u32>,
    /// The query the rows answer.
    query: String,
    qgen: u32,
    rows: Vec<SearchRow>,
    out: Vec<Option<Cancel>>,
    recents: Vec<String>,
    /// The titles each query of the session found, by lowercased query, oldest first.
    found: Vec<(String, Vec<String>)>,
    gen: u32,
    settings_gen: u32,
}

impl Default for SearchStore {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl SearchStore {
    pub fn new(recents: Vec<String>) -> Self {
        let mut recents = recents;
        recents.truncate(MAX_RECENTS);
        SearchStore {
            typed: String::new(),
            due: None,
            query: String::new(),
            qgen: 0,
            rows: Vec::new(),
            out: Vec::new(),
            recents,
            found: Vec::new(),
            gen: 0,
            settings_gen: 0,
        }
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    /// Moves whenever the filter or the recents change: what the application persists.
    pub fn settings_gen(&self) -> u32 {
        self.settings_gen
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn rows(&self) -> &[SearchRow] {
        &self.rows
    }

    pub fn row_state(&self, row: usize) -> Option<RowState> {
        let r = self.rows.get(row)?;
        // a row whose first page is still out reads Loading, whatever the pager says
        if r.pager.items.is_empty() && self.out.get(row).is_some_and(Option::is_some) {
            return Some(RowState::Loading);
        }
        Some(r.state())
    }

    pub fn recents(&self) -> &[String] {
        &self.recents
    }

    pub fn phase(&self) -> Phase {
        if self.due.is_some() {
            Phase::Typing
        } else if self.query.chars().count() < MIN_QUERY {
            Phase::Idle
        } else if self.rows.is_empty() {
            Phase::NoCatalogs
        } else {
            Phase::Rows
        }
    }

    fn bump(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    /// The field's text changed at `now`: the query is asked [`DEBOUNCE_MS`] after the last
    /// keystroke. Clearing the field clears the rows at once.
    pub fn type_query(&mut self, text: &str, now: u32) {
        if text == self.typed {
            return;
        }
        self.typed = text.to_string();
        if text.trim().chars().count() < MIN_QUERY {
            self.due = None;
            self.replace(String::new(), Vec::new());
        } else {
            self.due = Some(now.wrapping_add(DEBOUNCE_MS));
        }
        self.bump();
    }

    /// Ask `text` now, without waiting for typing to pause.
    pub fn submit(&mut self, text: &str, addons: &[Addon], filter: &AddonFilter) -> Vec<SearchJob> {
        self.typed = text.to_string();
        self.due = None;
        let query = text.trim().to_string();
        if query == self.query && !self.rows.is_empty() {
            self.bump();
            return Vec::new();
        }
        let rows = plan(addons, &query, filter);
        self.replace(query, rows)
    }

    /// The frame's clock: plan the typed query once its deadline has passed.
    pub fn tick(&mut self, now: u32, addons: &[Addon], filter: &AddonFilter) -> Vec<SearchJob> {
        match self.due {
            // wrapping-safe "now >= due"
            Some(due) if now.wrapping_sub(due) < u32::MAX / 2 => {
                let typed = self.typed.clone();
                self.submit(&typed, addons, filter)
            }
            _ => Vec::new(),
        }
    }

    /// The filter or the addons in use changed: ask the shown query again over the new set.
    /// Nothing when no query is shown.
    pub fn replan(&mut self, addons: &[Addon], filter: &AddonFilter) -> Vec<SearchJob> {
        if self.query.chars().count() < MIN_QUERY {
            return Vec::new();
        }
        let query = self.query.clone();
        let rows = plan(addons, &query, filter);
        self.replace(query, rows)
    }

    /// Ask a failed row again.
    pub fn retry(&mut self, row: usize) -> Vec<SearchJob> {
        let Some(r) = self.rows.get(row) else { return Vec::new() };
        if !matches!(r.state(), RowState::Failed(_)) || self.out[row].is_some() {
            return Vec::new();
        }
        let job = self.ask(row);
        self.bump();
        job.into_iter().collect()
    }

    /// Put `term` at the head of the recent searches.
    pub fn remember(&mut self, term: &str) {
        let term = term.trim();
        if term.chars().count() < MIN_QUERY {
            return;
        }
        let before = self.recents.clone();
        self.recents.retain(|t| !t.eq_ignore_ascii_case(term));
        self.recents.insert(0, term.to_string());
        self.recents.truncate(MAX_RECENTS);
        if self.recents != before {
            self.settings_gen = self.settings_gen.wrapping_add(1);
            self.bump();
        }
    }

    pub fn clear_recents(&mut self) {
        if !self.recents.is_empty() {
            self.recents.clear();
            self.settings_gen = self.settings_gen.wrapping_add(1);
            self.bump();
        }
    }

    /// Replace the rows with `rows` for `query`: the older query's requests are cancelled.
    fn replace(&mut self, query: String, rows: Vec<SearchRow>) -> Vec<SearchJob> {
        for c in self.out.drain(..).flatten() {
            c.cancel();
        }
        self.qgen = self.qgen.wrapping_add(1);
        self.query = query;
        self.out = vec![None; rows.len()];
        self.rows = rows;
        self.bump();
        (0..self.rows.len()).filter_map(|i| self.ask(i)).collect()
    }

    fn ask(&mut self, row: usize) -> Option<SearchJob> {
        let url = self.rows[row].pager.next_url()?;
        let cancel = Cancel::default();
        self.out[row] = Some(cancel.clone());
        let r = &self.rows[row];
        let label = format!("{} {}/{}", r.addon_name, r.pager.catalog.kind, r.pager.catalog.id);
        Some(SearchJob { qgen: self.qgen, row, url, cancel, label })
    }

    /// Fold a row's page in. `false` for a landing of a query no longer current.
    pub fn land(&mut self, landing: SearchLanding) -> bool {
        if landing.qgen != self.qgen || landing.row >= self.rows.len() {
            return false;
        }
        if self.out[landing.row].take().is_none() {
            return false;
        }
        if let Ok(page) = &landing.result {
            let key = self.query.to_lowercase();
            if !self.found.iter().any(|(q, _)| *q == key) {
                if self.found.len() >= MAX_CACHED {
                    self.found.remove(0);
                }
                self.found.push((key.clone(), Vec::new()));
            }
            if let Some((_, titles)) = self.found.iter_mut().find(|(q, _)| *q == key) {
                titles.extend(page.metas.iter().map(|m| title_of(&m.name)).filter(|t| !t.is_empty()));
            }
        }
        self.rows[landing.row].pager.land(landing.result);
        self.bump();
        true
    }

    /// What to offer for the typed text, at most [`MAX_SUGGESTIONS`], each once (whatever its
    /// case) and never the text itself: the recents that start with it, then those that contain
    /// it, then the titles found for the longest query of this session the text starts with that
    /// contain it, in the order they were found. Nothing for text too short to search.
    pub fn suggestions(&self) -> Vec<String> {
        let typed = self.typed.trim();
        if typed.chars().count() < MIN_QUERY {
            return Vec::new();
        }
        let q = typed.to_lowercase();
        let starts = self.recents.iter().filter(|t| t.to_lowercase().starts_with(&q));
        let contains = self.recents.iter().filter(|t| {
            let t = t.to_lowercase();
            !t.starts_with(&q) && t.contains(&q)
        });
        let found = self.found.iter().filter(|(k, _)| q.starts_with(k.as_str())).max_by_key(|(k, _)| k.len());
        let titles = found.into_iter().flat_map(|(_, t)| t.iter()).filter(|t| t.to_lowercase().contains(&q));
        let mut out: Vec<String> = Vec::new();
        for t in starts.chain(contains).chain(titles) {
            let l = t.to_lowercase();
            if l != q && !out.iter().any(|o| o.to_lowercase() == l) {
                out.push(t.clone());
                if out.len() == MAX_SUGGESTIONS {
                    break;
                }
            }
        }
        out
    }

    /// A row's pager, for the full grid its MORE opens (seeded with what the row already holds).
    pub fn pager_for(&self, key: &crate::stores::grid::GridKey) -> Option<&sn_stremio::catalog::Pager> {
        let wanted = key.search.as_deref()?;
        self.rows
            .iter()
            .find(|r| {
                r.addon_id == key.addon_id
                    && r.pager.catalog.kind == key.kind
                    && r.pager.catalog.id == key.catalog_id
                    && r.pager.search.as_deref() == Some(wanted)
            })
            .map(|r| &r.pager)
    }
}

/// A found title as a suggestion: trimmed, without a trailing year (`Heat (1995)` → `Heat`).
pub fn title_of(name: &str) -> String {
    let name = name.trim();
    let bare = name
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once('('))
        .filter(|(_, year)| year.len() == 4 && year.chars().all(|c| c.is_ascii_digit()))
        .map_or(name, |(head, _)| head.trim_end());
    bare.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_stremio::manifest::Manifest;
    use sn_stremio::types::MetaPreview;

    fn addons() -> Vec<Addon> {
        vec![
            Addon::new(
                "https://cinemeta.example/manifest.json",
                Manifest::parse(
                    br#"{"id":"cinemeta","name":"Cinemeta","catalogs":[
                    {"type":"movie","id":"top","extra":[{"name":"search"},{"name":"skip"}]},
                    {"type":"series","id":"top","extra":[{"name":"search"}]}]}"#,
                )
                .unwrap(),
            ),
            Addon::new(
                "https://live.example/manifest.json",
                Manifest::parse(br#"{"id":"live","name":"Live","catalogs":[{"type":"tv","id":"ch","extra":[{"name":"search"}]}]}"#).unwrap(),
            ),
        ]
    }

    fn page(n: usize) -> CatalogPage {
        CatalogPage { metas: (0..n).map(|i| MetaPreview { id: format!("tt{i}"), name: format!("M{i}"), ..Default::default() }).collect() }
    }

    #[test]
    fn typing_is_debounced_into_one_plan() {
        let mut s = SearchStore::default();
        let a = addons();
        s.type_query("b", 0);
        assert_eq!(s.phase(), Phase::Idle, "one letter is no query");
        s.type_query("ba", 100);
        s.type_query("bat", 200);
        assert_eq!(s.phase(), Phase::Typing);
        assert!(s.tick(200 + DEBOUNCE_MS - 1, &a, &AddonFilter::default()).is_empty(), "not before typing pauses");
        let jobs = s.tick(200 + DEBOUNCE_MS, &a, &AddonFilter::default());
        assert_eq!(jobs.len(), 3, "one request per searchable catalog, of every type");
        assert_eq!(jobs[2].url, "https://live.example/catalog/tv/ch/search=bat.json");
        assert_eq!(s.phase(), Phase::Rows);
        assert!(s.tick(10_000, &a, &AddonFilter::default()).is_empty(), "a planned query is asked once");
    }

    #[test]
    fn every_row_moves_through_its_own_states() {
        let mut s = SearchStore::default();
        let jobs = s.submit("batman", &addons(), &AddonFilter::default());
        assert!((0..3).all(|r| s.row_state(r) == Some(RowState::Loading)));
        let qgen = jobs[0].qgen;
        assert!(s.land(SearchLanding { qgen, row: 0, result: Ok(page(30)) }));
        assert!(s.land(SearchLanding { qgen, row: 1, result: Ok(page(0)) }));
        assert!(s.land(SearchLanding { qgen, row: 2, result: Err(FetchError::Status(502)) }));
        assert_eq!(s.row_state(0), Some(RowState::Results { count: 30, more: true }));
        assert_eq!(s.row_state(1), Some(RowState::Empty));
        assert_eq!(s.row_state(2), Some(RowState::Failed(FetchError::Status(502))));
        let again = s.retry(2);
        assert_eq!(again.len(), 1);
        assert_eq!(s.row_state(2), Some(RowState::Loading), "a retried row reads loading again");
    }

    #[test]
    fn a_newer_query_cancels_and_drops_the_older_ones_rows() {
        let mut s = SearchStore::default();
        let old = s.submit("batman", &addons(), &AddonFilter::default());
        let new = s.submit("superman", &addons(), &AddonFilter::default());
        assert!(old.iter().all(|j| j.cancel.is_cancelled()));
        assert!(!s.land(SearchLanding { qgen: old[0].qgen, row: 0, result: Ok(page(3)) }), "a stale landing is dropped");
        assert!(s.land(SearchLanding { qgen: new[0].qgen, row: 0, result: Ok(page(3)) }));
        assert!(s.rows()[0].pager.search.as_deref() == Some("superman"));
    }

    #[test]
    fn a_disabled_addon_is_not_asked_and_the_shown_query_follows_the_filter() {
        let mut s = SearchStore::default();
        let mut filter = AddonFilter::default();
        assert!(s.replan(&addons(), &filter).is_empty(), "no query shown, nothing asked");
        s.submit("news", &addons(), &filter);
        filter.toggle("cinemeta");
        let jobs = s.replan(&addons(), &filter);
        assert_eq!(jobs.len(), 1, "only the live addon's catalog is asked");
        filter.toggle("live");
        assert_eq!(s.replan(&addons(), &filter).len(), 0);
        assert_eq!(s.phase(), Phase::NoCatalogs, "every addon off reads as such, not as no results");
    }

    fn named(names: &[&str]) -> CatalogPage {
        CatalogPage { metas: names.iter().map(|n| MetaPreview { id: n.to_string(), name: n.to_string(), ..Default::default() }).collect() }
    }

    #[test]
    fn suggestions_are_matching_recents_then_found_titles_each_once() {
        let mut s = SearchStore::new(vec!["the batman".into(), "Batman Begins".into(), "news".into()]);
        s.type_query("bat", 0);
        assert_eq!(s.suggestions(), ["Batman Begins", "the batman"], "a recent that starts with it, then one that contains it");
        let jobs = s.submit("bat", &addons(), &AddonFilter::default());
        assert!(s.land(SearchLanding { qgen: jobs[0].qgen, row: 0, result: Ok(named(&["Batman (1989)", "BATMAN BEGINS", "Bat Out of Hell", "Robin"])) }));
        assert!(s.land(SearchLanding { qgen: jobs[0].qgen, row: 1, result: Ok(named(&["Batman", "Batwoman"])) }));
        assert_eq!(
            s.suggestions(),
            ["Batman Begins", "the batman", "Batman", "Bat Out of Hell", "Batwoman"],
            "found titles follow, without their year, case-insensitively once, only those that match"
        );
        s.type_query("batw", 10);
        assert_eq!(s.suggestions(), ["Batwoman"], "a longer text reads the shorter query's titles at once, no request");
        s.type_query("b", 20);
        assert!(s.suggestions().is_empty(), "too short to search");
        s.type_query("batman begins", 30);
        assert!(!s.suggestions().iter().any(|t| t.eq_ignore_ascii_case("batman begins")), "never the text itself");
        assert_eq!(title_of("Heat (1995)"), "Heat");
        assert_eq!(title_of("Blade Runner 2049"), "Blade Runner 2049", "a year in the title is the title");
        assert_eq!(title_of("Show (US)"), "Show (US)");
    }

    #[test]
    fn a_stale_landing_feeds_no_suggestions_and_the_cache_is_bounded() {
        let mut s = SearchStore::default();
        let old = s.submit("batman", &addons(), &AddonFilter::default());
        s.submit("superman", &addons(), &AddonFilter::default());
        assert!(!s.land(SearchLanding { qgen: old[0].qgen, row: 0, result: Ok(named(&["Batman Returns"])) }));
        s.type_query("batman", 0);
        assert!(s.suggestions().is_empty(), "the dropped landing is not remembered");
        for i in 0..MAX_CACHED + 4 {
            let jobs = s.submit(&format!("q{i}"), &addons(), &AddonFilter::default());
            s.land(SearchLanding { qgen: jobs[0].qgen, row: 0, result: Ok(named(&[&format!("q{i} title")])) });
        }
        assert_eq!(s.found.len(), MAX_CACHED);
        s.type_query("q0", 0);
        assert!(s.suggestions().is_empty(), "the oldest query went first");
        s.type_query(&format!("q{}", MAX_CACHED + 3), 0);
        assert_eq!(s.suggestions(), [format!("q{} title", MAX_CACHED + 3)]);
    }

    #[test]
    fn recents_are_newest_first_unique_and_bounded() {
        let mut s = SearchStore::default();
        for t in ["one", "two", "One", "x"] {
            s.remember(t);
        }
        assert_eq!(s.recents(), ["One", "two"], "a repeat moves to the top; a too-short term is not kept");
        for i in 0..20 {
            s.remember(&format!("term {i}"));
        }
        assert_eq!(s.recents().len(), MAX_RECENTS);
        s.clear_recents();
        assert!(s.recents().is_empty());
    }
}
