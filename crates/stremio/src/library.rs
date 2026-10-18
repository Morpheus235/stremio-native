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
//!
//! **Removing an item** and **dismissing it from Continue Watching** ([`edit_library`]) are
//! stremio-core's `RemoveFromLibrary` (`removed` set, `temp` cleared, so it leaves the Library and
//! Continue Watching both) and `RewindLibraryItem` (`state.timeOffset` back to 0, which is what
//! drops it from Continue Watching; the entry stays in the library), the same read-then-
//! `datastorePut` with `_mtime` now ([`edited_entry`]). An entry the account does not hold has
//! nothing to change and nothing is written.
//!
//! **Recording where the viewer is** ([`put_progress`]) is stremio-core's player `TimeChanged`
//! over the wire, the same read-then-`datastorePut`: the entry's `state` takes the video, the
//! offset, the duration and `lastWatched`; the time actually watched accumulates (a seek is not
//! watching); past [`WATCHED_THRESHOLD`] of the video it is flagged watched once
//! (`flaggedWatched`, `timesWatched` + 1, and the video's `state.watched` bit when the meta's
//! videos came with the write). A title the library does not hold is written as core's
//! temporary entry (`temp` and `removed`), which is what Continue Watching lists. The last write
//! is core's `Unload`: stopped past [`CREDITS_THRESHOLD`] the offset goes back to 0 and an
//! episode with a released next one moves on to it ([`PlayState::apply_progress`]).
//!
//! **Marking watched / unwatched** ([`mark_library`]) is stremio-core's `MarkAsWatched` (a movie,
//! a card: [`PlayState::mark_as_watched`]) and `MarkVideoAsWatched` / `MarkSeasonAsWatched` (an
//! episode, a season: [`PlayState::mark_videos`], which sets the `state.watched` bits of
//! `crate::watched` and moves the resume pointer past what was marked), the same read-then-
//! `datastorePut` rewriting only the `state` fields the mark changed. A title the account does not
//! hold is written as core's temporary entry. [`Sort`] is the Library page's order.

use crate::account::ApiError;
use crate::types::Video;
use crate::watched::{self, WatchedBitField};
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
    /// When it was created (RFC 3339): the Watched sorts' last tie-break.
    #[serde(rename = "_ctime", default, deserialize_with = "blank_is_none")]
    pub ctime: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct PlayState {
    /// Milliseconds into the video last played.
    #[serde(rename = "timeOffset", default, deserialize_with = "lenient_count")]
    pub time_offset: u64,
    /// That video's duration in milliseconds, 0 when unknown.
    #[serde(default, deserialize_with = "lenient_count")]
    pub duration: u64,
    #[serde(default, deserialize_with = "blank_is_none")]
    pub video_id: Option<String>,
    #[serde(rename = "timesWatched", default, deserialize_with = "lenient_count")]
    pub times_watched: u32,
    /// When it was last played (an RFC 3339 time, which orders as text), if ever.
    #[serde(rename = "lastWatched", default, deserialize_with = "blank_is_none")]
    pub last_watched: Option<String>,
    /// Milliseconds watched of `video_id`, and of every video before it.
    #[serde(rename = "timeWatched", default, deserialize_with = "lenient_count")]
    pub time_watched: u64,
    #[serde(rename = "overallTimeWatched", default, deserialize_with = "lenient_count")]
    pub overall_time_watched: u64,
    /// `video_id` was watched through (0 or 1).
    #[serde(rename = "flaggedWatched", default, deserialize_with = "lenient_count")]
    pub flagged_watched: u32,
    /// Which videos were watched: core's watched bitfield string (`crate::watched`).
    #[serde(default, deserialize_with = "blank_is_none")]
    pub watched: Option<String>,
}

/// A counter as other clients store it: a number (Stremio writes `timeOffset` as a float), a
/// numeric string, or `null`/junk read as 0. Per field, so one odd value never resets the rest of
/// a title's state — the state is written back to the account.
fn lenient_count<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: TryFrom<u64> + Default,
{
    let v = Option::<serde_json::Value>::deserialize(d)?;
    let n = match v {
        Some(serde_json::Value::Number(n)) => n.as_u64().or_else(|| n.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f.round() as u64)),
        Some(serde_json::Value::String(s)) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f.round() as u64),
        _ => None,
    };
    Ok(n.and_then(|n| T::try_from(n).ok()).unwrap_or_default())
}

impl PlayState {
    /// core's `LibraryItem::mark_as_watched` (the movie and card *Mark as watched*): watched adds
    /// one to `timesWatched`, sets `lastWatched` to `now` and `timeOffset` to 0 (nothing left to
    /// continue); unwatched sets `timesWatched` to 0 and nothing else.
    pub fn mark_as_watched(&mut self, watched: bool, now: &str) {
        if watched {
            self.times_watched = self.times_watched.saturating_add(1);
            self.last_watched = Some(now.to_string());
            self.time_offset = 0;
        } else {
            self.times_watched = 0;
        }
    }

    /// The watched bits over `videos` (a meta's), core's `LibraryItemState::watched_bitfield`.
    pub fn watched_bitfield(&self, videos: &[Video]) -> WatchedBitField {
        WatchedBitField::of_state(self.watched.as_deref(), videos)
    }

    /// core's `MetaDetails` `MarkVideoAsWatched` / `MarkSeasonAsWatched` (see [`VideoMark`]).
    pub fn mark_videos(&mut self, mark: &VideoMark, now: &str) {
        let order = watched::core_order(&mark.videos);
        let marked: Vec<&Video> = match &mark.target {
            MarkTarget::Video(id) => order.iter().copied().filter(|v| v.id == *id).take(1).collect(),
            MarkTarget::Season(s) => order.iter().copied().filter(|v| watched::series_info(v).is_some_and(|i| i.0 == *s)).collect(),
        };
        if marked.is_empty() {
            return;
        }
        // `mark_videos_as_watched`
        let mut bits = self.watched_bitfield(&mark.videos);
        for v in &marked {
            bits.set_video(&v.id, mark.watched);
        }
        self.watched = Some(bits.to_string());
        if mark.watched {
            let released = marked.last().and_then(|v| v.released.as_deref());
            self.last_watched = match (self.last_watched.take(), released) {
                (Some(last), Some(r)) if last.as_str() < r => Some(r.to_string()),
                (None, r) => r.map(str::to_string),
                (last, _) => last,
            };
        }
        if !mark.watched {
            return;
        }
        match &mark.target {
            // the pointer moves past a video marked watched while it is the one to continue
            MarkTarget::Video(id) if self.video_id.as_deref() == Some(id.as_str()) => match watched::next_video(&order, id, now) {
                Some(next) => self.advance_to_video(&next.id),
                None => self.time_offset = 0,
            },
            MarkTarget::Video(_) => {}
            // `reconcile_series_resume_after_watched_change`
            MarkTarget::Season(season) => {
                let Some(mut current) = self.video_id.clone() else { return };
                let in_season = order.iter().any(|v| v.id == current && watched::series_info(v).is_some_and(|i| i.0 == *season));
                if !bits.get_video(&current) || !in_season {
                    return;
                }
                loop {
                    match watched::next_video(&order, &current, now) {
                        Some(next) if bits.get_video(&next.id) => current = next.id.clone(),
                        Some(next) => return self.advance_to_video(&next.id),
                        None => {
                            self.time_offset = 0;
                            return;
                        }
                    }
                }
            }
        }
    }

