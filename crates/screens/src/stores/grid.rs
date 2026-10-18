//! **The grid store** — one paginated catalog view per [`GridKey`] a page holds open: the full
//! grid a search row's MORE opens, and Discover's chosen catalog under its chosen genre.
//!
//! Pure like every store here: [`GridStore::open`] and [`GridStore::more`] hand back [`GridJob`]s
//! (a URL and a [`Cancel`] token), the application runs them over `sn_stremio::Fetch` and folds
//! the outcome back in with [`GridStore::land`]. A view is reference-counted by the pages that
//! hold it, so a grid pushed twice (Back and forward again) shares one pager, and the last close
//! cancels its request and forgets it.
//!
//! The paging rules are the [`Pager`]'s: the end of a catalog is what the addon said, never a
//! guess at its page size; a failed page keeps what loaded and is asked again on a retry.

use std::collections::HashMap;

use sn_machine::machine::Canon;
use sn_stremio::catalog::{More, Pager};
use sn_stremio::manifest::{Addon, Catalog};
use sn_stremio::sched::Cancel;
use sn_stremio::types::{CatalogPage, MetaPreview};
use sn_stremio::FetchError;

/// One catalog view: which addon (its manifest id), which catalog, and the extras chosen for it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GridKey {
    pub addon_id: String,
    pub kind: String,
    pub catalog_id: String,
    /// The `genre` extra, when one is chosen. A catalog that REQUIRES a genre and is opened
    /// without one gets its first option (`Catalog::default_genre`, applied by the pager).
    pub genre: Option<String>,
    /// The `search` extra: a search row's MORE.
    pub search: Option<String>,
}

impl GridKey {
    pub fn write(&self, c: &mut Canon) {
        c.str(&self.addon_id).str(&self.kind).str(&self.catalog_id);
        c.option(self.genre.as_deref(), |c, g| {
            c.str(g);
        });
        c.option(self.search.as_deref(), |c, s| {
            c.str(s);
        });
    }

    /// The addon and catalog this key names among `addons`.
    pub fn resolve<'a>(&self, addons: &'a [Addon]) -> Option<(&'a Addon, &'a Catalog)> {
        let addon = addons.iter().find(|a| a.manifest.id == self.addon_id)?;
        let catalog = addon.manifest.catalogs.iter().find(|c| c.kind == self.kind && c.id == self.catalog_id)?;
        Some((addon, catalog))
    }
}

/// What a grid shows right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridState {
    /// The addon this key names is not installed (any more): nothing will load.
    Missing,
    /// The first page is out.
    Loading,
    /// The catalog holds nothing (for this genre or query).
    Empty,
    /// The first page failed.
    Failed(FetchError),
    /// Cards to show; `more` says whether the catalog goes on.
    Ready { more: bool },
    /// Cards to show, and the page after them failed: the catalog may go on, a retry asks it again.
    MoreFailed,
}

struct View {
    pager: Option<Pager>,
    holders: u32,
    /// The request out for this view's next page.
    out: Option<Cancel>,
}

/// A request for one view's next page.
#[derive(Clone, Debug)]
pub struct GridJob {
    pub key: GridKey,
    pub url: String,
    pub cancel: Cancel,
}

/// A page request's outcome, handed back on the frame thread.
#[derive(Clone, Debug)]
pub struct GridLanding {
    pub key: GridKey,
    pub result: Result<CatalogPage, FetchError>,
}

#[derive(Default)]
pub struct GridStore {
    views: HashMap<GridKey, View>,
    gen: u32,
}

