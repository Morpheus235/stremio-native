//! **The detail store** — what a Detail page reads: an item's meta (its description and, for a
//! series, its episodes) and, for one video, the streams every addon offers, kept apart per addon.
//!
//! Pure like the catalog store: it plans [`DetailJob`]s, the application runs them off the frame
//! thread, and [`DetailLanding`]s come back on the frame thread.
//!
//! - **Keyed and counted.** A page OPENS what it reads when it mounts and CLOSES it when it
//!   unmounts; an entry lives while any page holds it, so a Detail page under a Streams page keeps
//!   its meta, and nothing outlives the pages that read it.
//! - **Per-addon stream groups.** [`sn_stremio::streams::plan`] makes one group per addon that
//!   serves streams for the type and id (for a series, the EPISODE id); each group loads on its own
//!   job and keeps its own `Loading` / `Ready` / `Failed` state, so a slow addon is that addon's
//!   spinner and a failing one that addon's failure, never an empty list for everyone.

use std::collections::HashMap;

use sn_stremio::manifest::Addon;
use sn_stremio::search::AddonFilter;
use sn_stremio::streams::{plan, GroupState, StreamGroup};
use sn_stremio::types::Meta;
use sn_stremio::FetchError;

/// An item, as a meta request names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MetaKey {
    pub kind: String,
    pub id: String,
}

/// One video, as a stream request names it: a movie's own id, a series episode's id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StreamKey {
    pub kind: String,
    pub video_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MetaState {
    Loading,
    Ready(Box<Meta>),
    Failed(FetchError),
    /// No installed addon serves `meta` for this type and id.
    NoSource,
}

#[derive(Clone, Debug)]
pub enum DetailJob {
    /// Every serving addon's meta URL in addon order; the first that answers is the meta
    /// ([`sn_stremio::meta::fetch_meta`]).
    Meta { key: MetaKey, urls: Vec<String> },
    /// Load `group` (a planned, `Loading` group) for `key`; `index` is its position in the plan.
    Streams { key: StreamKey, index: usize, group: StreamGroup },
}

#[derive(Clone, Debug)]
pub enum DetailLanding {
    Meta { key: MetaKey, result: Result<Meta, FetchError> },
    Streams { key: StreamKey, index: usize, state: GroupState },
}

struct Held<T> {
    value: T,
    holders: u32,
}

#[derive(Default)]
pub struct DetailStore {
    metas: HashMap<MetaKey, Held<MetaState>>,
    streams: HashMap<StreamKey, Held<Vec<StreamGroup>>>,
    gen: u32,
}