    /// core's `advance_to_video`: continue `video_id` from (almost) its start.
    fn advance_to_video(&mut self, video_id: &str) {
        self.video_id = Some(video_id.to_string());
        self.overall_time_watched = self.overall_time_watched.saturating_add(self.time_watched);
        self.time_watched = 0;
        self.flagged_watched = 0;
        self.time_offset = 1;
    }
}

/// What *Mark as watched / unwatched* on an episode or a season changes ([`PlayState::mark_videos`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoMark {
    /// The meta's videos (what the bits are laid over).
    pub videos: Vec<Video>,
    pub target: MarkTarget,
    pub watched: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkTarget {
    /// One video (core's `MarkVideoAsWatched`).
    Video(String),
    /// Every episode of a season (core's `MarkSeasonAsWatched`).
    Season(u32),
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
            .or_else(|| (!self.episodic()).then(|| self.id.clone()))?;
        // The offset belongs to `state.video_id`; it says nothing about a different video.
        let offset_ms = if self.state.video_id.as_deref() == Some(video_id.as_str()) || self.state.video_id.is_none() && !self.episodic() {
            self.state.time_offset
        } else {
            0
        };
        Some(ResumeTarget { kind: self.kind.clone(), video_id, offset_ms })
    }

    /// Where a play of `video_id` picks up: the saved offset when it was the last video played
    /// (a movie that never recorded one is its own id), else 0.
    pub fn saved_offset(&self, video_id: &str) -> u64 {
        match self.state.video_id.as_deref().filter(|v| !v.is_empty()) {
            Some(v) if v == video_id => self.state.time_offset,
            None if !self.episodic() && self.id == video_id => self.state.time_offset,
            _ => 0,
        }
    }

    /// Whether the item's videos are episodes, so its own id is never a playable video: series,
    /// and the anime type Kitsu/MAL addons use (an anime film's meta lists its one video).
    fn episodic(&self) -> bool {
        matches!(self.kind.as_str(), "series" | "anime")
    }

    /// What orders Continue Watching, most recent first: the last play, else the last change.
    pub fn recency(&self) -> &str {
        self.state.last_watched.as_deref().or(self.mtime.as_deref()).unwrap_or("")
    }

    /// Watched (core's `LibraryItem::watched`, a card's tick): played through, or marked, at
    /// least once.
    pub fn watched(&self) -> bool {
        self.state.times_watched > 0
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

/// The episode after `video_id` in watching order: by season (specials, season 0, after every
/// other season) then episode, then the order the meta lists them. `None` for the last episode,
/// for a video the list does not hold, or for a list without season/episode numbers.
pub fn next_episode<'a>(videos: &'a [Video], video_id: &str) -> Option<&'a Video> {
    if videos.iter().all(|v| v.season.is_none() && v.episode.is_none()) {
        return None;
    }
    let mut order: Vec<(usize, &Video)> = videos.iter().enumerate().collect();
    order.sort_by_key(|(i, v)| {
        let season = v.season.unwrap_or(0);
        (season == 0, season, v.episode.unwrap_or(u32::MAX), *i)
    });
    let at = order.iter().position(|(_, v)| v.id == video_id)?;
    order.get(at + 1).map(|(_, v)| *v)
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

/// A change to an entry ([`edit_library`], [`mark_library`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// stremio-core's `RemoveFromLibrary`: `removed`, not `temp`.
    Remove,
    /// stremio-core's `RewindLibraryItem`: `state.timeOffset` 0, so it leaves Continue Watching.
    Rewind,
    /// stremio-core's `LibraryItemMarkAsWatched` / `MetaItemMarkAsWatched` / `MetaDetails`
    /// `MarkAsWatched` ([`PlayState::mark_as_watched`]).
    MarkWatched(bool),
    /// stremio-core's `MarkVideoAsWatched` / `MarkSeasonAsWatched` ([`PlayState::mark_videos`]).
    MarkVideos(VideoMark),
}

impl Edit {
    /// Does the edit write an entry the account does not hold yet (core builds a temporary one
    /// from the meta for these)?
    pub fn creates(&self) -> bool {
        matches!(self, Edit::MarkWatched(true) | Edit::MarkVideos(_))
    }

    /// Apply a watched mark to `state`; `false` for an edit that is not one.
    pub fn apply_to(&self, state: &mut PlayState, now: &str) -> bool {
        match self {
            Edit::MarkWatched(w) => state.mark_as_watched(*w, now),
            Edit::MarkVideos(m) => state.mark_videos(m, now),
            Edit::Remove | Edit::Rewind => return false,
        }
        true
    }
}

/// `existing` (the account's raw entry, every field kept) with `edit` applied and `_mtime` `now`.
/// A watched mark rewrites only the `state` fields it changed.
pub fn edited_entry(mut existing: serde_json::Map<String, serde_json::Value>, edit: &Edit, now: &str) -> serde_json::Value {
    match edit {
        Edit::MarkWatched(_) | Edit::MarkVideos(_) => {
            let mut raw = match existing.remove("state") {
                Some(serde_json::Value::Object(s)) => s,
                _ => serde_json::Map::new(),
            };
            let before: PlayState = serde_json::from_value(serde_json::Value::Object(raw.clone())).unwrap_or_default();
            let mut after = before.clone();
            edit.apply_to(&mut after, now);
            let opt = |v: &Option<String>| v.clone().map_or(serde_json::Value::Null, serde_json::Value::from);
            let fields: [(&str, bool, serde_json::Value); 8] = [
                ("timesWatched", before.times_watched != after.times_watched, after.times_watched.into()),
                ("lastWatched", before.last_watched != after.last_watched, opt(&after.last_watched)),
                ("timeOffset", before.time_offset != after.time_offset, after.time_offset.into()),
                ("timeWatched", before.time_watched != after.time_watched, after.time_watched.into()),
                ("overallTimeWatched", before.overall_time_watched != after.overall_time_watched, after.overall_time_watched.into()),
                ("flaggedWatched", before.flagged_watched != after.flagged_watched, after.flagged_watched.into()),
                ("video_id", before.video_id != after.video_id, opt(&after.video_id)),
                ("watched", before.watched != after.watched, opt(&after.watched)),
            ];
            for (k, changed, v) in fields {
                if changed {
                    raw.insert(k.into(), v);
                }
            }
            existing.insert("state".into(), serde_json::Value::Object(raw));
        }
        Edit::Remove => {
            existing.insert("removed".into(), true.into());
            existing.insert("temp".into(), false.into());
        }
        Edit::Rewind => match existing.get_mut("state") {
            Some(serde_json::Value::Object(state)) => {
                state.insert("timeOffset".into(), 0.into());
            }
            _ => {
                existing.insert("state".into(), json!({"timeOffset": 0}));
            }
        },
    }
    existing.insert("_mtime".into(), now.into());
    serde_json::Value::Object(existing)
}