impl GridStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves whenever anything a page reads here changes.
    pub fn gen(&self) -> u32 {
        self.gen
    }

    /// A page holds `key` open. The first holder resolves it against `addons` and asks its first
    /// page; `seed` is a pager already holding the first page (a search row's), so its grid opens
    /// on the cards the row showed instead of asking for them again.
    pub fn open(&mut self, addons: &[Addon], key: GridKey, seed: Option<Pager>) -> Vec<GridJob> {
        if let Some(view) = self.views.get_mut(&key) {
            view.holders += 1;
            return Vec::new();
        }
        let pager = seed.or_else(|| {
            key.resolve(addons).map(|(addon, catalog)| Pager::new(&addon.base, catalog.clone(), key.genre.clone(), key.search.clone()))
        });
        self.views.insert(key.clone(), View { pager, holders: 1, out: None });
        self.gen = self.gen.wrapping_add(1);
        self.ask(&key).into_iter().collect()
    }

    /// A page let go of `key`. The last holder cancels the request out and forgets the view.
    pub fn close(&mut self, key: &GridKey) {
        let Some(view) = self.views.get_mut(key) else { return };
        view.holders = view.holders.saturating_sub(1);
        if view.holders == 0 {
            if let Some(cancel) = view.out.take() {
                cancel.cancel();
            }
            self.views.remove(key);
            self.gen = self.gen.wrapping_add(1);
        }
    }

    /// The grid reached the end of what is loaded: ask for the next page if the catalog goes on
    /// and nothing is out already.
    pub fn more(&mut self, key: &GridKey) -> Vec<GridJob> {
        match self.views.get(key).and_then(|v| v.pager.as_ref()) {
            Some(p) if p.has_more() => self.ask(key).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Ask a failed page again.
    pub fn retry(&mut self, key: &GridKey) -> Vec<GridJob> {
        match self.views.get(key).and_then(|v| v.pager.as_ref()) {
            Some(p) if matches!(p.more, More::Failed { .. }) => self.ask(key).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    fn ask(&mut self, key: &GridKey) -> Option<GridJob> {
        let view = self.views.get_mut(key)?;
        if view.out.is_some() {
            return None;
        }
        let url = view.pager.as_ref()?.next_url()?;
        let cancel = Cancel::default();
        view.out = Some(cancel.clone());
        Some(GridJob { key: key.clone(), url, cancel })
    }

    /// Fold a page in. `false` when nothing a page reads moved (the view was closed meanwhile).
    pub fn land(&mut self, landing: GridLanding) -> bool {
        let Some(view) = self.views.get_mut(&landing.key) else { return false };
        if view.out.take().is_none() {
            return false;
        }
        let Some(pager) = view.pager.as_mut() else { return false };
        pager.land(landing.result);
        self.gen = self.gen.wrapping_add(1);
        true
    }

    /// The view's state, or `None` when no page holds it.
    pub fn state(&self, key: &GridKey) -> Option<GridState> {
        let view = self.views.get(key)?;
        let Some(pager) = view.pager.as_ref() else { return Some(GridState::Missing) };
        Some(match (&pager.more, pager.items.is_empty()) {
            (More::Failed { error, .. }, true) => GridState::Failed(error.clone()),
            (More::Next(_), true) => GridState::Loading,
            (More::End, true) => GridState::Empty,
            (More::Failed { .. }, false) => GridState::MoreFailed,
            (more, false) => GridState::Ready { more: matches!(more, More::Next(_)) },
        })
    }

    pub fn items(&self, key: &GridKey) -> &[MetaPreview] {
        self.views.get(key).and_then(|v| v.pager.as_ref()).map_or(&[], |p| p.items.as_slice())
    }

    /// The URL of the next page the view would ask for (the tests read what a genre encodes to).
    pub fn next_url(&self, key: &GridKey) -> Option<String> {
        self.views.get(key)?.pager.as_ref()?.next_url()
    }

    /// Is a request out for `key`?
    pub fn loading(&self, key: &GridKey) -> bool {
        self.views.get(key).is_some_and(|v| v.out.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_stremio::manifest::Manifest;

    fn addons() -> Vec<Addon> {
        vec![Addon::new(
            "https://live.example/cfg/manifest.json",
            Manifest::parse(
                br#"{"id":"live","name":"Live","catalogs":[
                    {"type":"tv","id":"channels","name":"Channels","extra":[{"name":"genre","isRequired":true,"options":["News & Talk","Sport"]},{"name":"skip"}]}]}"#,
            )
            .unwrap(),
        )]
    }

    fn key(genre: Option<&str>) -> GridKey {
        GridKey { addon_id: "live".into(), kind: "tv".into(), catalog_id: "channels".into(), genre: genre.map(Into::into), search: None }
    }

    fn page(n: usize) -> CatalogPage {
        CatalogPage { metas: (0..n).map(|i| MetaPreview { id: format!("ch{i}"), kind: "tv".into(), name: format!("C{i}"), ..Default::default() }).collect() }
    }

    #[test]
    fn a_required_genre_defaults_to_the_first_option_and_a_chosen_one_is_encoded() {
        let mut s = GridStore::new();
        let jobs = s.open(&addons(), key(None), None);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].url, "https://live.example/cfg/catalog/tv/channels/genre=News%20%26%20Talk.json");
        let jobs = s.open(&addons(), key(Some("Sport")), None);
        assert_eq!(jobs[0].url, "https://live.example/cfg/catalog/tv/channels/genre=Sport.json");
    }

    #[test]
    fn pages_follow_and_the_last_holder_cancels() {
        let mut s = GridStore::new();
        let k = key(Some("Sport"));
        let first = s.open(&addons(), k.clone(), None);
        assert_eq!(s.state(&k), Some(GridState::Loading));
        assert!(s.open(&addons(), k.clone(), None).is_empty(), "a second holder shares the view");
        assert!(s.more(&k).is_empty(), "nothing is asked twice while a page is out");
        assert!(s.land(GridLanding { key: k.clone(), result: Ok(page(20)) }));
        assert_eq!(s.state(&k), Some(GridState::Ready { more: true }));
        let next = s.more(&k);
        assert_eq!(next[0].url, "https://live.example/cfg/catalog/tv/channels/genre=Sport&skip=20.json");
        s.close(&k);
        assert!(!next[0].cancel.is_cancelled(), "another page still holds it");
        s.close(&k);
        assert!(next[0].cancel.is_cancelled(), "the last close cancels the request out");
        assert_eq!(s.state(&k), None);
        assert!(!s.land(GridLanding { key: k, result: Ok(page(5)) }), "a late page for a closed view is dropped");
        let _ = first;
    }

    #[test]
    fn a_failed_first_page_reads_failed_and_retries() {
        let mut s = GridStore::new();
        let k = key(None);
        s.open(&addons(), k.clone(), None);
        s.land(GridLanding { key: k.clone(), result: Err(FetchError::Status(503)) });
        assert_eq!(s.state(&k), Some(GridState::Failed(FetchError::Status(503))));
        assert_eq!(s.retry(&k).len(), 1);
    }

    /// A later page failing is not the end of the catalog: it reads as its own state (the page
    /// offers *Try again*) and a retry asks the same page again.
    #[test]
    fn a_failed_next_page_is_not_the_end_and_retries() {
        let mut s = GridStore::new();
        let k = key(None);
        s.open(&addons(), k.clone(), None);
        s.land(GridLanding { key: k.clone(), result: Ok(page(20)) });
        let next = s.more(&k);
        s.land(GridLanding { key: k.clone(), result: Err(FetchError::Status(503)) });
        assert_eq!(s.state(&k), Some(GridState::MoreFailed));
        assert_eq!(s.retry(&k)[0].url, next[0].url, "the same page, asked again");
        assert_eq!(s.items(&k).len(), 20, "what loaded stays");
    }

    #[test]
    fn an_addon_that_is_not_installed_is_missing_not_loading() {
        let mut s = GridStore::new();
        let k = GridKey { addon_id: "gone".into(), ..key(None) };
        assert!(s.open(&addons(), k.clone(), None).is_empty());
        assert_eq!(s.state(&k), Some(GridState::Missing));
    }
}
