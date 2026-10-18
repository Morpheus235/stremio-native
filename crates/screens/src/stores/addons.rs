//! **The addons store** — THE answer to "which addons does the app use", and the one place that
//! answer changes:
//!
//! - signed out (or before anything is known): [`InUse::Defaults`], `sn_stremio::account::
//!   DEFAULT_ADDONS` (Cinemeta), whose manifests the catalog store fetches;
//! - signed in: [`InUse::Installed`], the account's addon collection in its own order, with the
//!   manifests the collection already carries (nothing to fetch per addon).
//!
//! **Cache, then refresh.** Signing in (or a boot that restored a session) asks for the copy kept
//! on the device AND the account's collection at once. The device copy shows the last known addons
//! immediately; the account's answer replaces it when it lands, and is then kept for the next boot.
//! A device copy that arrives AFTER the account's answer is dropped: the order of the two landings
//! never puts an older list on screen. A failed refresh keeps whatever is shown and says so
//! ([`Refresh::Failed`]); with nothing cached it falls back to the defaults, so Home still browses.
//!
//! **Search's per-addon switch** (`sn_stremio::search::AddonFilter`) lives here too: the Addons
//! page toggles it, the search fan-out reads it, and it is persisted ([`AddonsJob::WriteFilter`]).
//! So is the Streams page's (its addon dropdown: [`AddonsStore::stream_filter`], an addon switched
//! off is not asked for streams; [`AddonsJob::WriteStreamFilter`]), on the device only.
//!
//! The accessor every screen and the application read is [`AddonsStore::in_use`]; the ready
//! manifests of whatever is in use are the catalog store's `addons()` (`registry::Views::addons`).

use sn_stremio::account::{ApiError, DEFAULT_ADDONS};
use sn_stremio::manifest::Addon;
use sn_stremio::search::AddonFilter;

use super::account::Secret;

/// Which addons the app uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InUse<'a> {
    /// The defaults, as transport URLs (their manifests are fetched).
    Defaults(&'static [&'static str]),
    /// The account's collection, manifests included.
    Installed(&'a [Addon]),
}