/// Apply `edit` to the account's entry `id` (see the module docs). `now` is an RFC 3339 time.
pub fn edit_library(fetch: &dyn Fetch, auth_key: &str, id: &str, edit: Edit, now: &str) -> Result<(), ApiError> {
    write_edit(fetch, auth_key, id, None, &edit, now)
}

/// Apply a watched mark (`edit`) to `entry`'s library item; an item the account does not hold is
/// first created as core's temporary entry (`removed`, `temp`, `lastWatched` now) when the mark
/// [`creates`](Edit::creates) one, and nothing is written otherwise.
pub fn mark_library(fetch: &dyn Fetch, auth_key: &str, entry: &NewEntry, edit: Edit, now: &str) -> Result<(), ApiError> {
    write_edit(fetch, auth_key, &entry.id, Some(entry).filter(|_| edit.creates()), &edit, now)
}

/// A temporary entry for `entry` in core's shape (`LibraryItem::from(&MetaItemPreview)`).
fn temp_entry(entry: &NewEntry, now: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut fresh = added_entry(None, entry, now);
    fresh["removed"] = true.into();
    fresh["temp"] = true.into();
    match fresh {
        serde_json::Value::Object(m) => m,
        _ => unreachable!("added_entry builds an object"),
    }
}

fn write_edit(fetch: &dyn Fetch, auth_key: &str, id: &str, create: Option<&NewEntry>, edit: &Edit, now: &str) -> Result<(), ApiError> {
    let headers = [("Content-Type".to_string(), "application/json".to_string())];
    let raw = fetch.post(
        &format!("{}/api/datastoreGet", crate::account::API),
        &headers,
        json!({"authKey": auth_key, "collection": "libraryItem", "ids": [id], "all": false}).to_string().as_bytes(),
    )?;
    let existing: Vec<serde_json::Value> = crate::account::parse(&raw)?;
    let existing = match existing.into_iter().find(|v| v.get("_id").and_then(|i| i.as_str()) == Some(id)) {
        Some(serde_json::Value::Object(m)) => m,
        _ => match create {
            Some(entry) => temp_entry(entry, now),
            None => return Ok(()),
        },
    };
    let item = edited_entry(existing, edit, now);
    let raw = fetch.post(
        &format!("{}/api/datastorePut", crate::account::API),
        &headers,
        json!({"authKey": auth_key, "collection": "libraryItem", "changes": [item]}).to_string().as_bytes(),
    )?;
    let _: serde_json::Value = crate::account::parse(&raw)?;
    Ok(())
}

/// Where a playback stands, for [`put_progress`]. Times in milliseconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// The library item: the WORK (a series' id for an episode).
    pub id: String,
    pub kind: String,
    /// The work's name and poster, for an entry the library does not hold yet.
    pub name: String,
    pub poster: Option<String>,
    /// The video playing (an episode's id; a movie's own id).
    pub video_id: String,
    pub time_offset: u64,
    pub duration: u64,
    /// How much of the video was watched since the previous write (a seek is not watching).
    pub watched: u64,
    /// The playback's last write: core's `Unload`, whose `item_state_update` moves a video
    /// stopped in its credits on ([`PlayState::apply_progress`]).
    pub unload: bool,
    /// The meta's videos, once it is in: what the watched bit is laid over (core's
    /// `Player::watched`, `None` until the meta loads, when no bit is written).
    pub videos: Option<Vec<Video>>,
}

/// The fraction of a video that counts it as watched (stremio-core's `WATCHED_THRESHOLD_COEF`).
pub const WATCHED_THRESHOLD: f64 = 0.7;
/// Stopped past this fraction, a video is done with (stremio-core's `CREDITS_THRESHOLD_COEF`).
pub const CREDITS_THRESHOLD: f64 = 0.9;

impl PlayState {
    /// One progress write, as stremio-core's player applies it: `TimeChanged`
    /// (`models/player.rs`, `ActionPlayer::TimeChanged`), then for the last write `Unload`'s
    /// `item_state_update`. `live` (core's `is_live`): no watched flag, the channel is the video.
    pub fn apply_progress(&mut self, p: &Progress, live: bool, item_id: &str, now: &str) {
        let video_id = if live { item_id } else { p.video_id.as_str() };
        self.last_watched = Some(now.to_string());
        if self.video_id.as_deref() != Some(video_id) {
            self.video_id = Some(video_id.to_string());
            self.overall_time_watched = self.overall_time_watched.saturating_add(self.time_watched);
            self.time_watched = 0;
            self.flagged_watched = 0;
        }
        // ponytail: one write carries every tick since the last one, so the new video's first is
        // counted too (core drops that single tick's delta); a seek is already excluded upstream.
        self.time_watched = self.time_watched.saturating_add(p.watched);
        self.overall_time_watched = self.overall_time_watched.saturating_add(p.watched);
        // the position as `Seek` and `TimeChanged` together leave it
        self.time_offset = p.time_offset;
        self.duration = p.duration;
        if !live && self.flagged_watched == 0 && self.duration > 0 && self.time_watched as f64 > self.duration as f64 * WATCHED_THRESHOLD {
            self.flagged_watched = 1;
            self.times_watched = self.times_watched.saturating_add(1);
            if let Some(videos) = &p.videos {
                let mut bits = self.watched_bitfield(videos);
                bits.set_video(video_id, true);
                self.watched = Some(bits.to_string());
            }
        }
        if !p.unload {
            return;
        }
        if live {
            self.time_offset = 0;
            self.video_id = Some(item_id.to_string());
        } else if self.time_offset as f64 > self.duration as f64 * CREDITS_THRESHOLD {
            self.time_offset = 0;
            let order = p.videos.as_deref().map(watched::core_order).unwrap_or_default();
            if let Some(next) = watched::next_video(&order, video_id, now) {
                self.advance_to_video(&next.id);
            }
        }
    }
}

impl LibraryItem {
    /// [`PlayState::apply_progress`] and core's `TimeChanged` entry rules: a temporary entry never
    /// watched through is a removed one, a removed one being played is temporary.
    pub fn apply_progress(&mut self, p: &Progress, now: &str) {
        self.state.apply_progress(p, self.kind == "tv", &self.id, now);
        if self.temp && self.state.times_watched == 0 {
            self.removed = true;
        }
        if self.removed {
            self.temp = true;
        }
        self.mtime = Some(now.to_string());
    }

    /// Core's temporary entry for a title the library does not hold, played (`p`).
    pub fn progressed_temp(p: &Progress, now: &str) -> LibraryItem {
        let mut item = LibraryItem {
            id: p.id.clone(),
            name: p.name.clone(),
            kind: p.kind.clone(),
            poster: p.poster.clone(),
            removed: true,
            temp: true,
            ctime: Some(now.to_string()),
            ..LibraryItem::default()
        };
        item.apply_progress(p, now);
        item
    }
}

