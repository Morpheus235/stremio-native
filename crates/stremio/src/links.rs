//! A meta's `links` (stremio-core's `Link`: `{name, category, url}`) as the places they open.
//!
//! Cinemeta and the addons built on its SDK send one link per genre, cast member and director,
//! each a `stremio:///` deep link: `stremio:///discover/{transportUrl}/{type}/{catalog}?genre=Drama`
//! (core's `DiscoverDeepLinks`) or `stremio:///search?search=Tim%20Robbins` (core spells its own
//! `?query=`; both are read). stremio-web's `MetaPreview` keeps a `stremio:` link only when its
//! path is one it can route, and groups the links by `category`; the categories read here are the
//! genres, the cast and the directors. Anything else — `imdb`, `share`, `Writers`, a web link, a
//! deep link to a route the app has no page for — is ignored.

use serde::Deserialize;

/// One entry of a meta's `links`, as the addon sent it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Link {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub url: String,
}

/// The link groups the Detail page shows, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Genres,
    Cast,
    Director,
}

impl Category {
    /// The group `category` names (case-insensitive, singular or plural); `None`: not shown.
    pub fn of(category: &str) -> Option<Category> {
        match category.trim().to_ascii_lowercase().as_str() {
            "genres" | "genre" => Some(Category::Genres),
            "cast" | "actors" | "stars" => Some(Category::Cast),
            "directors" | "director" => Some(Category::Director),
            _ => None,
        }
    }
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Discover on catalog `catalog` of type `kind` from the addon at `transport_url` (as the
    /// link spells it; `crate::manifest::base_of` compares it with an installed addon's), filtered
    /// to `genre`.
    Discover { transport_url: String, kind: String, catalog: String, genre: Option<String> },
    /// A search for this text.
    Search(String),
}

/// The place a `stremio:///` deep link opens, when it is one the app routes.
pub fn target(url: &str) -> Option<Target> {
    let rest = url.trim().strip_prefix("stremio://")?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let param = |name: &str| {
        query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| decode(k) == name).map(|(_, v)| decode(v)).filter(|v| !v.trim().is_empty())
    };
    let mut segs = path.trim_matches('/').split('/');
    match segs.next()? {
        "search" => param("search").or_else(|| param("query")).map(|q| Target::Search(q.trim().to_string())),
        "discover" => {
            let (transport_url, kind, catalog) = (decode(segs.next()?), decode(segs.next()?), decode(segs.next()?));
            if transport_url.is_empty() || kind.is_empty() || catalog.is_empty() {
                return None;
            }
            Some(Target::Discover { transport_url, kind, catalog, genre: param("genre") })
        }
        _ => None,
    }
}

/// `links` the Detail page shows: each kept link's group, name and target, in the addon's order.
pub fn routed(links: &[Link]) -> Vec<(Category, String, Target)> {
    links
        .iter()
        .filter_map(|l| {
            let name = l.name.trim();
            (!name.is_empty()).then_some(())?;
            Some((Category::of(&l.category)?, name.to_string(), target(&l.url)?))
        })
        .collect()
}

/// Percent-decoding (`encodeURIComponent`'s inverse; `+` reads as a space, as a form query
/// spells it). A malformed escape is kept as written; invalid UTF-8 is replaced.
pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match b[i] {
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discover_link_names_its_addon_type_catalog_and_genre() {
        assert_eq!(
            target("stremio:///discover/https%3A%2F%2Fcatalog.example.com%2Fmanifest.json/movie/top?genre=Sci-Fi%20%26%20Fantasy"),
            Some(Target::Discover {
                transport_url: "https://catalog.example.com/manifest.json".into(),
                kind: "movie".into(),
                catalog: "top".into(),
                genre: Some("Sci-Fi & Fantasy".into()),
            })
        );
        let plain = target("stremio:///discover/https%3A%2F%2Fcatalog.example.com%2Fmanifest.json/series/popular");
        assert!(matches!(plain, Some(Target::Discover { genre: None, .. })), "no genre: the catalog unfiltered");
        assert_eq!(target("stremio:///discover/https%3A%2F%2Fcatalog.example.com%2Fmanifest.json/movie"), None, "no catalog");
    }

    #[test]
    fn a_search_link_reads_search_or_query() {
        assert_eq!(target("stremio:///search?search=Tim%20Robbins"), Some(Target::Search("Tim Robbins".into())));
        assert_eq!(target("stremio:///search?query=Frank+Darabont"), Some(Target::Search("Frank Darabont".into())));
        assert_eq!(target("stremio:///search?search="), None, "nothing to search");
    }

    #[test]
    fn links_to_other_places_are_ignored() {
        for url in ["https://www.imdb.com/title/tt0111161/", "stremio:///detail/movie/tt1", "stremio:///library", "", "not a url"] {
            assert_eq!(target(url), None, "{url}");
        }
        let links: Vec<Link> = serde_json::from_str(
            r#"[
                {"name":"Drama","category":"Genres","url":"stremio:///discover/https%3A%2F%2Fcatalog.example.com%2Fmanifest.json/movie/top?genre=Drama"},
                {"name":"8.9","category":"imdb","url":"https://imdb.com/title/tt1"},
                {"name":"A Writer","category":"Writers","url":"stremio:///search?search=A%20Writer"},
                {"name":"Tim Robbins","category":"Cast","url":"stremio:///search?search=Tim%20Robbins"},
                {"name":"Frank Darabont","category":"Directors","url":"stremio:///search?search=Frank%20Darabont"},
                {"name":"  ","category":"Cast","url":"stremio:///search?search=x"},
                {"name":"Share","category":"share","url":"https://example.com/s"}
            ]"#,
        )
        .unwrap();
        let got: Vec<(Category, String)> = routed(&links).into_iter().map(|(c, n, _)| (c, n)).collect();
        assert_eq!(got, [(Category::Genres, "Drama".into()), (Category::Cast, "Tim Robbins".into()), (Category::Director, "Frank Darabont".into())]);
    }

    #[test]
    fn decoding_keeps_a_malformed_escape() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(decode("Ch%C3%A2teau"), "Château");
    }
}
