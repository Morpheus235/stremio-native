//! Asking every addon that serves streams for one video, and keeping each addon's answer apart.
//!
//! The request goes to `stream/{type}/{video id}` — for a series that is the EPISODE id
//! (`tt0903747:2:5`), never the series id, which no addon answers. Each addon that declares
//! `stream` for the type and id prefix gets its own [`StreamGroup`], so a slow or failing addon
//! shows as that addon's state instead of an empty list for everyone.

use crate::manifest::Addon;
use crate::types::{Stream, StreamsResponse};
use crate::url::resource_url;
use crate::{get_json, Fetch, FetchError};

#[derive(Clone, Debug, PartialEq)]
pub enum GroupState {
    Loading,
    Ready(Vec<Stream>),
    Failed(FetchError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct StreamGroup {
    pub addon_id: String,
    pub addon_name: String,
    pub url: String,
    pub state: GroupState,
}

/// One group per addon serving streams for `kind`/`video_id`, in addon order, all `Loading`.
pub fn plan(addons: &[Addon], kind: &str, video_id: &str) -> Vec<StreamGroup> {
    addons
        .iter()
        .filter(|a| a.manifest.serves("stream", kind, video_id))
        .map(|a| StreamGroup {
            addon_id: a.manifest.id.clone(),
            addon_name: if a.manifest.name.is_empty() { a.manifest.id.clone() } else { a.manifest.name.clone() },
            url: resource_url(&a.base, "stream", kind, video_id, &[]),
            state: GroupState::Loading,
        })
        .collect()
}

impl StreamGroup {
    pub fn load(&mut self, fetch: &dyn Fetch) {
        self.state = match get_json::<StreamsResponse>(fetch, &self.url, &[]) {
            Ok(r) => GroupState::Ready(r.streams.into_iter().filter(|s| s.playable().is_some()).collect()),
            Err(e) => GroupState::Failed(e),
        };
    }
}

/// Is every group settled, and did all of them come back with nothing? That is the one case a
/// screen may say "no streams found".
pub fn none_found(groups: &[StreamGroup]) -> bool {
    !groups.is_empty() && groups.iter().all(|g| matches!(&g.state, GroupState::Ready(s) if s.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFetch;
    use crate::library::LibraryItem;
    use crate::manifest::Manifest;

    fn addons() -> Vec<Addon> {
        let a = |url: &str, json: &str| Addon::new(url, Manifest::parse(json.as_bytes()).unwrap());
        vec![
            a("https://cinemeta.example/manifest.json", r#"{"id":"cinemeta","resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"]}"#),
            a("https://streams.example/cfg=k/manifest.json", r#"{"id":"s1","name":"Streams One","resources":["stream"],"types":["movie","series"],"idPrefixes":["tt"]}"#),
            a("https://live.example/manifest.json", r#"{"id":"live","name":"Live","resources":[{"name":"stream","types":["tv"],"idPrefixes":["live:"]}],"types":["tv"]}"#),
        ]
    }

    #[test]
    fn a_resumed_series_asks_streams_for_its_episode() {
        let item: LibraryItem = serde_json::from_str(
            r#"{"_id":"tt0903747","type":"series","state":{"timeOffset":5,"video_id":"tt0903747:2:5"}}"#,
        )
        .unwrap();
        let target = item.resume_target().unwrap();
        let groups = plan(&addons(), &target.kind, &target.video_id);
        let urls: Vec<_> = groups.iter().map(|g| g.url.as_str()).collect();
        assert_eq!(urls, ["https://streams.example/cfg=k/stream/series/tt0903747%3A2%3A5.json"]);
    }

    #[test]
    fn each_addon_keeps_its_own_outcome() {
        let mut groups = plan(&addons(), "movie", "tt0111161");
        let fetch = FakeFetch::default().with(
            "https://streams.example/cfg=k/stream/movie/tt0111161.json",
            r#"{"streams":[{"url":"https://cdn.example/a.mkv","name":"1080p"},{"name":"no source"}]}"#,
        );
        for g in &mut groups {
            g.load(&fetch);
        }
        let GroupState::Ready(streams) = &groups[0].state else { panic!("{:?}", groups[0].state) };
        assert_eq!(streams.len(), 1, "an unplayable entry is dropped");
        assert!(!none_found(&groups));
        let fetch = FakeFetch::default().failing("https://streams.example/cfg=k/stream/movie/tt0111161.json", FetchError::Status(500));
        groups[0].load(&fetch);
        assert_eq!(groups[0].state, GroupState::Failed(FetchError::Status(500)));
        assert!(!none_found(&groups), "a failure is not 'no streams'");
    }

    #[test]
    fn live_tv_ids_reach_only_the_live_addon() {
        let groups = plan(&addons(), "tv", "live:42");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].addon_id, "live");
    }
}
