//! An addon's manifest: what it serves, for which types and ids, and its catalogs.
//!
//! Two spellings of the same facts exist in the wild and both are read:
//!
//! * a resource is either a bare name (`"stream"`, inheriting the manifest's `types` and
//!   `idPrefixes`) or an object (`{"name":"stream","types":["movie"],"idPrefixes":["tt"]}`);
//! * a catalog's extras are either `extra: [{name, isRequired, options}]` or the legacy
//!   `extraSupported` / `extraRequired` name lists.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub resources: Vec<ResourceDecl>,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub id_prefixes: Option<Vec<String>>,
    #[serde(default)]
    pub catalogs: Vec<Catalog>,
    #[serde(default)]
    pub behavior_hints: ManifestHints,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestHints {
    #[serde(default)]
    pub adult: bool,
    #[serde(default)]
    pub p2p: bool,
    #[serde(default)]
    pub configurable: bool,
    #[serde(default)]
    pub configuration_required: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ResourceDecl {
    Name(String),
    Full {
        name: String,
        #[serde(default)]
        types: Option<Vec<String>>,
        #[serde(default, rename = "idPrefixes")]
        id_prefixes: Option<Vec<String>>,
    },
}

impl ResourceDecl {
    pub fn name(&self) -> &str {
        match self {
            ResourceDecl::Name(name) | ResourceDecl::Full { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    extra: Vec<Extra>,
    #[serde(default)]
    extra_supported: Vec<String>,
    #[serde(default)]
    extra_required: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Extra {
    pub name: String,
    #[serde(default)]
    pub is_required: bool,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub options_limit: Option<u32>,
}

impl Catalog {
    /// Every extra the catalog accepts, merging the modern and legacy spellings.
    pub fn extras(&self) -> Vec<Extra> {
        let mut out = self.extra.clone();
        for name in &self.extra_supported {
            if !out.iter().any(|e| &e.name == name) {
                out.push(Extra {
                    name: name.clone(),
                    is_required: self.extra_required.contains(name),
                    options: Vec::new(),
                    options_limit: None,
                });
            }
        }
        for extra in &mut out {
            if self.extra_required.contains(&extra.name) {
                extra.is_required = true;
            }
        }
        out
    }

    pub fn extra(&self, name: &str) -> Option<Extra> {
        self.extras().into_iter().find(|e| e.name == name)
    }

    pub fn supports(&self, name: &str) -> bool {
        self.extra(name).is_some()
    }

    /// Required extras other than the ones the caller is about to supply.
    pub fn unmet_required(&self, supplied: &[&str]) -> Vec<Extra> {
        self.extras()
            .into_iter()
            .filter(|e| e.is_required && !supplied.contains(&e.name.as_str()))
            .collect()
    }

    /// The genre choices, in the addon's order. Empty when the catalog takes no genre or
    /// declares it without options.
    pub fn genres(&self) -> Vec<String> {
        self.extra("genre").map(|e| e.options).unwrap_or_default()
    }

    /// The genre to send when the user has chosen none: a catalog that REQUIRES a genre answers
    /// nothing without one, so it opens on its first option. `None` when no genre is needed.
    pub fn default_genre(&self) -> Option<String> {
        let genre = self.extra("genre")?;
        if genre.is_required {
            genre.options.first().cloned()
        } else {
            None
        }
    }

    /// Can this catalog be shown as a plain row with nothing but an optional genre chosen?
    /// A catalog requiring `search` (a search-only catalog) cannot.
    pub fn is_browsable(&self) -> bool {
        self.unmet_required(&["genre", "skip"]).is_empty()
            && !(self.extra("genre").is_some_and(|g| g.is_required && g.options.is_empty()))
    }

    pub fn is_searchable(&self) -> bool {
        self.supports("search") && self.unmet_required(&["search", "genre", "skip"]).is_empty()
    }

    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

impl Manifest {
    pub fn parse(body: &[u8]) -> Result<Manifest, String> {
        serde_json::from_slice::<Manifest>(body).map(Manifest::named_catalogs).map_err(|e| e.to_string())
    }

    /// A catalog the addon gave no `name` is called by the addon's own name, not by its id
    /// ("publicdomainmovies" read as a row title). Idempotent.
    pub fn named_catalogs(mut self) -> Manifest {
        if !self.name.trim().is_empty() {
            for c in &mut self.catalogs {
                if c.name.as_deref().is_none_or(|n| n.trim().is_empty()) {
                    c.name = Some(self.name.clone());
                }
            }
        }
        self
    }

    /// Does this addon answer `resource` for an item of `kind` with `id`?
    pub fn serves(&self, resource: &str, kind: &str, id: &str) -> bool {
        self.resources.iter().any(|decl| {
            if decl.name() != resource {
                return false;
            }
            let (types, prefixes) = match decl {
                ResourceDecl::Name(_) => (Some(&self.types), self.id_prefixes.as_ref()),
                ResourceDecl::Full { types, id_prefixes, .. } => (
                    types.as_ref().or(Some(&self.types)),
                    id_prefixes.as_ref().or(self.id_prefixes.as_ref()),
                ),
            };
            let type_ok = types.is_none_or(|t| t.is_empty() || t.iter().any(|t| t == kind));
            let id_ok = prefixes.is_none_or(|p| p.is_empty() || p.iter().any(|p| id.starts_with(p.as_str())));
            type_ok && id_ok
        })
    }

    pub fn searchable_catalogs(&self) -> impl Iterator<Item = &Catalog> {
        self.catalogs.iter().filter(|c| c.is_searchable())
    }
}

/// An installed addon: where it lives and what it declared.
#[derive(Clone, Debug, PartialEq)]
pub struct Addon {
    /// The transport URL with `/manifest.json` removed, no trailing slash.
    pub base: String,
    pub manifest: Manifest,
}

impl Addon {
    /// `transport_url` is what a user installs: `https://host/[config/]manifest.json`.
    pub fn new(transport_url: &str, manifest: Manifest) -> Addon {
        Addon { base: base_of(transport_url), manifest: manifest.named_catalogs() }
    }
}

pub fn base_of(transport_url: &str) -> String {
    let url = transport_url.trim();
    let url = url.strip_suffix("/manifest.json").unwrap_or(url);
    url.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CINEMETA: &str = r#"{
        "id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta",
        "resources":["catalog","meta","addon_catalog"],
        "types":["movie","series"],"idPrefixes":["tt"],
        "catalogs":[
            {"type":"movie","id":"top","extra":[{"name":"search"},{"name":"genre","options":["Action","Drama"]},{"name":"skip"}]},
            {"type":"series","id":"top","extraSupported":["search","genre","skip"]},
            {"type":"movie","id":"year","extra":[{"name":"genre","isRequired":true,"options":["2026","2025"]},{"name":"skip"}]}
        ]}"#;

    const LIVE: &str = r#"{
        "id":"org.example.livetv","version":"1.0.0","name":"Live TV",
        "resources":[{"name":"stream","types":["tv"],"idPrefixes":["livetv:"]},"meta","catalog"],
        "types":["tv"],
        "catalogs":[
            {"type":"tv","id":"channels","name":"Channels","extra":[
                {"name":"genre","isRequired":true,"options":["Sport & News","Kids/Family"]},
                {"name":"search"}]}
        ]}"#;

    #[test]
    fn reads_both_extra_spellings() {
        let m = Manifest::parse(CINEMETA.as_bytes()).unwrap();
        assert!(m.catalogs[0].supports("search"));
        assert!(m.catalogs[1].supports("search"), "legacy extraSupported");
        assert!(m.catalogs[1].supports("skip"));
        assert_eq!(m.catalogs[0].genres(), ["Action", "Drama"]);
    }

    #[test]
    fn a_required_genre_opens_on_its_first_option() {
        let m = Manifest::parse(CINEMETA.as_bytes()).unwrap();
        assert_eq!(m.catalogs[0].default_genre(), None);
        assert_eq!(m.catalogs[2].default_genre().as_deref(), Some("2026"));
        assert!(m.catalogs[2].is_browsable());
        assert!(!m.catalogs[2].is_searchable());
    }

    #[test]
    fn search_covers_every_type_not_only_movie_and_series() {
        let m = Manifest::parse(LIVE.as_bytes()).unwrap();
        let found: Vec<_> = m.searchable_catalogs().map(|c| c.kind.as_str()).collect();
        assert_eq!(found, ["tv"]);
    }

    #[test]
    fn a_search_only_catalog_is_not_a_browse_row() {
        let m = Manifest::parse(
            br#"{"id":"x","catalogs":[{"type":"movie","id":"s","extra":[{"name":"search","isRequired":true}]}]}"#,
        )
        .unwrap();
        assert!(!m.catalogs[0].is_browsable());
        assert!(m.catalogs[0].is_searchable());
    }

    #[test]
    fn resource_objects_override_manifest_types_and_prefixes() {
        let m = Manifest::parse(LIVE.as_bytes()).unwrap();
        assert!(m.serves("stream", "tv", "livetv:42"));
        assert!(!m.serves("stream", "tv", "tt0111161"));
        assert!(!m.serves("stream", "movie", "livetv:42"));
        assert!(m.serves("meta", "tv", "anything"));
        let c = Manifest::parse(CINEMETA.as_bytes()).unwrap();
        assert!(c.serves("meta", "movie", "tt0111161"));
        assert!(!c.serves("meta", "movie", "kitsu:1"));
        assert!(!c.serves("stream", "movie", "tt0111161"));
    }

    #[test]
    fn base_strips_the_manifest_and_keeps_the_config_path() {
        assert_eq!(base_of("https://v3-cinemeta.strem.io/manifest.json"), "https://v3-cinemeta.strem.io");
        assert_eq!(base_of("https://a.example/cfg=abc/manifest.json"), "https://a.example/cfg=abc");
        assert_eq!(base_of("https://a.example/"), "https://a.example");
    }
    #[test]
    fn a_catalog_without_a_name_is_called_by_its_addon() {
        let m = Manifest::parse(br#"{"id":"pd","name":"Public Domain Movies","catalogs":[{"type":"movie","id":"publicdomainmovies"},{"type":"movie","id":"top","name":"Top"}]}"#).unwrap();
        assert_eq!(m.catalogs[0].display_name(), "Public Domain Movies");
        assert_eq!(m.catalogs[1].display_name(), "Top");
    }

}
