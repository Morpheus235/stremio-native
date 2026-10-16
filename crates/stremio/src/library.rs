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
//!
//! **Adding an item** ([`add_to_library`]) is stremio-core's `AddToLibrary` over the wire: the
//! entry the account already holds (a removed or temporary one keeps its play state and its
//! creation time) or a fresh one shaped like core's `LibraryItem::from(&MetaItemPreview)`, with
//! `removed` and `temp` cleared and `_mtime` now, written back with `datastorePut`
//! (`{"authKey", "collection": "libraryItem", "changes": [item]}`, answered by core's
//! `SuccessResponse`).

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
    /// When the entry last changed (RFC 3339).
    #[serde(rename = "_mtime", default, deserialize_with = "blank_is_none")]
    pub mtime: Option<String>,
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
    /// When it was last played (an RFC 3339 time, which orders as text), if ever.
    #[serde(rename = "lastWatched", default, deserialize_with = "blank_is_none")]
    pub last_watched: Option<String>,
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
    /// Shown in Continue Watching (stremio-core's `is_in_continue_watching`): in progress, and
    /// kept or temporary. A title watched without being added to the library is a temporary
    /// entry, flagged removed too, and it is most of what a viewer continues.
    pub fn is_continue_watching(&self) -> bool {
        self.kind != "other" && (!self.removed || self.temp) && self.state.time_offset > 0
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

    /// What orders Continue Watching, most recent first: the last play, else the last change.
    pub fn recency(&self) -> &str {
        self.state.last_watched.as_deref().or(self.mtime.as_deref()).unwrap_or("")
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
        .filter(|i| !i.removed || i.temp)
        .collect())
}

/// What an item menu knows of the item it adds: the catalog's preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEntry {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub poster: Option<String>,
}

/// The entry `datastorePut` is given for `entry`: `existing` (the account's raw entry, every field
/// it holds kept) with `removed`/`temp` cleared and `_mtime` set to `now`, or a new entry in core's
/// shape. `now` is an RFC 3339 time ([`rfc3339_ms`]).
pub fn added_entry(existing: Option<serde_json::Value>, entry: &NewEntry, now: &str) -> serde_json::Value {
    match existing {
        Some(serde_json::Value::Object(mut item)) => {
            item.insert("removed".into(), false.into());
            item.insert("temp".into(), false.into());
            item.insert("_mtime".into(), now.into());
            serde_json::Value::Object(item)
        }
        _ => json!({
            "_id": entry.id,
            "name": entry.name,
            "type": entry.kind,
            "poster": entry.poster,
            "posterShape": "poster",
            "removed": false,
            "temp": false,
            "_ctime": now,
            "_mtime": now,
            "state": {
                "lastWatched": now,
                "timeWatched": 0,
                "timeOffset": 0,
                "overallTimeWatched": 0,
                "timesWatched": 0,
                "flaggedWatched": 0,
                "duration": 0,
                "video_id": null,
                "watched": null,
                "noNotif": false
            },
            "behaviorHints": {}
        }),
    }
}

/// Add `entry` to the account's library (see the module docs). `now` is an RFC 3339 time.
pub fn add_to_library(fetch: &dyn Fetch, auth_key: &str, entry: &NewEntry, now: &str) -> Result<(), ApiError> {
    let headers = [("Content-Type".to_string(), "application/json".to_string())];
    let raw = fetch.post(
        &format!("{}/api/datastoreGet", crate::account::API),
        &headers,
        json!({"authKey": auth_key, "collection": "libraryItem", "ids": [entry.id], "all": false}).to_string().as_bytes(),
    )?;
    let existing: Vec<serde_json::Value> = crate::account::parse(&raw)?;
    let existing = existing.into_iter().find(|v| v.get("_id").and_then(|i| i.as_str()) == Some(entry.id.as_str()));
    let item = added_entry(existing, entry, now);
    let raw = fetch.post(
        &format!("{}/api/datastorePut", crate::account::API),
        &headers,
        json!({"authKey": auth_key, "collection": "libraryItem", "changes": [item]}).to_string().as_bytes(),
    )?;
    let _: serde_json::Value = crate::account::parse(&raw)?;
    Ok(())
}

