//! Paging a catalog to completion.
//!
//! Addons page with the `skip` extra, but they do not agree on a page size: Cinemeta answers 100
//! items (50 for some catalogs), many community addons 20 or 25, some whatever they have. A
//! client that stops when a page is shorter than a fixed size it assumed stops after the FIRST
//! page of every addon that pages by 20. So the end of a catalog is decided by what the addon
//! said, never by a guess at its page size:
//!
//! * an empty page ends it;
//! * a page with no id we have not already seen ends it (an addon that ignores `skip` returns
//!   page one again, forever);
//! * a catalog without the `skip` extra has exactly one page;
//! * [`MAX_PAGES`] bounds a misbehaving addon.
//!
//! The next `skip` is the number of items the addon has RETURNED so far, duplicates included,
//! because that is the offset the addon counts in.

use crate::manifest::Catalog;
use crate::types::{CatalogPage, MetaPreview};
use crate::url::resource_url;
use crate::{get_json, Fetch, FetchError};
use std::collections::HashSet;

/// Hard bound on pages fetched for one catalog view.
pub const MAX_PAGES: usize = 200;

/// Where a catalog's paging stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum More {
    /// Another page may exist; ask with this `skip`.
    Next(usize),
    /// The addon said there is nothing more.
    End,
    /// The last page failed; retrying asks for the same `skip` again.
    Failed { skip: usize, error: FetchError },
}

/// One catalog view: an addon catalog plus the chosen extras (genre, search), and the items
/// loaded so far.
#[derive(Clone, Debug)]
pub struct Pager {
    pub base: String,
    pub catalog: Catalog,
    pub genre: Option<String>,
    pub search: Option<String>,
    pub items: Vec<MetaPreview>,
    seen: HashSet<String>,
    returned: usize,
    pages: usize,
    pub more: More,
}

impl Pager {
    pub fn new(base: &str, catalog: Catalog, genre: Option<String>, search: Option<String>) -> Pager {
        let genre = genre.or_else(|| catalog.default_genre());
        Pager {
            base: base.trim_end_matches('/').to_string(),
            catalog,
            genre,
            search,
            items: Vec::new(),
            seen: HashSet::new(),
            returned: 0,
            pages: 0,
            more: More::Next(0),
        }
    }

    /// The URL of the next page, or `None` when the catalog has ended.
    pub fn next_url(&self) -> Option<String> {
        let skip = match &self.more {
            More::Next(skip) | More::Failed { skip, .. } => *skip,
            More::End => return None,
        };
        let skip_s = skip.to_string();
        let mut extra: Vec<(&str, &str)> = Vec::new();
        if let Some(genre) = self.genre.as_deref() {
            extra.push(("genre", genre));
        }
        if let Some(search) = self.search.as_deref() {
            extra.push(("search", search));
        }
        if skip > 0 {
            extra.push(("skip", &skip_s));
        }
        Some(resource_url(&self.base, "catalog", &self.catalog.kind, &self.catalog.id, &extra))
    }

    /// Fold one page's outcome in; returns how many new items it added.
    pub fn land(&mut self, outcome: Result<CatalogPage, FetchError>) -> usize {
        let skip = match &self.more {
            More::Next(skip) | More::Failed { skip, .. } => *skip,
            More::End => return 0,
        };
        let page = match outcome {
            Ok(page) => page,
            Err(FetchError::Cancelled) => return 0,
            Err(error) => {
                self.more = More::Failed { skip, error };
                return 0;
            }
        };
        self.pages += 1;
        self.returned = skip + page.metas.len();
        let before = self.items.len();
        for meta in page.metas {
            if self.seen.insert(meta.id.clone()) {
                self.items.push(meta);
            }
        }
        let added = self.items.len() - before;
        self.more = if added == 0 || !self.catalog.supports("skip") || self.pages >= MAX_PAGES {
            More::End
        } else {
            More::Next(self.returned)
        };
        added
    }

    /// Fetch and land the next page. Returns the number of new items, or `None` at the end.
    pub fn load_next(&mut self, fetch: &dyn Fetch) -> Option<usize> {
        let url = self.next_url()?;
        let outcome = get_json::<CatalogPage>(fetch, &url, &[]);
        Some(self.land(outcome))
    }

