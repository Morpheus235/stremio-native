//! The user's library (the `libraryItem` datastore collection) and what "resume" opens.
//!
//! A library item is keyed by the WORK (`tt0903747` for a series), but streams are served per
//! VIDEO: an addon answers `stream/series/tt0903747:1:5.json` and has nothing for the bare series
//! id. So a resumable series must resolve to the episode the user stopped on before any stream
//! is asked for. [`LibraryItem::resume_target`] is that resolution, in this order:
//!
//! 1. `state.video_id`, the video last played;
//! 2. `behaviorHints.defaultVideoId`, the video the addon says the item opens on;
//! 3. for a movie (or any item whose videos are the item itself), the item id.
//!
//! A series with none of those has no target until its meta is loaded and the first episode is
//! chosen ([`first_episode`]).

use crate::account::ApiError;
use crate::types::Video;
use crate::Fetch;
use serde::{Deserialize, Deserializer};
use serde_json::json;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub poster: Option<String>,
    #[serde(default)]
    pub removed: bool,
    #[serde(default)]
    pub temp: bool,
    #[serde(default)]
    pub state: PlayState,
    #[serde(default)]
    pub behavior_hints: ItemHints,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct PlayState {
    /// Milliseconds into the video last played.
    #[serde(rename = "timeOffset", default)]
    pub time_offset: u64,
    /// That video's duration in milliseconds, 0 when unknown.
    #[serde(default)]
    pub duration: u64,
    #[serde(default, deserialize_with = "blank_is_none")]
    pub video_id: Option<String>,
    #[serde(rename = "timesWatched", default)]
    pub times_watched: u32,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemHints {
    #[serde(default, deserialize_with = "blank_is_none")]
    pub default_video_id: Option<String>,
}

/// The video a resume opens, and where in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeTarget {
    /// The item's type (`series`, `movie`, `tv`...), which the stream request uses as-is.
    pub kind: String,
    /// The VIDEO id streams are asked for.
    pub video_id: String,
    /// Where to start, in milliseconds; 0 to start from the beginning.
    pub offset_ms: u64,
}

impl LibraryItem {
    /// Shown in Continue Watching: in progress, not removed, not a temporary entry.
    pub fn is_continue_watching(&self) -> bool {
        !self.removed && !self.temp && self.state.time_offset > 0
    }

    /// The video a resume opens. `None` for a series with no recorded or default video: the
    /// caller loads its meta and uses [`first_episode`].
    pub fn resume_target(&self) -> Option<ResumeTarget> {
        let video_id = self
            .state
            .video_id
            .clone()
            .or_else(|| self.behavior_hints.default_video_id.clone())
            .or_else(|| (self.kind != "series").then(|| self.id.clone()))?;
        // The offset belongs to `state.video_id`; it says nothing about a different video.
        let offset_ms = if self.state.video_id.as_deref() == Some(video_id.as_str()) || self.state.video_id.is_none() && self.kind != "series" {
            self.state.time_offset
        } else {
            0
        };
        Some(ResumeTarget { kind: self.kind.clone(), video_id, offset_ms })
    }

    /// Progress through the last-played video, 0.0–1.0, for a progress bar.
    pub fn progress(&self) -> f32 {
        if self.state.duration == 0 {
            return 0.0;
        }
        (self.state.time_offset as f32 / self.state.duration as f32).clamp(0.0, 1.0)
    }
}

/// The episode a series with no play state opens on: the first episode of the first season
/// that is not specials (season 0), else the first video listed.
pub fn first_episode(videos: &[Video]) -> Option<&Video> {
    videos
        .iter()
        .filter(|v| v.season.unwrap_or(1) > 0)
        .min_by_key(|v| (v.season.unwrap_or(1), v.episode.unwrap_or(0)))
        .or_else(|| videos.first())
}

/// The whole library from the account datastore, live items only.
pub fn fetch_library(fetch: &dyn Fetch, auth_key: &str) -> Result<Vec<LibraryItem>, ApiError> {
    let raw = fetch.post(
        &format!("{}/api/datastoreGet", crate::account::API),
        &[("Content-Type".into(), "application/json".into())],
        json!({"type": "DatastoreGet", "authKey": auth_key, "collection": "libraryItem", "ids": [], "all": true})
            .to_string()
            .as_bytes(),
    )?;
    let items: Vec<serde_json::Value> = crate::account::parse(&raw)?;
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<LibraryItem>(v).ok())
        .filter(|i| !i.removed)
        .collect())
}

fn blank_is_none<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.filter(|s| !s.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: &str) -> LibraryItem {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_resumable_series_opens_the_episode_it_stopped_on_not_the_series() {
        let i = item(
            r#"{"_id":"tt0903747","name":"Breaking Bad","type":"series",
                "state":{"timeOffset":1260000,"duration":2820000,"video_id":"tt0903747:2:5","timesWatched":0}}"#,
        );
        assert!(i.is_continue_watching());
        assert_eq!(
            i.resume_target(),
            Some(ResumeTarget { kind: "series".into(), video_id: "tt0903747:2:5".into(), offset_ms: 1260000 })
        );
    }

    #[test]
    fn a_series_without_play_state_uses_the_default_video_or_waits_for_its_meta() {
        let i = item(r#"{"_id":"tt1","type":"series","state":{"video_id":""},"behaviorHints":{"defaultVideoId":"tt1:1:1"}}"#);
        assert_eq!(i.resume_target().unwrap().video_id, "tt1:1:1");
        assert_eq!(i.resume_target().unwrap().offset_ms, 0);
        let i = item(r#"{"_id":"tt1","type":"series","state":{"video_id":null}}"#);
        assert_eq!(i.resume_target(), None, "never ask streams for the bare series id");
    }

    #[test]
    fn a_movie_resumes_on_itself() {
        let i = item(r#"{"_id":"tt0111161","type":"movie","state":{"timeOffset":600000,"duration":8520000}}"#);
        assert_eq!(
            i.resume_target(),
            Some(ResumeTarget { kind: "movie".into(), video_id: "tt0111161".into(), offset_ms: 600000 })
        );
        assert!((i.progress() - 0.0704).abs() < 0.001);
    }

    #[test]
    fn removed_and_temporary_items_are_not_continue_watching() {
        assert!(!item(r#"{"_id":"a","type":"movie","removed":true,"state":{"timeOffset":5}}"#).is_continue_watching());
        assert!(!item(r#"{"_id":"a","type":"movie","temp":true,"state":{"timeOffset":5}}"#).is_continue_watching());
        assert!(!item(r#"{"_id":"a","type":"movie","state":{"timeOffset":0}}"#).is_continue_watching());
    }

    #[test]
    fn the_first_episode_skips_specials() {
        let videos: Vec<Video> = serde_json::from_str(
            r#"[{"id":"s:0:1","season":0,"episode":1},{"id":"s:2:1","season":2,"episode":1},
                {"id":"s:1:2","season":1,"episode":2},{"id":"s:1:1","season":1,"episode":1}]"#,
        )
        .unwrap();
        assert_eq!(first_episode(&videos).unwrap().id, "s:1:1");
        let only_specials: Vec<Video> = serde_json::from_str(r#"[{"id":"s:0:1","season":0,"episode":1}]"#).unwrap();
        assert_eq!(first_episode(&only_specials).unwrap().id, "s:0:1");
    }
}
