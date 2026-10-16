//! Response bodies. Fields addons are known to omit are optional, and anything unknown is ignored,
//! so one sloppy item never discards the page it came in.

use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

/// `GET /catalog/...` → `{"metas":[...]}`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct CatalogPage {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub metas: Vec<MetaPreview>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetaPreview {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub poster: Option<String>,
    #[serde(default)]
    pub poster_shape: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub release_info: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub imdb_rating: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
}

/// `GET /meta/...` → `{"meta":{...}}`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MetaResponse {
    pub meta: Meta,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    #[serde(flatten)]
    pub preview: MetaPreview,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub videos: Vec<Video>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub runtime: Option<String>,
    #[serde(default)]
    pub cast: Vec<String>,
    #[serde(default)]
    pub director: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Video {
    pub id: String,
    #[serde(default, alias = "name")]
    pub title: String,
    #[serde(default)]
    pub season: Option<u32>,
    #[serde(default, alias = "number")]
    pub episode: Option<u32>,
    #[serde(default)]
    pub released: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
}

/// `GET /stream/...` → `{"streams":[...]}`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct StreamsResponse {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub streams: Vec<Stream>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub yt_id: Option<String>,
    #[serde(default)]
    pub info_hash: Option<String>,
    #[serde(default)]
    pub file_idx: Option<u32>,
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub behavior_hints: StreamHints,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StreamHints {
    /// The URL cannot be fetched by a browser as-is (needs headers, or is not CORS-clean). A
    /// native player plays it directly; this only matters to web clients.
    #[serde(default)]
    pub not_web_ready: bool,
    #[serde(default)]
    pub binge_group: Option<String>,
    #[serde(default)]
    pub proxy_headers: Option<ProxyHeaders>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub video_size: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ProxyHeaders {
    #[serde(default)]
    pub request: BTreeMap<String, String>,
    #[serde(default)]
    pub response: BTreeMap<String, String>,
}

/// What the player can do with a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Playable {
    /// An HTTP(S) URL the player fetches itself, with these request headers on every request
    /// (manifest, segments and redirects alike) — live-TV addons refuse requests without them.
    Http { url: String, headers: Vec<(String, String)> },
    /// A torrent; needs a streaming server.
    Torrent { info_hash: String, file_idx: Option<u32> },
    /// A YouTube id.
    YouTube(String),
    /// A link to open elsewhere, not playable here.
    External(String),
}

impl Stream {
    pub fn playable(&self) -> Option<Playable> {
        if let Some(url) = self.url.as_deref().filter(|u| !u.is_empty()) {
            let headers = self
                .behavior_hints
                .proxy_headers
                .as_ref()
                .map(|h| h.request.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            return Some(Playable::Http { url: url.to_string(), headers });
        }
        if let Some(hash) = self.info_hash.as_deref().filter(|h| !h.is_empty()) {
            return Some(Playable::Torrent { info_hash: hash.to_lowercase(), file_idx: self.file_idx });
        }
        if let Some(id) = self.yt_id.as_deref().filter(|i| !i.is_empty()) {
            return Some(Playable::YouTube(id.to_string()));
        }
        self.external_url.clone().map(Playable::External)
    }

    /// The label a stream list shows: the addon's `name` line and its `title`/`description`
    /// detail (addons use either for the multi-line quality text).
    pub fn label(&self) -> (String, String) {
        let name = self.name.clone().unwrap_or_default();
        let detail = self.title.clone().or_else(|| self.description.clone()).unwrap_or_default();
        (name, detail)
    }
}

/// `GET /subtitles/...` → `{"subtitles":[...]}`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SubtitlesResponse {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub subtitles: Vec<Subtitle>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Subtitle {
    #[serde(default)]
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub lang: String,
}

/// A list where an item that fails to decode is skipped instead of failing the whole list.
fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw: Option<Vec<serde_json::Value>> = Option::deserialize(d)?;
    Ok(raw
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

/// A string some addons send as a number (`"imdbRating": 7.4`, `"releaseInfo": 2019`).
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::String(s)) => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bad_meta_does_not_lose_the_page() {
        let page: CatalogPage = serde_json::from_str(
            r#"{"metas":[{"id":"tt1","type":"movie","name":"A","imdbRating":7.4,"releaseInfo":2019},
                         {"type":"movie","name":"no id"},
                         {"id":"tt2","type":"movie","name":"B"}]}"#,
        )
        .unwrap();
        let ids: Vec<_> = page.metas.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["tt1", "tt2"]);
        assert_eq!(page.metas[0].imdb_rating.as_deref(), Some("7.4"));
        assert_eq!(page.metas[0].release_info.as_deref(), Some("2019"));
    }

    #[test]
    fn a_missing_or_null_list_is_empty() {
        let page: CatalogPage = serde_json::from_str(r#"{"metas":null}"#).unwrap();
        assert!(page.metas.is_empty());
        let page: StreamsResponse = serde_json::from_str("{}").unwrap();
        assert!(page.streams.is_empty());
    }

    #[test]
    fn a_live_tv_stream_carries_its_request_headers() {
        let r: StreamsResponse = serde_json::from_str(
            r#"{"streams":[{"name":"Live","title":"Channel 1",
                "url":"https://live.example/play/1/index.m3u8",
                "behaviorHints":{"notWebReady":true,
                  "proxyHeaders":{"request":{"User-Agent":"LiveClient/2.6","Referer":"https://live.example/"}}}}]}"#,
        )
        .unwrap();
        let s = &r.streams[0];
        assert!(s.behavior_hints.not_web_ready);
        assert_eq!(
            s.playable(),
            Some(Playable::Http {
                url: "https://live.example/play/1/index.m3u8".into(),
                headers: vec![
                    ("Referer".into(), "https://live.example/".into()),
                    ("User-Agent".into(), "LiveClient/2.6".into())
                ],
            })
        );
        assert_eq!(s.label(), ("Live".into(), "Channel 1".into()));
    }

    #[test]
    fn stream_sources_in_priority_order() {
        let s: Stream = serde_json::from_str(r#"{"infoHash":"ABCDEF","fileIdx":2}"#).unwrap();
        assert_eq!(s.playable(), Some(Playable::Torrent { info_hash: "abcdef".into(), file_idx: Some(2) }));
        let s: Stream = serde_json::from_str(r#"{"ytId":"dQw4w9WgXcQ"}"#).unwrap();
        assert_eq!(s.playable(), Some(Playable::YouTube("dQw4w9WgXcQ".into())));
        let s: Stream = serde_json::from_str(r#"{"url":""}"#).unwrap();
        assert_eq!(s.playable(), None);
    }

    #[test]
    fn series_meta_reads_videos_with_either_title_key() {
        let m: MetaResponse = serde_json::from_str(
            r#"{"meta":{"id":"tt0903747","type":"series","name":"Breaking Bad",
               "videos":[{"id":"tt0903747:1:1","name":"Pilot","season":1,"episode":1},
                         {"id":"tt0903747:1:2","title":"Cat's in the Bag...","season":1,"number":2}]}}"#,
        )
        .unwrap();
        assert_eq!(m.meta.preview.name, "Breaking Bad");
        assert_eq!(m.meta.videos[0].title, "Pilot");
        assert_eq!(m.meta.videos[1].episode, Some(2));
    }
}