/// The entry `datastorePut` is given for `p`: `existing` (the account's raw entry, every field
/// kept) with its play state moved on ([`LibraryItem::apply_progress`]), or a new temporary entry in
/// core's shape. `now` is RFC 3339.
pub fn progressed_entry(existing: Option<serde_json::Value>, p: &Progress, now: &str) -> serde_json::Value {
    let mut item = match existing {
        Some(serde_json::Value::Object(item)) => item,
        _ => temp_entry(&NewEntry { id: p.id.clone(), kind: p.kind.clone(), name: p.name.clone(), poster: p.poster.clone() }, now),
    };
    let mut state = match item.remove("state") {
        Some(serde_json::Value::Object(s)) => s,
        _ => serde_json::Map::new(),
    };
    let flag = |k: &str| item.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let mut typed = LibraryItem {
        id: p.id.clone(),
        kind: item.get("type").and_then(|v| v.as_str()).unwrap_or(&p.kind).to_string(),
        removed: flag("removed"),
        temp: flag("temp"),
        state: serde_json::from_value(serde_json::Value::Object(state.clone())).unwrap_or_default(),
        ..LibraryItem::default()
    };
    typed.apply_progress(p, now);
    let s = &typed.state;
    for (k, v) in [
        ("video_id", json!(s.video_id)),
        ("timeOffset", s.time_offset.into()),
        ("duration", s.duration.into()),
        ("timeWatched", s.time_watched.into()),
        ("overallTimeWatched", s.overall_time_watched.into()),
        ("flaggedWatched", s.flagged_watched.into()),
        ("timesWatched", s.times_watched.into()),
        ("lastWatched", json!(s.last_watched)),
    ] {
        state.insert(k.into(), v);
    }
    if let Some(w) = &s.watched {
        state.insert("watched".into(), w.clone().into());
    }
    item.insert("state".into(), serde_json::Value::Object(state));
    item.insert("removed".into(), typed.removed.into());
    item.insert("temp".into(), typed.temp.into());
    item.insert("_mtime".into(), now.into());
    serde_json::Value::Object(item)
}

/// Record `p` in the account's library (see the module docs). `now` is an RFC 3339 time.
pub fn put_progress(fetch: &dyn Fetch, auth_key: &str, p: &Progress, now: &str) -> Result<(), ApiError> {
    let headers = [("Content-Type".to_string(), "application/json".to_string())];
    let raw = fetch.post(
        &format!("{}/api/datastoreGet", crate::account::API),
        &headers,
        json!({"authKey": auth_key, "collection": "libraryItem", "ids": [p.id], "all": false}).to_string().as_bytes(),
    )?;
    let existing: Vec<serde_json::Value> = crate::account::parse(&raw)?;
    let existing = existing.into_iter().find(|v| v.get("_id").and_then(|i| i.as_str()) == Some(p.id.as_str()));
    let item = progressed_entry(existing, p, now);
    let raw = fetch.post(
        &format!("{}/api/datastorePut", crate::account::API),
        &headers,
        json!({"authKey": auth_key, "collection": "libraryItem", "changes": [item]}).to_string().as_bytes(),
    )?;
    let _: serde_json::Value = crate::account::parse(&raw)?;
    Ok(())
}

/// The Library page's orders: stremio-core's `library_with_filters::Sort`, in its order (the
/// first is the default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    LastWatched,
    Name,
    NameReverse,
    TimesWatched,
    Watched,
    NotWatched,
}

impl Sort {
    pub const ALL: [Sort; 6] = [Sort::LastWatched, Sort::Name, Sort::NameReverse, Sort::TimesWatched, Sort::Watched, Sort::NotWatched];