    /// Keep loading until at least `want` items are held or the catalog ends or fails.
    pub fn fill_to(&mut self, fetch: &dyn Fetch, want: usize) {
        while self.items.len() < want && matches!(self.more, More::Next(_)) {
            if self.load_next(fetch).is_none() {
                break;
            }
        }
    }

    pub fn has_more(&self) -> bool {
        matches!(self.more, More::Next(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFetch;
    use crate::manifest::Manifest;

    fn catalog(extra: &str) -> Catalog {
        let m = Manifest::parse(format!(r#"{{"id":"a","catalogs":[{{"type":"movie","id":"top","extra":{extra}}}]}}"#).as_bytes()).unwrap();
        m.catalogs[0].clone()
    }

    fn page(range: std::ops::Range<usize>) -> String {
        let metas: Vec<String> = range.map(|i| format!(r#"{{"id":"tt{i}","type":"movie","name":"M{i}"}}"#)).collect();
        format!(r#"{{"metas":[{}]}}"#, metas.join(","))
    }

    #[test]
    fn an_addon_paging_by_twenty_is_read_past_its_first_page() {
        let base = "https://a.example";
        let fetch = FakeFetch::default()
            .with("https://a.example/catalog/movie/top.json", &page(0..20))
            .with("https://a.example/catalog/movie/top/skip=20.json", &page(20..40))
            .with("https://a.example/catalog/movie/top/skip=40.json", &page(40..47))
            .with("https://a.example/catalog/movie/top/skip=47.json", &page(0..0));
        let mut p = Pager::new(base, catalog(r#"[{"name":"skip"}]"#), None, None);
        p.fill_to(&fetch, usize::MAX);
        assert_eq!(p.items.len(), 47);
        assert_eq!(p.more, More::End);
    }

    #[test]
    fn an_addon_that_ignores_skip_ends_instead_of_looping() {
        let fetch = FakeFetch::default()
            .with("https://a.example/catalog/movie/top.json", &page(0..10))
            .with("https://a.example/catalog/movie/top/skip=10.json", &page(0..10));
        let mut p = Pager::new("https://a.example", catalog(r#"[{"name":"skip"}]"#), None, None);
        p.fill_to(&fetch, usize::MAX);
        assert_eq!(p.items.len(), 10);
        assert_eq!(p.more, More::End);
        assert_eq!(fetch.asked.borrow().len(), 2);
    }

    #[test]
    fn a_catalog_without_skip_has_one_page() {
        let fetch = FakeFetch::default().with("https://a.example/catalog/movie/top.json", &page(0..100));
        let mut p = Pager::new("https://a.example", catalog("[]"), None, None);
        p.fill_to(&fetch, usize::MAX);
        assert_eq!((p.items.len(), p.has_more()), (100, false));
    }

    #[test]
    fn a_failed_page_keeps_what_loaded_and_retries_the_same_skip() {
        let fetch = FakeFetch::default()
            .with("https://a.example/catalog/movie/top.json", &page(0..20))
            .failing("https://a.example/catalog/movie/top/skip=20.json", FetchError::Status(503));
        let mut p = Pager::new("https://a.example", catalog(r#"[{"name":"skip"}]"#), None, None);
        p.fill_to(&fetch, usize::MAX);
        assert_eq!(p.items.len(), 20);
        assert_eq!(p.more, More::Failed { skip: 20, error: FetchError::Status(503) });
        assert_eq!(p.next_url().as_deref(), Some("https://a.example/catalog/movie/top/skip=20.json"));
    }

    #[test]
    fn a_required_genre_is_sent_and_encoded() {
        let c = catalog(r#"[{"name":"genre","isRequired":true,"options":["Sport & News"]},{"name":"skip"}]"#);
        let p = Pager::new("https://a.example", c, None, None);
        assert_eq!(p.next_url().as_deref(), Some("https://a.example/catalog/movie/top/genre=Sport%20%26%20News.json"));
    }

    #[test]
    fn genre_search_and_skip_compose() {
        let c = catalog(r#"[{"name":"genre"},{"name":"search"},{"name":"skip"}]"#);
        let mut p = Pager::new("https://a.example", c, Some("Drama".into()), Some("love".into()));
        p.more = More::Next(40);
        assert_eq!(
            p.next_url().as_deref(),
            Some("https://a.example/catalog/movie/top/genre=Drama&search=love&skip=40.json")
        );
    }
}
