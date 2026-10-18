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
    /// Cinemeta sends `"director": null` on a series: null reads as none, not a failed meta.
    #[serde(default, deserialize_with = "lenient_vec")]
    pub cast: Vec<String>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub director: Vec<String>,
    /// stremio-core's `links`: genres, cast, directors as deep links (`crate::links`). A link
    /// that does not decode is dropped alone.
    #[serde(default, deserialize_with = "lenient_vec")]
    pub links: Vec<crate::links::Link>,
    /// `null` or a malformed object reads as no hints, never as a failed meta.
    #[serde(default, deserialize_with = "lenient_hints")]
    pub behavior_hints: MetaHints,
    /// The TMDB addon (and AIOStreams serving it) names the work's IMDb id here.
    #[serde(default, rename = "imdb_id", deserialize_with = "lenient_string")]
    pub imdb_id: Option<String>,
}

/// The meta's `behaviorHints`. Only `defaultVideoId` is read: the video id streams are asked
/// for when the item has no video list (a movie). AIOStreams' and the TMDB addon's catalogs list
/// a movie as `tmdb:157336` and name its IMDb id here; their stream scrapers answer only for that.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MetaHints {
    #[serde(deserialize_with = "lenient_string")]
    pub default_video_id: Option<String>,
}

impl Meta {
    /// `behaviorHints.defaultVideoId` when the addon names one (blank reads as none).
    pub fn default_video_id(&self) -> Option<&str> {
        self.behavior_hints.default_video_id.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

    /// The work's IMDb id (`tt…`): its own id, `imdb_id`, an `imdb` link or the default video.
    pub fn imdb(&self) -> Option<&str> {
        let tt = |s: &str| s.len() > 2 && s.starts_with("tt") && s[2..].bytes().all(|b| b.is_ascii_digit());
        let link = self.links.iter().filter(|l| l.category.eq_ignore_ascii_case("imdb")).find_map(|l| l.url.split('/').find(|p| tt(p)));
        [Some(self.preview.id.as_str()), self.imdb_id.as_deref(), link, self.default_video_id()].into_iter().flatten().find(|s| tt(s))
    }
}

/// One video of a meta. Addons spell the title `title` or `name` and the episode `episode` or the
/// legacy `number`, and Cinemeta (and the TMDB addon) send BOTH `episode` and `number` on every
/// video. A serde `alias` rejects an object carrying both spellings as a duplicate field, which
/// dropped every Cinemeta episode, so both spellings are read as separate fields and merged.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(from = "VideoWire")]
pub struct Video {
    pub id: String,
    pub title: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub released: Option<String>,
    pub thumbnail: Option<String>,
    pub overview: Option<String>,
}

#[derive(Deserialize)]
struct VideoWire {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    season: Option<u32>,
    #[serde(default)]
    episode: Option<u32>,
    #[serde(default)]
    number: Option<u32>,
    #[serde(default)]
    released: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    /// Some addons spell the episode's overview `description` (Cinemeta sends both).
    #[serde(default)]
    description: Option<String>,
}

impl From<VideoWire> for Video {
    fn from(w: VideoWire) -> Video {
        Video {
            id: w.id,
            title: w.title.filter(|t| !t.is_empty()).or(w.name).unwrap_or_default(),
            season: w.season,
            episode: w.episode.or(w.number),
            released: w.released,
            thumbnail: w.thumbnail,
            overview: w.overview.filter(|o| !o.trim().is_empty()).or(w.description),
        }
    }
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
    /// AIOStreams' own annotation: `type` "error" marks an upstream's failure sent as a stream,
    /// "statistic" its scrape summary and "info" a message (`stream_facts::statistic_note`).
    #[serde(default)]
    pub stream_data: Option<StreamData>,
}

/// AIOStreams' `streamData` (only the part this app reads).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct StreamData {
    /// "error", "statistic", "p2p", "http", …
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
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
    /// The OpenSubtitles hash of the file, which a subtitles addon matches exact releases by.
    #[serde(default)]
    pub video_hash: Option<String>,
    /// A live stream (a TV channel): no length, no resume point.
    #[serde(default)]
    pub is_live: bool,
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

#[derive(Clone, Default, Deserialize, PartialEq)]
pub struct Subtitle {
    #[serde(default)]
    pub id: String,
    /// Never logged: an addon may put its configuration in it.
    pub url: String,
    #[serde(default)]
    pub lang: String,
    /// The addon's own name for the entry (the newer `label`). Server text.
    #[serde(default, deserialize_with = "lenient_string")]
    pub label: Option<String>,
    /// The release the timing was made for (OpenSubtitles v3's `movieReleaseName`). Server text.
    #[serde(default, rename = "movieReleaseName", deserialize_with = "lenient_string")]
    pub release_name: Option<String>,
    /// The subtitle file's name (OpenSubtitles v3's `subtitleFileName`). Server text.
    #[serde(default, rename = "subtitleFileName", deserialize_with = "lenient_string")]
    pub file_name: Option<String>,
}

impl std::fmt::Debug for Subtitle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subtitle").field("id", &self.id).field("lang", &self.lang).finish()
    }
}

