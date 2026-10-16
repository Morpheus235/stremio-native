//! Search fan-out: which catalogs a query goes to, and what each one's row says.
//!
//! A query goes to EVERY catalog that declares the `search` extra, in every installed addon the
//! user has left switched on, whatever the catalog's type: a live-TV addon's `tv` catalog is
//! searched exactly like Cinemeta's `movie` and `series` ones. Each catalog is its own
//! [`SearchRow`], so one slow or failing addon never holds back or blanks the others, and a row
//! states honestly whether it holds everything the addon found.

use crate::catalog::{More, Pager};
use crate::manifest::Addon;
use crate::FetchError;
use std::collections::BTreeSet;

/// Which addons a search asks. Empty `disabled` means all of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddonFilter {
    pub disabled: BTreeSet<String>,
}

impl AddonFilter {
    pub fn allows(&self, addon_id: &str) -> bool {
        !self.disabled.contains(addon_id)
    }
    pub fn toggle(&mut self, addon_id: &str) {
        if !self.disabled.remove(addon_id) {
            self.disabled.insert(addon_id.to_string());
        }
    }
}

/// How many items a result row shows before offering the rest.
pub const ROW_PREVIEW: usize = 20;

/// One catalog's answer to the query.
#[derive(Clone, Debug)]
pub struct SearchRow {
    pub addon_id: String,
    pub addon_name: String,
    pub pager: Pager,
}

/// What a row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    /// The first page is in flight.
    Loading,
    /// Nothing matched.
    Empty,
    /// The addon failed before returning anything.
    Failed(FetchError),
    /// Items to show. `more` says the addon holds more than are loaded (or than the row shows),
    /// so the row offers "see all" instead of pretending these are all the results.
    Results { count: usize, more: bool },
}

impl SearchRow {
    pub fn title(&self) -> String {
        format!("{} — {}", self.addon_name, self.pager.catalog.display_name())
    }

    pub fn state(&self) -> RowState {
        let count = self.pager.items.len();
        match (&self.pager.more, count) {
            (More::Next(0), 0) => RowState::Loading,
            (More::Failed { error, .. }, 0) => RowState::Failed(error.clone()),
            (_, 0) => RowState::Empty,
            (more, n) => RowState::Results { count: n, more: n > ROW_PREVIEW || matches!(more, More::Next(_)) },
        }
    }

    /// The items a row shows inline.
    pub fn preview(&self) -> &[crate::types::MetaPreview] {
        let n = self.pager.items.len().min(ROW_PREVIEW);
        &self.pager.items[..n]
    }
}

/// The rows for `query` over `addons`, in addon order then catalog order. A query shorter than
/// [`MIN_QUERY`] characters asks nothing.
pub fn plan(addons: &[Addon], query: &str, filter: &AddonFilter) -> Vec<SearchRow> {
    let query = query.trim();
    if query.chars().count() < MIN_QUERY {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for addon in addons.iter().filter(|a| filter.allows(&a.manifest.id)) {
        for catalog in addon.manifest.searchable_catalogs() {
            rows.push(SearchRow {
                addon_id: addon.manifest.id.clone(),
                addon_name: if addon.manifest.name.is_empty() { addon.manifest.id.clone() } else { addon.manifest.name.clone() },
                pager: Pager::new(&addon.base, catalog.clone(), None, Some(query.to_string())),
            });
        }
    }
    rows
}

pub const MIN_QUERY: usize = 2;

/// The addons a filter UI lists: every addon with at least one searchable catalog.
pub fn searchable_addons(addons: &[Addon]) -> Vec<(&str, &str)> {
    addons
        .iter()
        .filter(|a| a.manifest.searchable_catalogs().next().is_some())
        .map(|a| (a.manifest.id.as_str(), a.manifest.name.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFetch;
    use crate::manifest::Manifest;

    fn addon(url: &str, json: &str) -> Addon {
        Addon::new(url, Manifest::parse(json.as_bytes()).unwrap())
    }

    fn addons() -> Vec<Addon> {
        vec![
            addon(
                "https://cinemeta.example/manifest.json",
                r#"{"id":"cinemeta","name":"Cinemeta","catalogs":[
                    {"type":"movie","id":"top","extra":[{"name":"search"},{"name":"skip"}]},
                    {"type":"series","id":"top","extra":[{"name":"search"}]},
                    {"type":"movie","id":"popular","extra":[{"name":"skip"}]}]}"#,
            ),
            addon(
                "https://live.example/cfg/manifest.json",
                r#"{"id":"livetv","name":"Live TV","catalogs":[
                    {"type":"tv","id":"channels","extra":[{"name":"search"},{"name":"genre","options":["News"]}]}]}"#,
            ),
        ]
    }

    #[test]
    fn every_searchable_catalog_of_every_type_is_asked() {
        let rows = plan(&addons(), "news", &AddonFilter::default());
        let asked: Vec<_> = rows.iter().map(|r| r.pager.next_url().unwrap()).collect();
        assert_eq!(
            asked,
            [
                "https://cinemeta.example/catalog/movie/top/search=news.json",
                "https://cinemeta.example/catalog/series/top/search=news.json",
                "https://live.example/cfg/catalog/tv/channels/search=news.json",
            ]
        );
    }

    #[test]
    fn a_disabled_addon_is_not_asked() {
        let mut filter = AddonFilter::default();
        filter.toggle("cinemeta");
        let rows = plan(&addons(), "news", &filter);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].addon_id, "livetv");
        filter.toggle("cinemeta");
        assert_eq!(plan(&addons(), "news", &filter).len(), 3);
        assert_eq!(searchable_addons(&addons()), [("cinemeta", "Cinemeta"), ("livetv", "Live TV")]);
    }

    #[test]
    fn a_row_with_more_hits_than_it_shows_says_so() {
        let metas: Vec<String> = (0..100).map(|i| format!(r#"{{"id":"tt{i}","name":"M{i}"}}"#)).collect();
        let fetch = FakeFetch::default()
            .with("https://cinemeta.example/catalog/movie/top/search=batman.json", &format!(r#"{{"metas":[{}]}}"#, metas.join(",")))
            .with("https://cinemeta.example/catalog/series/top/search=batman.json", r#"{"metas":[{"id":"tt9","name":"S"}]}"#)
            .failing("https://live.example/cfg/catalog/tv/channels/search=batman.json", FetchError::Transport("timed out".into()));
        let mut rows = plan(&addons(), "batman", &AddonFilter::default());
        assert!(rows.iter().all(|r| r.state() == RowState::Loading));
        for row in &mut rows {
            row.pager.load_next(&fetch);
        }
        assert_eq!(rows[0].state(), RowState::Results { count: 100, more: true });
        assert_eq!(rows[0].preview().len(), ROW_PREVIEW);
        assert_eq!(rows[1].state(), RowState::Results { count: 1, more: false });
        assert_eq!(rows[2].state(), RowState::Failed(FetchError::Transport("timed out".into())));
    }

    #[test]
    fn a_too_short_query_asks_nothing() {
        assert!(plan(&addons(), " a ", &AddonFilter::default()).is_empty());
    }
}