/// Where the list on screen came from.
#[derive(Clone, Debug, PartialEq)]
pub enum Collection {
    Defaults,
    /// The device's copy of the account's collection, shown until the account answers.
    Cached(Vec<Addon>),
    /// The account's own answer.
    Fresh(Vec<Addon>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refresh {
    Idle,
    Loading,
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum AddonsJob {
    ReadCache { user_id: String },
    Fetch { user_id: String, auth_key: Secret },
    WriteCache { user_id: String, bytes: Vec<u8> },
    DropCache { user_id: String },
    ReadFilter,
    WriteFilter(AddonFilter),
    /// The Streams page's per-addon switch, kept on the device.
    ReadStreamFilter,
    WriteStreamFilter(AddonFilter),
}

#[derive(Clone, Debug)]
pub enum AddonsLanding {
    CacheRead { user_id: String, addons: Option<Vec<Addon>> },
    Fetched { user_id: String, result: Result<(Vec<Addon>, Vec<u8>), ApiError> },
    FilterRead(AddonFilter),
    StreamFilterRead(AddonFilter),
}

pub struct AddonsStore {
    collection: Collection,
    refresh: Refresh,
    /// The account the collection belongs to; `None` signed out.
    user: Option<(String, Secret)>,
    filter: AddonFilter,
    /// The addons switched off for stream queries (the Streams page's chips, held).
    streams: AddonFilter,
    gen: u32,
    /// Moves only when [`Self::in_use`] changes (what the catalog store is rebuilt on).
    epoch: u32,
}

impl Default for AddonsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AddonsStore {
    pub fn new() -> Self {
        AddonsStore { collection: Collection::Defaults, refresh: Refresh::Idle, user: None, filter: AddonFilter::default(), streams: AddonFilter::default(), gen: 0, epoch: 0 }
    }

    /// The boot's jobs: the persisted search and stream switches.
    pub fn boot(&mut self) -> Vec<AddonsJob> {
        vec![AddonsJob::ReadFilter, AddonsJob::ReadStreamFilter]
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    fn changed(&mut self, in_use: bool) {
        self.gen = self.gen.wrapping_add(1);
        if in_use {
            self.epoch = self.epoch.wrapping_add(1);
        }
    }

    /// **The addons the app uses.**
    pub fn in_use(&self) -> InUse<'_> {
        match &self.collection {
            Collection::Defaults => InUse::Defaults(DEFAULT_ADDONS),
            Collection::Cached(a) | Collection::Fresh(a) => InUse::Installed(a),
        }
    }

    pub fn collection(&self) -> &Collection {
        &self.collection
    }

    pub fn refresh(&self) -> &Refresh {
        &self.refresh
    }

    pub fn signed_in(&self) -> bool {
        self.user.is_some()
    }

    /// Search's per-addon switches.
    pub fn filter(&self) -> &AddonFilter {
        &self.filter
    }

    /// The addons a stream list asks (an addon switched off on the Streams page is not).
    pub fn stream_filter(&self) -> &AddonFilter {
        &self.streams
    }

    /// Switch `addon_id` off for stream queries, or back on (a stream list's addon dropdown).
    pub fn toggle_streams(&mut self, addon_id: &str) -> Vec<AddonsJob> {
        self.streams.toggle(addon_id);
        self.changed(false);
        vec![AddonsJob::WriteStreamFilter(self.streams.clone())]
    }

    /// Is `addon_id` asked by a search?
    pub fn searched(&self, addon_id: &str) -> bool {
        self.filter.allows(addon_id)
    }

    /// The account `user_id` is signed in: show its device copy, ask for its collection.
    pub fn sign_in(&mut self, user_id: &str, auth_key: &Secret) -> Vec<AddonsJob> {
        if self.user.as_ref().is_some_and(|(u, _)| u == user_id) {
            return Vec::new();
        }
        self.user = Some((user_id.to_string(), auth_key.clone()));
        self.refresh = Refresh::Loading;
        self.changed(false);
        vec![AddonsJob::ReadCache { user_id: user_id.to_string() }, AddonsJob::Fetch { user_id: user_id.to_string(), auth_key: auth_key.clone() }]
    }

    /// Ask the account again (a press on a failed refresh's read-out).
    pub fn retry(&mut self) -> Vec<AddonsJob> {
        let Some((user_id, key)) = self.user.clone() else { return Vec::new() };
        if self.refresh == Refresh::Loading {
            return Vec::new();
        }
        self.refresh = Refresh::Loading;
        self.changed(false);
        vec![AddonsJob::Fetch { user_id, auth_key: key }]
    }

    /// Signed out: back to the defaults; the departed account's device copy is removed.
    pub fn sign_out(&mut self) -> Vec<AddonsJob> {
        let Some((user_id, _)) = self.user.take() else { return Vec::new() };
        let was_installed = self.collection != Collection::Defaults;
        self.collection = Collection::Defaults;
        self.refresh = Refresh::Idle;
        self.changed(was_installed);
        vec![AddonsJob::DropCache { user_id }]
    }

    /// Switch search off (or back on) for `addon_id`.
    pub fn toggle_search(&mut self, addon_id: &str) -> Vec<AddonsJob> {
        self.filter.toggle(addon_id);
        self.changed(false);
        vec![AddonsJob::WriteFilter(self.filter.clone())]
    }

    pub fn land(&mut self, landing: AddonsLanding) -> Vec<AddonsJob> {
        match landing {
            AddonsLanding::CacheRead { user_id, addons } => {
                if !self.is_user(&user_id) {
                    return Vec::new();
                }
                // the account answered first: its list stands
                if matches!(self.collection, Collection::Fresh(_)) {
                    return Vec::new();
                }
                match addons {
                    Some(a) if !a.is_empty() => {
                        let same = self.installed() == Some(&a[..]);
                        self.collection = Collection::Cached(a);
                        self.changed(!same);
                    }
                    _ => {}
                }
                Vec::new()
            }
            AddonsLanding::Fetched { user_id, result } => {
                if !self.is_user(&user_id) {
                    return Vec::new();
                }
                match result {
                    Ok((addons, bytes)) => {
                        let same = self.installed() == Some(&addons[..]);
                        self.collection = Collection::Fresh(addons);
                        self.refresh = Refresh::Idle;
                        self.changed(!same);
                        vec![AddonsJob::WriteCache { user_id, bytes }]
                    }
                    Err(e) => {
                        self.refresh = Refresh::Failed(super::api_reason(&e));
                        self.changed(false);
                        Vec::new()
                    }
                }
            }
            AddonsLanding::FilterRead(filter) => {
                if filter != self.filter {
                    self.filter = filter;
                    self.changed(false);
                }
                Vec::new()
            }
            AddonsLanding::StreamFilterRead(filter) => {
                if filter != self.streams {
                    self.streams = filter;
                    self.changed(false);
                }
                Vec::new()
            }
        }
    }

    fn is_user(&self, user_id: &str) -> bool {
        self.user.as_ref().is_some_and(|(u, _)| u == user_id)
    }

    fn installed(&self) -> Option<&[Addon]> {
        match &self.collection {
            Collection::Defaults => None,
            Collection::Cached(a) | Collection::Fresh(a) => Some(a),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sn_stremio::manifest::Manifest;
    use sn_stremio::FetchError;

    pub(crate) fn addon(id: &str, base: &str) -> Addon {
        let m: Manifest = serde_json::from_str(&format!(
            r#"{{"id":"{id}","name":"{id}","resources":["catalog"],"types":["movie"],"catalogs":[{{"type":"movie","id":"top","name":"Top"}}]}}"#
        ))
        .unwrap();
        Addon::new(&format!("{base}/manifest.json"), m)
    }

    fn ids(s: &AddonsStore) -> Vec<String> {
        match s.in_use() {
            InUse::Defaults(urls) => urls.iter().map(|u| u.to_string()).collect(),
            InUse::Installed(a) => a.iter().map(|a| a.manifest.id.clone()).collect(),
        }
    }

    #[test]
    fn signed_out_uses_the_defaults() {
        let s = AddonsStore::new();
        assert_eq!(s.in_use(), InUse::Defaults(DEFAULT_ADDONS));
    }

    #[test]
    fn the_device_copy_shows_first_and_the_accounts_answer_replaces_it() {
        let mut s = AddonsStore::new();
        let jobs = s.sign_in("u1", &Secret::new("k"));
        assert!(matches!(&jobs[..], [AddonsJob::ReadCache { .. }, AddonsJob::Fetch { .. }]));
        s.land(AddonsLanding::CacheRead { user_id: "u1".into(), addons: Some(vec![addon("old", "https://a.example")]) });
        assert_eq!(ids(&s), ["old"]);
        assert!(matches!(s.collection(), Collection::Cached(_)));
        let epoch = s.epoch();
        let jobs = s.land(AddonsLanding::Fetched {
            user_id: "u1".into(),
            result: Ok((vec![addon("new", "https://b.example"), addon("old", "https://a.example")], b"{}".to_vec())),
        });
        assert_eq!(ids(&s), ["new", "old"], "the account's order");
        assert!(matches!(&jobs[..], [AddonsJob::WriteCache { user_id, .. }] if user_id == "u1"), "and it is kept for the next boot");
        assert_ne!(s.epoch(), epoch);
    }

    #[test]
    fn a_device_copy_landing_after_the_accounts_answer_is_dropped() {
        let mut s = AddonsStore::new();
        s.sign_in("u1", &Secret::new("k"));
        s.land(AddonsLanding::Fetched { user_id: "u1".into(), result: Ok((vec![addon("new", "https://b.example")], Vec::new())) });
        s.land(AddonsLanding::CacheRead { user_id: "u1".into(), addons: Some(vec![addon("old", "https://a.example")]) });
        assert_eq!(ids(&s), ["new"]);
    }

    #[test]
    fn an_identical_refresh_does_not_rebuild_what_is_on_screen() {
        let mut s = AddonsStore::new();
        s.sign_in("u1", &Secret::new("k"));
        s.land(AddonsLanding::CacheRead { user_id: "u1".into(), addons: Some(vec![addon("a", "https://a.example")]) });
        let epoch = s.epoch();
        s.land(AddonsLanding::Fetched { user_id: "u1".into(), result: Ok((vec![addon("a", "https://a.example")], Vec::new())) });
        assert_eq!(s.epoch(), epoch, "the same list: the catalog keeps its rows");
    }

    #[test]
    fn a_failed_refresh_keeps_what_is_shown_and_signing_out_returns_to_the_defaults() {
        let mut s = AddonsStore::new();
        s.sign_in("u1", &Secret::new("k"));
        s.land(AddonsLanding::CacheRead { user_id: "u1".into(), addons: Some(vec![addon("a", "https://a.example")]) });
        s.land(AddonsLanding::Fetched { user_id: "u1".into(), result: Err(ApiError::Fetch(FetchError::Status(502))) });
        assert_eq!(ids(&s), ["a"]);
        assert_eq!(s.refresh(), &Refresh::Failed("HTTP 502".into()));
        assert!(matches!(&s.retry()[..], [AddonsJob::Fetch { .. }]));
        let jobs = s.sign_out();
        assert!(matches!(&jobs[..], [AddonsJob::DropCache { user_id }] if user_id == "u1"));
        assert_eq!(s.in_use(), InUse::Defaults(DEFAULT_ADDONS));
        // a late landing for the departed account changes nothing
        s.land(AddonsLanding::Fetched { user_id: "u1".into(), result: Ok((vec![addon("x", "https://x.example")], Vec::new())) });
        assert_eq!(s.in_use(), InUse::Defaults(DEFAULT_ADDONS));
    }

    #[test]
    fn the_search_switch_is_shared_with_the_search_filter_and_persisted() {
        let mut s = AddonsStore::new();
        assert!(s.searched("com.linvo.cinemeta"));
        let jobs = s.toggle_search("com.linvo.cinemeta");
        assert!(!s.searched("com.linvo.cinemeta"));
        assert!(!s.filter().allows("com.linvo.cinemeta"), "the same AddonFilter the search fan-out reads");
        assert!(matches!(&jobs[..], [AddonsJob::WriteFilter(f)] if !f.allows("com.linvo.cinemeta")));
        let mut restored = AddonsStore::new();
        restored.land(AddonsLanding::FilterRead(s.filter().clone()));
        assert!(!restored.searched("com.linvo.cinemeta"));
    }

    /// The Streams page's per-addon switch (a long-press on its chip) is its own, per addon id,
    /// persisted on the device (never the account), and apart from search's.
    #[test]
    fn the_stream_switch_is_per_addon_persisted_and_apart_from_search() {
        let mut s = AddonsStore::new();
        assert!(s.boot().iter().any(|j| matches!(j, AddonsJob::ReadStreamFilter)), "read at boot");
        let jobs = s.toggle_streams("org.example.torrent");
        assert!(!s.stream_filter().allows("org.example.torrent") && s.stream_filter().allows("org.example.other"));
        assert!(s.searched("org.example.torrent"), "search is not affected");
        assert!(matches!(&jobs[..], [AddonsJob::WriteStreamFilter(f)] if !f.allows("org.example.torrent")));
        let mut restored = AddonsStore::new();
        restored.land(AddonsLanding::StreamFilterRead(s.stream_filter().clone()));
        assert!(!restored.stream_filter().allows("org.example.torrent"), "survives a restart");
        let jobs = restored.toggle_streams("org.example.torrent");
        assert!(restored.stream_filter().allows("org.example.torrent") && matches!(&jobs[..], [AddonsJob::WriteStreamFilter(_)]));
    }
}