impl Subtitle {
    /// The line a menu names the entry by: the addon's label, else the release, else the file
    /// name; `None` when the addon says none of them.
    pub fn release(&self) -> Option<&str> {
        [&self.label, &self.release_name, &self.file_name].into_iter().flatten().map(|s| s.trim()).find(|s| !s.is_empty())
    }

    /// The words of the entry's names, lower-cased (`Show.S01E01.SDH.forced.srt` → `show`, …).
    fn words(&self) -> impl Iterator<Item = String> + '_ {
        [&self.label, &self.file_name]
            .into_iter()
            .flatten()
            .flat_map(|s| s.split(|c: char| !c.is_alphanumeric()))
            .map(str::to_lowercase)
    }

    /// Only the foreign dialogue (the name says `forced`). ponytail: read from the names, the
    /// protocol has no flag; a field wins once addons send one.
    pub fn forced(&self) -> bool {
        self.words().any(|w| w == "forced")
    }

    /// For the deaf and hard of hearing (the name says `SDH`, `HI` or `CC`). Same ceiling as
    /// [`Self::forced`].
    pub fn hearing_impaired(&self) -> bool {
        self.words().any(|w| matches!(w.as_str(), "sdh" | "hi" | "cc"))
    }
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
fn lenient_hints<'de, D: Deserializer<'de>>(d: D) -> Result<MetaHints, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
}

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
    fn a_meta_names_its_default_video_and_null_hints_are_none() {
        let m: MetaResponse = serde_json::from_str(r#"{"meta":{"id":"tmdb:157336","type":"movie","name":"I","behaviorHints":{"defaultVideoId":"tt0816692","hasScheduledVideos":false}}}"#).unwrap();
        assert_eq!(m.meta.default_video_id(), Some("tt0816692"));
        let n: MetaResponse = serde_json::from_str(r#"{"meta":{"id":"tt1","type":"series","name":"S","behaviorHints":null}}"#).unwrap();
        assert_eq!(n.meta.default_video_id(), None);
        let blank: MetaResponse = serde_json::from_str(r#"{"meta":{"id":"tt1","type":"movie","name":"S","behaviorHints":{"defaultVideoId":" "}}}"#).unwrap();
        assert_eq!(blank.meta.default_video_id(), None);
    }

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

    /// A video's still is `thumbnail`, and its overview `overview` or, from addons that spell it
    /// so, `description`.
    #[test]
    fn a_video_reads_its_still_and_either_overview_key() {
        let m: MetaResponse = serde_json::from_str(
            r#"{"meta":{"id":"x","type":"series","videos":[
                {"id":"x:1:1","season":1,"episode":1,"thumbnail":"https://img.example/1.jpg","overview":"One"},
                {"id":"x:1:2","season":1,"episode":2,"description":"Two"},
                {"id":"x:1:3","season":1,"episode":3,"overview":"","description":"Three"},
                {"id":"x:1:4","season":1,"episode":4}]}}"#,
        )
        .unwrap();
        let got: Vec<_> = m.meta.videos.iter().map(|v| (v.thumbnail.as_deref(), v.overview.as_deref())).collect();
        assert_eq!(got, [(Some("https://img.example/1.jpg"), Some("One")), (None, Some("Two")), (None, Some("Three")), (None, None)]);
    }

    /// Real Cinemeta output (trimmed): every video carries BOTH `episode` and the legacy `number`.
    #[test]
    fn cinemeta_videos_keep_their_season_and_episode() {
        let m: MetaResponse = serde_json::from_str(include_str!("fixtures/cinemeta_breaking_bad.json")).unwrap();
        let got: Vec<_> = m.meta.videos.iter().map(|v| (v.id.as_str(), v.season, v.episode)).collect();
        assert_eq!(
            got,
            [
                ("tt0903747:1:1", Some(1), Some(1)),
                ("tt0903747:1:2", Some(1), Some(2)),
                ("tt0903747:2:1", Some(2), Some(1)),
                ("tt0903747:0:1", Some(0), Some(1)),
                ("tt0903747:5:16", Some(5), Some(16)),
            ]
        );
        assert_eq!(m.meta.videos[0].title, "Pilot");
    }

    /// Real Cinemeta metas (trimmed): a series with `"director": null` and a movie, read with
    /// the facts, people and art a Detail page shows.
    #[test]
    fn cinemeta_metas_read_their_facts_people_and_art() {
        let s: MetaResponse = serde_json::from_str(include_str!("fixtures/cinemeta_breaking_bad_meta.json")).unwrap();
        let s = s.meta;
        assert_eq!((s.runtime.as_deref(), s.preview.imdb_rating.as_deref()), (Some("49 min"), Some("9.5")));
        assert_eq!(s.cast, ["Bryan Cranston", "Aaron Paul", "Anna Gunn"]);
        assert!(s.director.is_empty(), "null is none");
        assert_eq!(s.preview.genres, ["Crime", "Drama", "Thriller"]);
        assert!(s.preview.background.as_deref().is_some_and(|b| b.starts_with("https://images.metahub.space/background/")));
        assert!(s.preview.description.as_deref().is_some_and(|d| d.starts_with("A chemistry teacher")));
        let v = s.videos.iter().find(|v| v.id == "tt0903747:1:5").unwrap();
        assert_eq!((v.title.as_str(), v.season, v.episode), ("Gray Matter", Some(1), Some(5)));
        assert!(v.overview.as_deref().is_some_and(|o| !o.is_empty()) && v.thumbnail.is_some());
        let m: MetaResponse = serde_json::from_str(include_str!("fixtures/cinemeta_shawshank.json")).unwrap();
        let m = m.meta;
        assert_eq!(m.director, ["Frank Darabont"]);
        assert_eq!((m.preview.release_info.as_deref(), m.runtime.as_deref()), (Some("1994"), Some("142 min")));
        assert!(m.videos.is_empty());
    }

    #[test]
    fn a_tmdb_meta_names_its_imdb_id() {
        let m = |j: &str| serde_json::from_str::<MetaResponse>(j).unwrap().meta;
        assert_eq!(m(r#"{"meta":{"id":"tt1","type":"series","name":"A"}}"#).imdb(), Some("tt1"));
        assert_eq!(m(r#"{"meta":{"id":"tmdb:1","type":"series","name":"A","imdb_id":"tt2"}}"#).imdb(), Some("tt2"));
        let link = r#"{"meta":{"id":"tmdb:1","type":"series","name":"A","links":[{"name":"8.9","category":"imdb","url":"https://imdb.com/title/tt3/"}]}}"#;
        assert_eq!(m(link).imdb(), Some("tt3"));
        assert_eq!(m(r#"{"meta":{"id":"tmdb:1","type":"series","name":"A","imdb_id":null}}"#).imdb(), None);
    }

    /// Real Kitsu addon output (trimmed): `title`, `season`, `episode`, plus imdb/absolute numbers.
    #[test]
    fn kitsu_videos_keep_their_season_and_episode() {
        let m: MetaResponse = serde_json::from_str(include_str!("fixtures/kitsu_aot_s2.json")).unwrap();
        let got: Vec<_> = m.meta.videos.iter().map(|v| (v.id.as_str(), v.season, v.episode)).collect();
        assert_eq!(got, [("kitsu:8671:1", Some(1), Some(1)), ("kitsu:8671:2", Some(1), Some(2)), ("kitsu:8671:12", Some(1), Some(12))]);
        assert!(!m.meta.videos[0].title.is_empty());
    }
}
