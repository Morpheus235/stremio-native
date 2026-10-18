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
    /// Switched off for stream queries on this device (the Streams page's long-press on its chip):
    /// not asked.
    Off,
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

    /// *A Wild Last Boss Appeared!* season 2, every id scheme it reaches the page with, against
    /// the stream declarations of the user's AIOStreams and MediaFusion (URLs are placeholders).
    /// Each request carries the EPISODE id, the page's type and `:` encoded as `%3A`, as
    /// stremio-core's `ResourcePath` URL does; an addon whose prefixes miss the id is not asked.
    #[test]
    fn a_second_season_episode_asks_each_serving_addon_for_that_episode() {
        let a = |url: &str, json: &str| Addon::new(url, Manifest::parse(json.as_bytes()).unwrap());
        let addons = vec![
            a("https://aio.example/u/manifest.json", r#"{"id":"aiostreams","name":"AIOStreams","resources":[{"name":"stream","types":["movie","series","anime","tv","events"],"idPrefixes":["tt","imdb","mal","tvdb","tmdb","kitsu","tmdb:","tvdb:","mal:","mf","dl"]}],"types":["movie","series","anime","tv","events"]}"#),
            a("https://mf.example/u/manifest.json", r#"{"id":"mediafusion","name":"MediaFusion","resources":[{"name":"stream","types":["movie","series","tv","events"],"idPrefixes":["tt","tmdb:","tvdb:","mal:","mf","dl"]}],"types":["movie","series","tv","events"]}"#),
        ];
        let urls = |kind: &str, id: &str| plan(&addons, kind, id).into_iter().map(|g| g.url).collect::<Vec<_>>();
        // Cinemeta's numbering
        assert_eq!(
            urls("series", "tt33258199:2:1"),
            ["https://aio.example/u/stream/series/tt33258199%3A2%3A1.json", "https://mf.example/u/stream/series/tt33258199%3A2%3A1.json"]
        );
        // the TMDB addon's (one TMDB season of 24 under the IMDb id): what it lists is what is asked
        assert_eq!(
            urls("series", "tt33258199:1:13"),
            ["https://aio.example/u/stream/series/tt33258199%3A1%3A13.json", "https://mf.example/u/stream/series/tt33258199%3A1%3A13.json"]
        );
        // Kitsu's second-season entry: MediaFusion declares no `kitsu` prefix
        assert_eq!(urls("series", "kitsu:50404:1"), ["https://aio.example/u/stream/series/kitsu%3A50404%3A1.json"]);
        assert_eq!(urls("anime", "kitsu:50404:1"), ["https://aio.example/u/stream/anime/kitsu%3A50404%3A1.json"], "nor the anime type");
    }

    #[test]
    fn live_tv_ids_reach_only_the_live_addon() {
        let groups = plan(&addons(), "tv", "live:42");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].addon_id, "live");
    }
}