    /// core's (serde) name for it: `lastwatched`, `name`, `namereverse`, `timeswatched`,
    /// `watched`, `notwatched`.
    pub fn name(self) -> &'static str {
        match self {
            Sort::LastWatched => "lastwatched",
            Sort::Name => "name",
            Sort::NameReverse => "namereverse",
            Sort::TimesWatched => "timeswatched",
            Sort::Watched => "watched",
            Sort::NotWatched => "notwatched",
        }
    }

    /// The sort `name` names ([`Sort::name`]), the default for an unknown one.
    pub fn named(name: &str) -> Sort {
        Sort::ALL.into_iter().find(|s| s.name() == name).unwrap_or_default()
    }

    /// core's `Sort::sort_items`. Times compare as RFC 3339 text; a missing one sorts first.
    pub fn cmp(self, a: &LibraryItem, b: &LibraryItem) -> std::cmp::Ordering {
        let (la, lb) = (&a.state.last_watched, &b.state.last_watched);
        match self {
            Sort::LastWatched => lb.cmp(la),
            Sort::TimesWatched => b.state.times_watched.cmp(&a.state.times_watched),
            Sort::Watched => b.watched().cmp(&a.watched()).then(lb.cmp(la)).then(b.ctime.cmp(&a.ctime)),
            Sort::NotWatched => a.watched().cmp(&b.watched()).then(la.cmp(lb)).then(a.ctime.cmp(&b.ctime)),
            Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            Sort::NameReverse => b.name.to_lowercase().cmp(&a.name.to_lowercase()),
        }
    }
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

    #[test]
    fn odd_counters_from_other_clients_never_reset_the_rest_of_the_state() {
        let st: PlayState = serde_json::from_str(
            r#"{"timeOffset":1234.6,"duration":"5000","timesWatched":null,"timeWatched":12.0,"overallTimeWatched":-3,"flaggedWatched":1,"video_id":"tt1:1:2","watched":"x"}"#,
        )
        .unwrap();
        assert_eq!((st.time_offset, st.duration, st.times_watched, st.time_watched, st.overall_time_watched, st.flagged_watched), (1235, 5000, 0, 12, 0, 1));
        assert_eq!(st.video_id.as_deref(), Some("tt1:1:2"));
    }

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

    /// Picking a stream on a detail page started at 0 even with a saved position: only Continue
    /// Watching resumed. The offset belongs to the video it was saved for and no other.
    #[test]
    fn a_saved_offset_resumes_only_the_video_it_was_saved_for() {
        let ep = item(r#"{"_id":"tt1","type":"series","state":{"timeOffset":1260000,"video_id":"tt1:2:5"}}"#);
        assert_eq!(ep.saved_offset("tt1:2:5"), 1_260_000);
        assert_eq!(ep.saved_offset("tt1:2:6"), 0);
        assert_eq!(ep.saved_offset("tt1"), 0);
        let anime = item(r#"{"_id":"kitsu:8671","type":"anime","state":{"timeOffset":90000,"video_id":""}}"#);
        assert_eq!(anime.saved_offset("kitsu:8671"), 0, "an anime show's own id is not an episode");
        assert_eq!(anime.resume_target(), None, "wait for its meta, as a series does");
        let film = item(r#"{"_id":"tt9","type":"movie","state":{"timeOffset":600000,"video_id":""}}"#);
        assert_eq!(film.saved_offset("tt9"), 600_000);
        assert_eq!(film.saved_offset("tt8"), 0);
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
    fn the_next_episode_follows_watching_order_across_seasons() {
        let videos: Vec<Video> = serde_json::from_str(
            r#"[{"id":"s:2:1","season":2,"episode":1},{"id":"s:0:1","season":0,"episode":1},
                {"id":"s:1:2","season":1,"episode":2},{"id":"s:1:1","season":1,"episode":1}]"#,
        )
        .unwrap();
        assert_eq!(next_episode(&videos, "s:1:1").unwrap().id, "s:1:2");
        assert_eq!(next_episode(&videos, "s:1:2").unwrap().id, "s:2:1", "a season's last episode leads to the next season");
        assert_eq!(next_episode(&videos, "s:2:1").unwrap().id, "s:0:1", "specials come last");
        assert!(next_episode(&videos, "s:0:1").is_none(), "nothing after the last");
        assert!(next_episode(&videos, "other").is_none(), "a video the meta does not list");
        let unnumbered: Vec<Video> = serde_json::from_str(r#"[{"id":"a"},{"id":"b"}]"#).unwrap();
        assert!(next_episode(&unnumbered, "a").is_none(), "a channel's videos are not episodes");
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

    fn progress(video: &str, offset: u64, watched: u64) -> Progress {
        Progress {
            id: "tt2".into(),
            kind: "series".into(),
            name: "Show".into(),
            poster: Some("https://p/s.jpg".into()),
            video_id: video.into(),
            time_offset: offset,
            duration: 3_000_000,
            watched,
            unload: false,
            videos: None,
        }
    }

    /// A title the library does not hold is written as core's temporary entry, and it reads back
    /// as Continue Watching at the episode and offset just played.
    #[test]
    fn progress_on_an_unknown_title_is_a_temporary_entry_in_continue_watching() {
        let v = progressed_entry(None, &progress("tt2:1:3", 600_000, 30_000), "2026-10-17T10:00:00.000Z");
        assert_eq!((v["temp"].as_bool(), v["removed"].as_bool()), (Some(true), Some(true)));
        assert_eq!(v["poster"], "https://p/s.jpg");
        let i: LibraryItem = serde_json::from_value(v.clone()).unwrap();
        assert!(i.is_continue_watching());
        assert_eq!(
            i.resume_target(),
            Some(ResumeTarget { kind: "series".into(), video_id: "tt2:1:3".into(), offset_ms: 600_000 })
        );
        assert_eq!(v["state"]["lastWatched"], "2026-10-17T10:00:00.000Z");
        assert_eq!(v["state"]["timeWatched"], 30_000);
    }

    #[test]
    fn progress_keeps_the_entry_and_accumulates_what_was_watched() {
        let old = serde_json::json!({"_id":"tt2","type":"series","name":"Show","removed":false,"temp":false,"x":1,
            "state":{"video_id":"tt2:1:3","timeOffset":600000,"timeWatched":100000,"overallTimeWatched":500000,"timesWatched":2,"flaggedWatched":0}});
        let v = progressed_entry(Some(old.clone()), &progress("tt2:1:3", 630_000, 30_000), "t");
        assert_eq!(v["x"], 1, "a field this client does not know is written back");
        assert_eq!(v["removed"], false, "a kept entry stays kept");
        assert_eq!(v["state"]["timeOffset"], 630_000);
        assert_eq!(v["state"]["timeWatched"], 130_000);
        assert_eq!(v["state"]["overallTimeWatched"], 530_000);
        assert_eq!(v["_mtime"], "t");
        // another episode starts its own count
        let v = progressed_entry(Some(old), &progress("tt2:1:4", 10_000, 10_000), "t");
        assert_eq!(v["state"]["video_id"], "tt2:1:4");
        assert_eq!(v["state"]["timeWatched"], 10_000);
        assert_eq!(v["state"]["overallTimeWatched"], 610_000, "the old episode's time joins the total");
    }

    #[test]
    fn watched_through_is_flagged_once_and_a_stop_in_the_credits_leaves_continue_watching() {
        let old = serde_json::json!({"_id":"tt2","type":"series","state":{"video_id":"tt2:1:3","timeWatched":2_090_000,"timesWatched":0}});
        let v = progressed_entry(Some(old), &progress("tt2:1:3", 2_200_000, 20_000), "t");
        assert_eq!((v["state"]["flaggedWatched"].as_u64(), v["state"]["timesWatched"].as_u64()), (Some(1), Some(1)));
        let again = progressed_entry(Some(v), &progress("tt2:1:3", 2_230_000, 30_000), "t");
        assert_eq!(again["state"]["timesWatched"], 1, "flagged once per video");
        // stopped in the credits (> 90 %), no meta to name a next episode: done with
        let end = Progress { unload: true, ..progress("tt2:1:3", 2_990_000, 30_000) };
        let done = progressed_entry(Some(again.clone()), &end, "t");
        assert_eq!(done["state"]["timeOffset"], 0);
        let i: LibraryItem = serde_json::from_value(done).unwrap();
        assert!(!i.is_continue_watching());
        // stopped before the credits: continued where it stopped
        let mid = progressed_entry(Some(again), &Progress { unload: true, ..progress("tt2:1:3", 2_500_000, 0) }, "t");
        assert_eq!(mid["state"]["timeOffset"], 2_500_000);
    }

    fn show() -> Vec<Video> {
        (1..=4).map(|e| Video { id: format!("tt2:1:{e}"), season: Some(1), episode: Some(e), ..Default::default() }).collect()
    }

    fn with_videos(p: Progress) -> Progress {
        Progress { videos: Some(show()), ..p }
    }

    /// core's `TimeChanged`: the bit is set the write `timeWatched` passes 70 % of the duration,
    /// once (`flaggedWatched` guards it); the writes after it carry the same bits.
    #[test]
    fn crossing_the_threshold_sets_the_episode_s_bit_once() {
        let before = progressed_entry(None, &with_videos(progress("tt2:1:3", 2_000_000, 2_090_000)), "t");
        assert!(before["state"]["watched"].is_null(), "below 70 %: no bit");
        assert_eq!(before["state"]["flaggedWatched"], 0);
        let crossed = progressed_entry(Some(before), &with_videos(progress("tt2:1:3", 2_030_000, 20_000)), "t");
        let w = crossed["state"]["watched"].as_str().expect("the bit is written").to_string();
        let bits = WatchedBitField::of_state(Some(&w), &show());
        assert!(bits.get_video("tt2:1:3"));
        assert!(!bits.get_video("tt2:1:2") && !bits.get_video("tt2:1:4"), "only the video played");
        assert!(w.starts_with("tt2:1:3:3:"), "anchored on the last watched video, core's `{{id}}:{{len}}:{{bits}}`");
        assert_eq!((crossed["state"]["flaggedWatched"].as_u64(), crossed["state"]["timesWatched"].as_u64()), (Some(1), Some(1)));
        let after = progressed_entry(Some(crossed), &with_videos(progress("tt2:1:3", 2_060_000, 30_000)), "t");
        assert_eq!(after["state"]["watched"], w.as_str());
        assert_eq!(after["state"]["timesWatched"], 1);
    }

    /// core keeps the bits it set: a seek back moves the offset, never the flag or the bit.
    #[test]
    fn seeking_back_keeps_the_bit() {
        let crossed = progressed_entry(None, &with_videos(progress("tt2:1:3", 2_200_000, 2_200_000)), "t");
        let w = crossed["state"]["watched"].clone();
        let back = progressed_entry(Some(crossed), &with_videos(progress("tt2:1:3", 60_000, 0)), "t");
        assert_eq!(back["state"]["timeOffset"], 60_000);
        assert_eq!(back["state"]["watched"], w);
        assert_eq!((back["state"]["flaggedWatched"].as_u64(), back["state"]["timesWatched"].as_u64()), (Some(1), Some(1)));
    }

    /// `set_video` of an id the meta does not list changes no bit (core's `WatchedBitField`); the
    /// flag and the count still move.
    #[test]
    fn an_episode_the_meta_does_not_list_is_progress_without_a_bit() {
        let v = progressed_entry(None, &with_videos(progress("tt2:9:9", 2_200_000, 2_200_000)), "t");
        assert_eq!(v["state"]["timesWatched"], 1);
        let bits = WatchedBitField::of_state(v["state"]["watched"].as_str(), &show());
        assert!(show().iter().all(|e| !bits.get_video(&e.id)), "no episode is marked");
        // without the meta's videos (core: `Player::watched` is None) the field is left alone
        let none = progressed_entry(None, &progress("tt2:1:3", 2_200_000, 2_200_000), "t");
        assert!(none["state"]["watched"].is_null());
    }

    /// A movie counts one watch per playback: `timesWatched` + 1 when it crosses, not again on
    /// the writes after it.
    #[test]
    fn a_movie_past_the_threshold_counts_one_watch() {
        let film = |offset, watched| Progress {
            id: "tt1".into(),
            kind: "movie".into(),
            video_id: "tt1".into(),
            videos: Some(Vec::new()),
            ..progress("tt1", offset, watched)
        };
        let old = serde_json::json!({"_id":"tt1","type":"movie","removed":false,"temp":false,"state":{"video_id":"tt1","timesWatched":2,"flaggedWatched":0,"timeWatched":0}});
        let mut v = progressed_entry(Some(old), &film(1_000_000, 1_000_000), "t");
        for (o, w) in [(2_200_000, 1_200_000), (2_500_000, 300_000), (2_800_000, 300_000)] {
            v = progressed_entry(Some(v), &film(o, w), "t");
        }
        assert_eq!(v["state"]["timesWatched"], 3, "one more watch");
        assert_eq!(v["state"]["flaggedWatched"], 1);
        // core lays the bits over the meta's (no) videos: the empty field
        assert_eq!(v["state"]["watched"], "undefined:1:eJwDAAAAAAE=");
        let end = progressed_entry(Some(v), &Progress { unload: true, ..film(2_950_000, 150_000) }, "t");
        assert_eq!((end["state"]["timesWatched"].as_u64(), end["state"]["timeOffset"].as_u64()), (Some(3), Some(0)));
    }

    /// The JSON written is core's `LibraryItemState` shape (camelCase, `video_id`).
    #[test]
    fn the_written_state_has_core_s_field_shape() {
        let v = progressed_entry(None, &with_videos(progress("tt2:1:3", 2_200_000, 2_200_000)), "2026-10-17T10:00:00.000Z");
        let mut keys: Vec<&str> = v["state"].as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["duration", "flaggedWatched", "lastWatched", "noNotif", "overallTimeWatched", "timeOffset", "timeWatched", "timesWatched", "video_id", "watched"]
        );
        for k in ["duration", "flaggedWatched", "overallTimeWatched", "timeOffset", "timeWatched", "timesWatched"] {
            assert!(v["state"][k].is_u64(), "{k} is a number");
        }
        assert_eq!(v["state"]["lastWatched"], "2026-10-17T10:00:00.000Z");
        assert!(v["state"]["video_id"].is_string() && v["state"]["watched"].is_string());
        assert_eq!((v["removed"].as_bool(), v["temp"].as_bool()), (Some(true), Some(true)), "core's temporary entry stays one");
    }

    /// core's `Unload` in the credits with a released next episode: the resume moves on to it.
    #[test]
    fn a_stop_in_the_credits_moves_on_to_the_next_episode() {
        let crossed = progressed_entry(None, &with_videos(progress("tt2:1:3", 2_200_000, 2_200_000)), "t");
        let end = progressed_entry(Some(crossed), &Progress { unload: true, ..with_videos(progress("tt2:1:3", 2_950_000, 750_000)) }, "t");
        let s = &end["state"];
        assert_eq!((s["video_id"].as_str(), s["timeOffset"].as_u64()), (Some("tt2:1:4"), Some(1)));
        assert_eq!((s["timeWatched"].as_u64(), s["flaggedWatched"].as_u64(), s["overallTimeWatched"].as_u64()), (Some(0), Some(0), Some(5_900_000)));
        // ^ core's `advance_to_video` adds `timeWatched` to a total `TimeChanged` already counted it in
        assert!(WatchedBitField::of_state(s["watched"].as_str(), &show()).get_video("tt2:1:3"), "the bit stays");
    }

    /// The held entry (the store's optimistic copy) moves exactly as the written JSON does.
    #[test]
    fn the_typed_item_moves_as_the_written_entry() {
        let p = with_videos(progress("tt2:1:3", 2_200_000, 2_200_000));
        let held = LibraryItem::progressed_temp(&p, "t");
        let written: LibraryItem = serde_json::from_value(progressed_entry(None, &p, "t")).unwrap();
        assert_eq!((held.state, held.removed, held.temp), (written.state, written.removed, written.temp));
    }

    #[test]
    fn progress_reads_the_entry_then_puts_it() {
        let api = Api {
            answers: [
                ("datastoreGet", r#"{"result":[{"_id":"tt2","type":"series","name":"Show","state":{}}]}"#),
                ("datastorePut", r#"{"result":{"success":true}}"#),
            ]
            .into(),
            bodies: Default::default(),
        };
        put_progress(&api, "KEY", &progress("tt2:1:3", 5_000, 5_000), "now").unwrap();
        let bodies = api.bodies.borrow();
        assert_eq!(bodies[0].1["ids"], serde_json::json!(["tt2"]));
        assert_eq!(bodies[1].0, "datastorePut");
        assert_eq!(bodies[1].1["changes"][0]["state"]["timeOffset"], 5_000);
        assert_eq!(bodies[1].1["changes"][0]["state"]["video_id"], "tt2:1:3");
    }

    #[test]
    fn removing_and_rewinding_are_core_s_edits_and_keep_every_other_field() {
        let old = || match serde_json::json!({"_id":"tt1","type":"movie","name":"Film","removed":false,"temp":true,
            "state":{"timeOffset":5000,"duration":9000,"video_id":"tt1"},"x":1}) {
            serde_json::Value::Object(m) => m,
            _ => unreachable!(),
        };
        let removed = edited_entry(old(), &Edit::Remove, "now");
        assert_eq!((removed["removed"].clone(), removed["temp"].clone(), removed["_mtime"].clone()), (true.into(), false.into(), "now".into()));
        assert_eq!(removed["x"], 1);
        let i: LibraryItem = serde_json::from_value(removed).unwrap();
        assert!(!i.is_continue_watching(), "a removed entry leaves Continue Watching too");
        let rewound = edited_entry(old(), &Edit::Rewind, "now");
        assert_eq!(rewound["state"]["timeOffset"], 0);
        assert_eq!(rewound["state"]["duration"], 9000, "the rest of the play state is kept");
        assert_eq!(rewound["removed"], false);
        let i: LibraryItem = serde_json::from_value(rewound).unwrap();
        assert!(!i.is_continue_watching(), "a rewound entry leaves Continue Watching");
    }

    #[test]
    fn an_edit_reads_the_entry_then_puts_it_and_an_unknown_entry_writes_nothing() {
        let api = Api {
            answers: [
                ("datastoreGet", r#"{"result":[{"_id":"tt2","type":"series","name":"Show","state":{"timeOffset":7}}]}"#),
                ("datastorePut", r#"{"result":{"success":true}}"#),
            ]
            .into(),
            bodies: Default::default(),
        };
        edit_library(&api, "KEY", "tt2", Edit::Remove, "now").unwrap();
        {
            let bodies = api.bodies.borrow();
            assert_eq!(bodies[0].1["ids"], serde_json::json!(["tt2"]));
            assert_eq!(bodies[1].0, "datastorePut");
            assert_eq!(bodies[1].1["authKey"], "KEY");
            assert_eq!(bodies[1].1["changes"][0]["removed"], true);
        }
        api.bodies.borrow_mut().clear();
        edit_library(&api, "KEY", "tt9", Edit::Rewind, "now").unwrap();
        assert_eq!(api.bodies.borrow().len(), 1, "nothing to change: only the read");
    }

    // --- watched marks: stremio-core's own vectors (`unit_tests/meta_details/mark_video_as_watched.rs`,
    // `unit_tests/ctx/mark_meta_item_as_watched.rs`, `models/library_with_filters.rs`) ---

    const PREV_TIME: u64 = 40 * 60 * 1000;
    const PREV_OVERALL: u64 = 5 * 60 * 1000;

    fn ep(season: u32, episode: u32) -> Video {
        Video { id: format!("tt123456:{season}:{episode}"), season: Some(season), episode: Some(episode), ..Default::default() }
    }

    /// core's `create_library_item`: playing `video_id`, 40 minutes in, flagged watched.
    fn playing(video_id: &str) -> PlayState {
        PlayState {
            video_id: Some(video_id.into()),
            time_offset: PREV_TIME,
            time_watched: PREV_TIME,
            overall_time_watched: PREV_OVERALL,
            flagged_watched: 1,
            duration: 60 * 60 * 1000,
            ..Default::default()
        }
    }

    fn mark(videos: &[Video], target: MarkTarget, watched: bool) -> VideoMark {
        VideoMark { videos: videos.to_vec(), target, watched }
    }

    const NOW: &str = "2026-10-17T10:00:00.000Z";

    #[test]
    fn marking_the_current_episode_watched_advances_to_the_next() {
        let videos = [ep(1, 1), ep(1, 2)];
        let mut s = playing("tt123456:1:1");
        s.mark_videos(&mark(&videos, MarkTarget::Video("tt123456:1:1".into()), true), NOW);
        assert_eq!(s.video_id.as_deref(), Some("tt123456:1:2"));
        assert_eq!((s.time_offset, s.time_watched, s.flagged_watched), (1, 0, 0));
        assert_eq!(s.overall_time_watched, PREV_OVERALL + PREV_TIME);
        assert!(s.watched_bitfield(&videos).get_video("tt123456:1:1"));
        assert!(!s.watched_bitfield(&videos).get_video("tt123456:1:2"));
        assert_eq!(s.times_watched, 0, "an episode mark leaves timesWatched alone");
    }

    #[test]
    fn marking_before_an_unreleased_episode_or_the_last_clears_the_resume() {
        let mut next = ep(1, 2);
        next.released = Some("2026-10-18T00:00:00.000Z".into());
        let mut s = playing("tt123456:1:1");
        s.mark_videos(&mark(&[ep(1, 1), next], MarkTarget::Video("tt123456:1:1".into()), true), NOW);
        assert_eq!((s.video_id.as_deref(), s.time_offset), (Some("tt123456:1:1"), 0));
        let mut s = playing("tt123456:1:2");
        s.mark_videos(&mark(&[ep(1, 1), ep(1, 2)], MarkTarget::Video("tt123456:1:2".into()), true), NOW);
        assert_eq!((s.video_id.as_deref(), s.time_offset), (Some("tt123456:1:2"), 0));
    }

    #[test]
    fn marking_a_season_watched_moves_the_resume_past_it() {
        let mut s = playing("tt123456:1:1");
        s.mark_videos(&mark(&[ep(1, 1), ep(1, 2)], MarkTarget::Season(1), true), NOW);
        assert_eq!((s.video_id.as_deref(), s.time_offset), (Some("tt123456:1:1"), 0), "nothing left: the progress is cleared");
        let videos = [ep(1, 1), ep(1, 2), ep(2, 1)];
        let mut s = playing("tt123456:1:1");
        s.mark_videos(&mark(&videos, MarkTarget::Season(1), true), NOW);
        assert_eq!((s.video_id.as_deref(), s.time_offset, s.time_watched, s.flagged_watched), (Some("tt123456:2:1"), 1, 0, 0));
        assert_eq!(s.overall_time_watched, PREV_OVERALL + PREV_TIME);
        let bits = s.watched_bitfield(&videos);
        assert!(bits.get_video("tt123456:1:1") && bits.get_video("tt123456:1:2") && !bits.get_video("tt123456:2:1"));
    }

    #[test]
    fn marking_another_season_or_unwatched_keeps_the_resume() {
        let videos = [ep(1, 1), ep(1, 2), ep(2, 1)];
        for (video_id, season) in [("tt123456:2:1", 1), ("tt123456:1:1", 2)] {
            let mut s = playing(video_id);
            let mut bits = s.watched_bitfield(&videos);
            bits.set_video(video_id, true);
            s.watched = Some(bits.to_string());
            s.mark_videos(&mark(&videos, MarkTarget::Season(season), true), NOW);
            assert_eq!((s.video_id.as_deref(), s.time_offset, s.time_watched), (Some(video_id), PREV_TIME, PREV_TIME));
        }
        let mut s = playing("tt123456:1:1");
        s.mark_videos(&mark(&[ep(1, 1), ep(1, 2)], MarkTarget::Season(1), false), NOW);
        assert_eq!((s.video_id.as_deref(), s.time_offset), (Some("tt123456:1:1"), PREV_TIME));
    }

    #[test]
    fn an_episode_mark_takes_its_release_as_last_watched_when_later() {
        let mut v = ep(1, 1);
        v.released = Some("2026-01-01T00:00:00.000Z".into());
        let mut s = PlayState { last_watched: Some("2020-01-01T00:00:00.000Z".into()), ..Default::default() };
        s.mark_videos(&mark(&[v.clone()], MarkTarget::Video(v.id.clone()), true), NOW);
        assert_eq!(s.last_watched.as_deref(), Some("2026-01-01T00:00:00.000Z"));
        let mut never = PlayState::default();
        never.mark_videos(&mark(&[v.clone()], MarkTarget::Video(v.id.clone()), true), NOW);
        assert_eq!(never.last_watched.as_deref(), Some("2026-01-01T00:00:00.000Z"));
        let mut unwatch = PlayState { last_watched: Some("2020-01-01T00:00:00.000Z".into()), ..Default::default() };
        unwatch.mark_videos(&mark(&[v.clone()], MarkTarget::Video(v.id), false), NOW);
        assert_eq!(unwatch.last_watched.as_deref(), Some("2020-01-01T00:00:00.000Z"), "unmarking leaves it");
    }

    #[test]
    fn marking_a_movie_counts_a_watch_and_unmarking_resets_the_count_only() {
        let mut s = PlayState { times_watched: 2, last_watched: Some("2020-01-01T00:00:00Z".into()), time_offset: 342_284, ..Default::default() };
        s.mark_as_watched(true, NOW);
        assert_eq!((s.times_watched, s.time_offset, s.last_watched.as_deref()), (3, 0, Some(NOW)));
        let mut s = PlayState { times_watched: 5, time_offset: 123_456, flagged_watched: 1, ..Default::default() };
        s.mark_as_watched(false, NOW);
        assert_eq!((s.times_watched, s.time_offset, s.flagged_watched), (0, 123_456, 1));
    }

    #[test]
    fn a_mark_rewrites_only_the_state_fields_it_changed() {
        let old = match serde_json::json!({"_id":"tt1","type":"movie","name":"Film","removed":false,"temp":false,"x":1,
            "state":{"timesWatched":1,"timeOffset":5000,"duration":9000,"noNotif":true,"lastWatched":"2020-01-01T00:00:00Z"}}) {
            serde_json::Value::Object(m) => m,
            _ => unreachable!(),
        };
        let v = edited_entry(old.clone(), &Edit::MarkWatched(true), NOW);
        assert_eq!(v["state"]["timesWatched"], 2);
        assert_eq!(v["state"]["timeOffset"], 0);
        assert_eq!(v["state"]["lastWatched"], NOW);
        assert_eq!(v["state"]["noNotif"], true, "a state field this client does not know is kept");
        assert!(v["state"].get("flaggedWatched").is_none(), "an unchanged field is not written");
        assert_eq!((v["x"].clone(), v["_mtime"].clone()), (1.into(), NOW.into()));
        let v = edited_entry(old, &Edit::MarkWatched(false), NOW);
        assert_eq!((v["state"]["timesWatched"].clone(), v["state"]["timeOffset"].clone()), (0.into(), 5000.into()));
        let i: LibraryItem = serde_json::from_value(v).unwrap();
        assert!(!i.watched());
    }

    /// core's `MetaItemMarkAsWatched` on a title the library lacks: a temporary entry, watched
    /// once; unmarking one it lacks writes nothing.
    #[test]
    fn marking_a_title_the_library_lacks_writes_a_temporary_entry() {
        let api = Api {
            answers: [("datastoreGet", r#"{"result":[]}"#), ("datastorePut", r#"{"result":{"success":true}}"#)].into(),
            bodies: Default::default(),
        };
        mark_library(&api, "KEY", &entry(), Edit::MarkWatched(true), NOW).unwrap();
        {
            let bodies = api.bodies.borrow();
            let put = &bodies[1].1["changes"][0];
            assert_eq!((put["_id"].clone(), put["temp"].clone(), put["removed"].clone()), ("tt1".into(), true.into(), true.into()));
            assert_eq!((put["state"]["timesWatched"].clone(), put["state"]["lastWatched"].clone()), (1.into(), NOW.into()));
        }
        api.bodies.borrow_mut().clear();
        mark_library(&api, "KEY", &entry(), Edit::MarkWatched(false), NOW).unwrap();
        assert_eq!(api.bodies.borrow().len(), 1, "nothing to unmark: only the read");
        api.bodies.borrow_mut().clear();
        let videos = vec![ep(1, 1)];
        mark_library(&api, "KEY", &entry(), Edit::MarkVideos(mark(&videos, MarkTarget::Video("tt123456:1:1".into()), true)), NOW).unwrap();
        let bodies = api.bodies.borrow();
        let w = bodies[1].1["changes"][0]["state"]["watched"].as_str().unwrap().to_string();
        assert!(WatchedBitField::of_state(Some(&w), &videos).get_video("tt123456:1:1"), "{w}");
    }

    /// core's `test_watched_and_not_watched_sort_items_ordering_of_library_items`.
    #[test]
    fn the_watched_sorts_order_as_core_s() {
        let lib = |id: &str, name: &str, last: &str, times: u32, ctime: &str| -> LibraryItem {
            item(&format!(r#"{{"_id":"{id}","type":"movie","name":"{name}","_ctime":"{ctime}","state":{{"lastWatched":"{last}","timesWatched":{times}}}}}"#))
        };
        let series = lib("tt13622776", "Ahsoka", "2026-10-17T00:00:00Z", 2, "2026-10-17T00:00:00Z");
        let week = lib("tt15398776", "Oppenheimer", "2026-10-10T00:00:00Z", 1, "2026-10-17T00:00:00Z");
        let three = lib("tt2267998", "Gone Girl", "2026-09-26T00:00:00Z", 0, "2026-09-26T00:00:00Z");
        let two = lib("tt0118715", "The Big Lebowski", "2026-10-03T00:00:00Z", 0, "2026-10-03T00:00:00Z");
        let unmarked = lib("tt1462764", "Indiana Jones and the Dial of Destiny", "2026-10-10T00:00:00Z", 0, "2026-09-19T00:00:00Z");
        fn sorted(sort: Sort, mut v: Vec<&LibraryItem>) -> Vec<String> {
            v.sort_by(|a, b| sort.cmp(a, b));
            v.into_iter().map(|i| i.id.clone()).collect()
        }
        assert_eq!(
            sorted(Sort::Watched, vec![&three, &unmarked, &two, &week, &series]),
            ["tt13622776", "tt15398776", "tt1462764", "tt0118715", "tt2267998"]
        );
        assert_eq!(
            sorted(Sort::NotWatched, vec![&three, &series, &two, &week, &unmarked]),
            ["tt2267998", "tt0118715", "tt1462764", "tt15398776", "tt13622776"]
        );
        assert_eq!(sorted(Sort::Name, vec![&two, &series, &three]), ["tt13622776", "tt2267998", "tt0118715"]);
        assert_eq!(sorted(Sort::NameReverse, vec![&two, &series, &three]), ["tt0118715", "tt2267998", "tt13622776"]);
        assert_eq!(sorted(Sort::TimesWatched, vec![&three, &week, &series]), ["tt13622776", "tt15398776", "tt2267998"]);
        assert_eq!(sorted(Sort::LastWatched, vec![&three, &series, &week]), ["tt13622776", "tt15398776", "tt2267998"]);
    }
}