/// `unix_ms` as RFC 3339 in UTC with milliseconds (`2026-10-16T09:05:00.250Z`), the form core's
/// `DateTime<Utc>` reads.
pub fn rfc3339_ms(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        unix_ms % 1000
    )
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

    /// Stremio keeps a title watched without adding it to the library as a TEMPORARY entry,
    /// which is also flagged removed; it is still continued (stremio-core's
    /// `is_in_continue_watching`: type not "other", not removed or temporary, a position).
    #[test]
    fn a_temporary_entry_in_progress_is_continued_and_a_removed_one_is_not() {
        assert!(item(r#"{"_id":"a","type":"movie","removed":true,"temp":true,"state":{"timeOffset":5}}"#).is_continue_watching());
        assert!(item(r#"{"_id":"a","type":"series","temp":true,"state":{"timeOffset":5}}"#).is_continue_watching());
        assert!(item(r#"{"_id":"a","type":"movie","state":{"timeOffset":5}}"#).is_continue_watching());
        assert!(!item(r#"{"_id":"a","type":"movie","removed":true,"state":{"timeOffset":5}}"#).is_continue_watching());
        assert!(!item(r#"{"_id":"a","type":"movie","state":{"timeOffset":0}}"#).is_continue_watching());
        assert!(!item(r#"{"_id":"a","type":"other","state":{"timeOffset":5}}"#).is_continue_watching());
    }

    /// The fetch keeps temporary entries: they ARE most of Continue Watching.
    #[test]
    fn the_fetch_keeps_temporary_entries_and_drops_removed_ones() {
        let fetch = crate::fake::FakeFetch::default().with(
            "https://api.strem.io/api/datastoreGet",
            r#"{"result":[
                {"_id":"keep","type":"movie","removed":false,"temp":false,"state":{"timeOffset":0}},
                {"_id":"temp","type":"series","removed":true,"temp":true,"state":{"timeOffset":7,"video_id":"temp:1:2"}},
                {"_id":"gone","type":"movie","removed":true,"temp":false,"state":{"timeOffset":9}}]}"#,
        );
        let ids: Vec<String> = fetch_library(&fetch, "k").unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["keep", "temp"]);
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

    #[test]
    fn the_clock_text_is_rfc3339_utc() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_ms(951_782_400_000), "2000-02-29T00:00:00.000Z", "a leap day");
        assert_eq!(rfc3339_ms(1_792_141_500_250), "2026-10-16T09:05:00.250Z");
    }

    fn entry() -> NewEntry {
        NewEntry { id: "tt1".into(), kind: "movie".into(), name: "Film".into(), poster: Some("https://p/x.jpg".into()) }
    }

    #[test]
    fn a_new_entry_has_core_s_shape_and_reads_back_as_a_library_item() {
        let v = added_entry(None, &entry(), "2026-10-16T09:05:00.250Z");
        let i: LibraryItem = serde_json::from_value(v.clone()).unwrap();
        assert_eq!((i.id.as_str(), i.kind.as_str(), i.removed, i.temp), ("tt1", "movie", false, false));
        assert_eq!(i.poster.as_deref(), Some("https://p/x.jpg"));
        assert_eq!(v["_ctime"], "2026-10-16T09:05:00.250Z");
        assert_eq!(v["state"]["timeOffset"], 0);
        assert!(!i.is_continue_watching());
    }

    #[test]
    fn re_adding_a_removed_entry_keeps_its_play_state_and_fields() {
        let old = serde_json::json!({"_id":"tt1","type":"movie","name":"Film","removed":true,"temp":true,
            "_ctime":"2020-01-01T00:00:00Z","_mtime":"2020-01-02T00:00:00Z","state":{"timeOffset":5000,"duration":9000},"x":1});
        let v = added_entry(Some(old), &entry(), "2026-10-16T09:05:00.250Z");
        assert_eq!(v["removed"], false);
        assert_eq!(v["temp"], false);
        assert_eq!(v["_mtime"], "2026-10-16T09:05:00.250Z");
        assert_eq!(v["_ctime"], "2020-01-01T00:00:00Z");
        assert_eq!(v["state"]["timeOffset"], 5000);
        assert_eq!(v["x"], 1, "a field this client does not know is written back as it was");
    }

    /// Answers each API path from a table and records every body.
    struct Api {
        answers: std::collections::HashMap<&'static str, &'static str>,
        bodies: std::cell::RefCell<Vec<(String, serde_json::Value)>>,
    }
    impl Fetch for Api {
        fn get(&self, _url: &str, _h: &[(String, String)]) -> Result<Vec<u8>, crate::FetchError> {
            Err(crate::FetchError::Status(405))
        }
        fn post(&self, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<Vec<u8>, crate::FetchError> {
            let path = url.rsplit('/').next().unwrap_or("").to_string();
            self.bodies.borrow_mut().push((path.clone(), serde_json::from_slice(body).unwrap()));
            self.answers.get(path.as_str()).map(|a| a.as_bytes().to_vec()).ok_or(crate::FetchError::Status(404))
        }
    }

    #[test]
    fn adding_reads_the_entry_then_puts_it_with_the_key() {
        let api = Api {
            answers: [("datastoreGet", r#"{"result":[]}"#), ("datastorePut", r#"{"result":{"success":true}}"#)].into(),
            bodies: Default::default(),
        };
        add_to_library(&api, "KEY", &entry(), "2026-10-16T09:05:00.250Z").unwrap();
        let bodies = api.bodies.borrow();
        assert_eq!(bodies[0].0, "datastoreGet");
        assert_eq!(bodies[0].1["ids"], serde_json::json!(["tt1"]));
        assert_eq!(bodies[1].0, "datastorePut");
        assert_eq!(bodies[1].1["authKey"], "KEY");
        assert_eq!(bodies[1].1["collection"], "libraryItem");
        assert_eq!(bodies[1].1["changes"][0]["_id"], "tt1");
        let refused = Api {
            answers: [("datastoreGet", r#"{"result":[]}"#), ("datastorePut", r#"{"error":{"code":1,"message":"Session does not exist"}}"#)].into(),
            bodies: Default::default(),
        };
        assert!(add_to_library(&refused, "KEY", &entry(), "t").is_err(), "an API refusal is a failure");
    }
}
