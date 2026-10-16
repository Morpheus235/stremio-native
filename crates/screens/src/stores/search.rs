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
//! - **The filter and the recents are the user's settings.** Both are seeded by the application
//!   from the settings file and its persistence watches [`SearchStore::settings_gen`].
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

/// A row's first page, to fetch off the frame thread.
#[derive(Clone, Debug)]
pub struct SearchJob {
    /// The query generation it belongs to.
    pub qgen: u32,
    pub row: usize,
    pub url: String,
    pub cancel: Cancel,
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
    filter: AddonFilter,
    recents: Vec<String>,
    gen: u32,
    settings_gen: u32,
}

impl Default for SearchStore {
    fn default() -> Self {
        Self::new(AddonFilter::default(), Vec::new())
    }
}

impl SearchStore {
    pub fn new(filter: AddonFilter, recents: Vec<String>) -> Self {
        let mut recents = recents;
        recents.truncate(MAX_RECENTS);
        SearchStore {
            typed: String::new(),
            due: None,
            query: String::new(),
            qgen: 0,
            rows: Vec::new(),
            out: Vec::new(),
            filter,
            recents,
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

    pub fn filter(&self) -> &AddonFilter {
        &self.filter
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
    pub fn submit(&mut self, text: &str, addons: &[Addon]) -> Vec<SearchJob> {
        self.typed = text.to_string();
        self.due = None;
        let query = text.trim().to_string();
        if query == self.query && !self.rows.is_empty() {
            self.bump();
            return Vec::new();
        }
        let rows = plan(addons, &query, &self.filter);
        self.replace(query, rows)
    }

    /// The frame's clock: plan the typed query once its deadline has passed.
    pub fn tick(&mut self, now: u32, addons: &[Addon]) -> Vec<SearchJob> {
        match self.due {
            // wrapping-safe "now >= due"
            Some(due) if now.wrapping_sub(due) < u32::MAX / 2 => {
                let typed = self.typed.clone();
                self.submit(&typed, addons)
            }
            _ => Vec::new(),
        }
    }

    /// Switch an addon in or out, and ask the current query again over the new set.
    pub fn toggle_addon(&mut self, addon_id: &str, addons: &[Addon]) -> Vec<SearchJob> {
        self.filter.toggle(addon_id);
        self.settings_gen = self.settings_gen.wrapping_add(1);
        let query = self.query.clone();
        let rows = plan(addons, &query, &self.filter);
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
        Some(SearchJob { qgen: self.qgen, row, url, cancel })
    }

    /// Fold a row's page in. `false` for a landing of a query no longer current.
    pub fn land(&mut self, landing: SearchLanding) -> bool {
        if landing.qgen != self.qgen || landing.row >= self.rows.len() {
            return false;
        }
        if self.out[landing.row].take().is_none() {
            return false;
        }
        self.rows[landing.row].pager.land(landing.result);
        self.bump();
        true
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
        assert!(s.tick(200 + DEBOUNCE_MS - 1, &a).is_empty(), "not before typing pauses");
        let jobs = s.tick(200 + DEBOUNCE_MS, &a);
        assert_eq!(jobs.len(), 3, "one request per searchable catalog, of every type");
        assert_eq!(jobs[2].url, "https://live.example/catalog/tv/ch/search=bat.json");
        assert_eq!(s.phase(), Phase::Rows);
        assert!(s.tick(10_000, &a).is_empty(), "a planned query is asked once");
    }

    #[test]
    fn every_row_moves_through_its_own_states() {
        let mut s = SearchStore::default();
        let jobs = s.submit("batman", &addons());
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
        let old = s.submit("batman", &addons());
        let new = s.submit("superman", &addons());
        assert!(old.iter().all(|j| j.cancel.is_cancelled()));
        assert!(!s.land(SearchLanding { qgen: old[0].qgen, row: 0, result: Ok(page(3)) }), "a stale landing is dropped");
        assert!(s.land(SearchLanding { qgen: new[0].qgen, row: 0, result: Ok(page(3)) }));
        assert!(s.rows()[0].pager.search.as_deref() == Some("superman"));
    }

    #[test]
    fn a_disabled_addon_is_not_asked_and_the_change_is_a_setting() {
        let mut s = SearchStore::default();
        s.submit("news", &addons());
        let settings = s.settings_gen();
        let jobs = s.toggle_addon("cinemeta", &addons());
        assert_eq!(jobs.len(), 1, "only the live addon's catalog is asked");
        assert!(!s.filter().allows("cinemeta"));
        assert_ne!(s.settings_gen(), settings, "the filter is persisted");
        assert_eq!(s.toggle_addon("live", &addons()).len(), 0);
        assert_eq!(s.phase(), Phase::NoCatalogs, "every addon off reads as such, not as no results");
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