impl DetailStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    fn bump(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    pub fn meta(&self, key: &MetaKey) -> Option<&MetaState> {
        self.metas.get(key).map(|h| &h.value)
    }

    pub fn streams(&self, key: &StreamKey) -> Option<&[StreamGroup]> {
        self.streams.get(key).map(|h| h.value.as_slice())
    }

    /// A page wants `key`'s meta. The first holder plans the request against `addons`.
    pub fn open_meta(&mut self, addons: &[Addon], key: MetaKey) -> Vec<DetailJob> {
        if let Some(h) = self.metas.get_mut(&key) {
            h.holders += 1;
            return Vec::new();
        }
        let urls = sn_stremio::meta::meta_urls(addons, &key.kind, &key.id);
        let (state, jobs) = if urls.is_empty() {
            (MetaState::NoSource, Vec::new())
        } else {
            (MetaState::Loading, vec![DetailJob::Meta { key: key.clone(), urls }])
        };
        self.metas.insert(key, Held { value: state, holders: 1 });
        self.bump();
        jobs
    }

    pub fn close_meta(&mut self, key: &MetaKey) {
        if let Some(h) = self.metas.get_mut(key) {
            h.holders = h.holders.saturating_sub(1);
            if h.holders == 0 {
                self.metas.remove(key);
                self.bump();
            }
        }
    }

    /// A page wants the streams of `key`. The first holder plans one group per serving addon; an
    /// addon `off` switched off for stream queries is planned [`GroupState::Off`] and not asked.
    pub fn open_streams(&mut self, addons: &[Addon], off: &AddonFilter, key: StreamKey) -> Vec<DetailJob> {
        if let Some(h) = self.streams.get_mut(&key) {
            h.holders += 1;
            return Vec::new();
        }
        let mut groups = plan(addons, &key.kind, &key.video_id);
        for g in groups.iter_mut().filter(|g| !off.allows(&g.addon_id)) {
            g.state = GroupState::Off;
        }
        let jobs = groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.state == GroupState::Loading)
            .map(|(index, group)| DetailJob::Streams { key: key.clone(), index, group: group.clone() })
            .collect();
        self.streams.insert(key, Held { value: groups, holders: 1 });
        self.bump();
        jobs
    }

    pub fn close_streams(&mut self, key: &StreamKey) {
        if let Some(h) = self.streams.get_mut(key) {
            h.holders = h.holders.saturating_sub(1);
            if h.holders == 0 {
                self.streams.remove(key);
                self.bump();
            }
        }
    }

    /// Addon `addon_id` was switched off (`on` false) or on again for stream queries: every held
    /// list drops its answer, or asks it now.
    pub fn set_addon(&mut self, addon_id: &str, on: bool) -> Vec<DetailJob> {
        let mut jobs = Vec::new();
        for (key, held) in &mut self.streams {
            for (index, g) in held.value.iter_mut().enumerate().filter(|(_, g)| g.addon_id == addon_id) {
                match (on, g.state == GroupState::Off) {
                    (false, false) => g.state = GroupState::Off,
                    (true, true) => {
                        g.state = GroupState::Loading;
                        jobs.push(DetailJob::Streams { key: key.clone(), index, group: g.clone() });
                    }
                    _ => {}
                }
            }
        }
        self.bump();
        jobs
    }

    /// Fold a job's outcome in. `false` for an entry no page holds any more (closed while out).
    pub fn land(&mut self, landing: DetailLanding) -> bool {
        match landing {
            DetailLanding::Meta { key, result } => {
                let Some(h) = self.metas.get_mut(&key) else { return false };
                h.value = match result {
                    Ok(meta) => MetaState::Ready(Box::new(meta)),
                    Err(e) => MetaState::Failed(e),
                };
            }
            DetailLanding::Streams { key, index, state } => {
                let Some(group) = self.streams.get_mut(&key).and_then(|h| h.value.get_mut(index)) else {
                    return false;
                };
                // an answer out when the addon was switched off is dropped
                if group.state == GroupState::Off {
                    return false;
                }
                group.state = state;
            }
        }
        self.bump();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_stremio::manifest::Manifest;
    use sn_stremio::types::{Stream, StreamHints};

    fn addon(url: &str, json: &str) -> Addon {
        Addon::new(url, Manifest::parse(json.as_bytes()).unwrap())
    }

    fn addons() -> Vec<Addon> {
        vec![
            addon("https://v3-cinemeta.strem.io/manifest.json",
                r#"{"id":"com.linvo.cinemeta","name":"Cinemeta","resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"]}"#),
            addon("https://one.example/manifest.json",
                r#"{"id":"one","name":"One","resources":["stream"],"types":["movie","series"],"idPrefixes":["tt"]}"#),
            addon("https://two.example/cfg/manifest.json",
                r#"{"id":"two","name":"","resources":[{"name":"stream","types":["series"]}],"types":["movie","series"]}"#),
        ]
    }

    fn http(url: &str) -> Stream {
        Stream { url: Some(url.into()), behavior_hints: StreamHints::default(), ..Default::default() }
    }

    #[test]
    fn meta_comes_from_the_first_addon_that_serves_it() {
        let mut s = DetailStore::new();
        let key = MetaKey { kind: "series".into(), id: "tt0903747".into() };
        let jobs = s.open_meta(&addons(), key.clone());
        assert!(matches!(&jobs[..], [DetailJob::Meta { urls, .. }]
            if urls == &["https://v3-cinemeta.strem.io/meta/series/tt0903747.json"]));
        assert_eq!(s.meta(&key), Some(&MetaState::Loading));
        let none = MetaKey { kind: "tv".into(), id: "live:1".into() };
        assert!(s.open_meta(&addons(), none.clone()).is_empty());
        assert_eq!(s.meta(&none), Some(&MetaState::NoSource));
    }

    /// The real MAL-Stremio manifest declares `meta` for `tt` ids too: placed above Cinemeta, the
    /// request still reaches Cinemeta (stremio-core asks every serving addon, first answer wins).
    #[test]
    fn meta_is_asked_of_every_serving_addon_in_order() {
        let mal = addon("https://mal.example/u1/manifest.json", include_str!("../../../stremio/src/fixtures/mal_stremio_manifest.json"));
        let mut list = vec![mal];
        list.extend(addons());
        let mut s = DetailStore::new();
        let key = MetaKey { kind: "series".into(), id: "tt0903747".into() };
        let jobs = s.open_meta(&list, key);
        assert!(matches!(&jobs[..], [DetailJob::Meta { urls, .. }]
            if urls == &["https://mal.example/u1/meta/series/tt0903747.json", "https://v3-cinemeta.strem.io/meta/series/tt0903747.json"]));
    }

    #[test]
    fn each_addon_keeps_its_own_stream_state() {
        let mut s = DetailStore::new();
        let key = StreamKey { kind: "series".into(), video_id: "tt0903747:1:2".into() };
        let jobs = s.open_streams(&addons(), &Default::default(), key.clone());
        let urls: Vec<String> = jobs
            .iter()
            .map(|j| match j {
                DetailJob::Streams { group, .. } => group.url.clone(),
                DetailJob::Meta { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(urls, [
            "https://one.example/stream/series/tt0903747%3A1%3A2.json",
            "https://two.example/cfg/stream/series/tt0903747%3A1%3A2.json",
        ], "the episode id, encoded, and the addon's configuration path kept");
        let groups = s.streams(&key).unwrap();
        assert_eq!(groups[1].addon_name, "two", "a nameless addon reads as its id");
        assert!(groups.iter().all(|g| g.state == GroupState::Loading));
        // the second addon fails first; the first is still loading
        assert!(s.land(DetailLanding::Streams { key: key.clone(), index: 1, state: GroupState::Failed(FetchError::Status(500)) }));
        assert_eq!(s.streams(&key).unwrap()[0].state, GroupState::Loading);
        assert_eq!(s.streams(&key).unwrap()[1].state, GroupState::Failed(FetchError::Status(500)));
        assert!(s.land(DetailLanding::Streams { key: key.clone(), index: 0, state: GroupState::Ready(vec![http("https://cdn.example/a.mkv")]) }));
        assert!(matches!(&s.streams(&key).unwrap()[0].state, GroupState::Ready(v) if v.len() == 1));
    }

    #[test]
    fn an_entry_lives_while_any_page_holds_it() {
        let mut s = DetailStore::new();
        let key = StreamKey { kind: "movie".into(), video_id: "tt1".into() };
        assert_eq!(s.open_streams(&addons(), &Default::default(), key.clone()).len(), 1, "only One serves movies");
        assert!(s.open_streams(&addons(), &Default::default(), key.clone()).is_empty(), "a second holder asks nothing");
        s.close_streams(&key);
        assert!(s.streams(&key).is_some());
        s.close_streams(&key);
        assert!(s.streams(&key).is_none());
        let gen = s.gen();
        assert!(!s.land(DetailLanding::Streams { key, index: 0, state: GroupState::Ready(Vec::new()) }), "a late landing for a closed entry is dropped");
        assert_eq!(s.gen(), gen);
    }

    /// An addon switched off for stream queries is planned Off and not asked; its answer out when
    /// it was switched off is dropped.
    #[test]
    fn an_addon_switched_off_is_not_asked_and_its_late_answer_is_dropped() {
        let mut s = DetailStore::new();
        let key = StreamKey { kind: "series".into(), video_id: "tt0903747:1:1".into() };
        let mut off = AddonFilter::default();
        off.toggle("two");
        let jobs = s.open_streams(&addons(), &off, key.clone());
        assert_eq!(jobs.len(), 1, "only One is asked");
        assert_eq!(s.streams(&key).unwrap()[1].state, GroupState::Off);
        assert!(s.set_addon("one", false).is_empty());
        assert!(!s.land(DetailLanding::Streams { key: key.clone(), index: 0, state: GroupState::Ready(Vec::new()) }), "One's late answer is dropped");
        assert_eq!(s.set_addon("two", true).len(), 1, "switched on, it is asked");
    }
}
