//! **Which videos of an item were watched** — a faithful port of stremio-core's
//! `stremio-watched-bitfield` crate (`BitField8`, `WatchedField`, `WatchedBitField`) and of the
//! video orders core reads it against.
//!
//! A library item's `state.watched` is `{anchor video id}:{anchor length}:{bitfield}`: the
//! bitfield is one bit per video (bit `i % 8` of byte `i / 8`), zlib-deflated and base64'd; the
//! anchor is the LAST watched video and its 1-based position. The bits are positions in the
//! meta's videos in [`bitfield_order`]; when the meta gains or loses videos the anchor's new
//! position shifts the bits so they keep naming the same videos
//! ([`WatchedBitField::construct_with_videos`]). An anchor the meta no longer lists reads as
//! nothing watched, as in core.
//!
//! [`core_order`] is core's `MetaItem.videos` order (it sorts every meta it reads: numbered
//! episodes by season, specials last, then by episode; the rest by release), which
//! [`next_video`] walks.

use std::fmt;
use std::io::{Read, Write};

use crate::types::Video;

/// A parse failure of a `state.watched` string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

/// core's `BitField8`: `length` bits over `values`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitField8 {
    pub length: usize,
    values: Vec<u8>,
}

impl BitField8 {
    pub fn new(length: usize) -> Self {
        BitField8 { length, values: vec![0; length.div_ceil(8)] }
    }

    /// `values`, padded to `length` bits (`values.len() * 8` when `None`).
    pub fn new_with_values(mut values: Vec<u8>, length: Option<usize>) -> Self {
        let length = length.unwrap_or(values.len() * 8);
        let bytes = length.div_ceil(8);
        if bytes > values.len() {
            values.resize(bytes, 0);
        }
        BitField8 { length, values }
    }

    pub fn get(&self, i: usize) -> bool {
        self.values.get(i / 8).is_some_and(|b| (b >> (i % 8)) & 1 != 0)
    }

    /// Past the end grows the field to whole bytes, as core's does.
    pub fn set(&mut self, i: usize, val: bool) {
        let index = i / 8;
        if index >= self.values.len() {
            self.values.resize(index + 1, 0);
            self.length = self.values.len() * 8;
        }
        let mask = 1u8 << (i % 8);
        if val {
            self.values[index] |= mask;
        } else {
            self.values[index] &= !mask;
        }
    }

    pub fn last_index_of(&self, val: bool) -> Option<usize> {
        (0..self.length).rev().find(|&i| self.get(i) == val)
    }

    /// Decode `encoded` (base64 of zlib), `length` bits (`None`: every decoded byte's).
    pub fn decode(encoded: &str, length: Option<usize>) -> Result<Self, Error> {
        let compressed = base64_decode(encoded)?;
        let mut values = Vec::new();
        flate2::read::ZlibDecoder::new(&compressed[..]).read_to_end(&mut values).map_err(|e| Error(e.to_string()))?;
        Ok(Self::new_with_values(values, length))
    }
}

impl fmt::Display for BitField8 {
    /// zlib at core's level 6, base64'd.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        z.write_all(&self.values).map_err(|_| fmt::Error)?;
        f.write_str(&base64_encode(&z.finish().map_err(|_| fmt::Error)?))
    }
}

/// core's `WatchedField`: the string as stored, before it is laid over a meta's videos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchedField {
    pub anchor_video: String,
    pub anchor_length: usize,
    pub bitfield: BitField8,
}

impl std::str::FromStr for WatchedField {
    type Err = Error;

    /// `{id}:{len}:{buf}`; the id may hold `:` itself, so the last two are popped off.
    fn from_str(s: &str) -> Result<Self, Error> {
        let mut parts: Vec<&str> = s.split(':').collect();
        if parts.len() < 3 {
            return Err(Error("Not enough components".into()));
        }
        let buf = parts.pop().unwrap_or_default();
        let anchor_length = parts.pop().unwrap_or_default().parse::<usize>().map_err(|e| Error(e.to_string()))?;
        Ok(WatchedField { anchor_video: parts.join(":"), anchor_length, bitfield: BitField8::decode(buf, None)? })
    }
}

impl fmt::Display for WatchedField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.anchor_video, self.anchor_length, self.bitfield)
    }
}

/// core's `WatchedBitField`: the bits laid over a meta's video ids ([`bitfield_order`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchedBitField {
    bitfield: BitField8,
    video_ids: Vec<String>,
}

impl WatchedBitField {
    pub fn construct_from_array(arr: &[bool], video_ids: Vec<String>) -> Self {
        let mut bitfield = BitField8::new(video_ids.len());
        for (i, v) in arr.iter().enumerate() {
            bitfield.set(i, *v);
        }
        WatchedBitField { bitfield, video_ids }
    }

    /// Lay `field` over `video_ids`, shifted so its anchor keeps naming the same video; an anchor
    /// `video_ids` does not hold is a blank field.
    pub fn construct_with_videos(field: WatchedField, video_ids: Vec<String>) -> Self {
        let Some(anchor) = video_ids.iter().position(|id| *id == field.anchor_video) else {
            return WatchedBitField { bitfield: BitField8::new(video_ids.len()), video_ids };
        };
        let offset = field.anchor_length as i64 - anchor as i64 - 1;
        let bitfield = BitField8::new_with_values(field.bitfield.values, Some(video_ids.len()));
        if offset == 0 {
            return WatchedBitField { bitfield, video_ids };
        }
        let mut out = WatchedBitField { bitfield: BitField8::new(video_ids.len()), video_ids };
        for i in 0..out.video_ids.len() {
            let prev = i as i64 + offset;
            if prev >= 0 && (prev as usize) < bitfield.length {
                out.set(i, bitfield.get(prev as usize));
            }
        }
        out
    }

    pub fn construct_and_resize(serialized: &str, video_ids: Vec<String>) -> Result<Self, Error> {
        Ok(Self::construct_with_videos(serialized.parse()?, video_ids))
    }

    /// core's `LibraryItemState::watched_bitfield`: `state.watched` over `videos` (a meta's), a
    /// blank field when there is none or it does not parse.
    pub fn of_state(watched: Option<&str>, videos: &[Video]) -> Self {
        let ids = bitfield_order(videos);
        match watched.and_then(|w| w.parse::<WatchedField>().ok()) {
            Some(field) => Self::construct_with_videos(field, ids),
            None => Self::construct_from_array(&[], ids),
        }
    }

    pub fn get(&self, idx: usize) -> bool {
        self.bitfield.get(idx)
    }

    pub fn get_video(&self, video_id: &str) -> bool {
        self.video_ids.iter().position(|id| id == video_id).is_some_and(|i| self.bitfield.get(i))
    }

    pub fn set(&mut self, idx: usize, v: bool) {
        self.bitfield.set(idx, v);
    }

    pub fn set_video(&mut self, video_id: &str, v: bool) {
        if let Some(i) = self.video_ids.iter().position(|id| id == video_id) {
            self.bitfield.set(i, v);
        }
    }

    /// The last watched video's id (the serialized anchor), `None` when nothing is.
    pub fn last_watched(&self) -> Option<&str> {
        let i = self.bitfield.last_index_of(true)?;
        self.video_ids.get(i).map(String::as_str)
    }
}

impl fmt::Display for WatchedBitField {
    /// What `state.watched` stores: the anchor is the last watched video (`undefined:1` when none).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let last = self.bitfield.last_index_of(true).unwrap_or(0);
        let id = self.video_ids.get(last).map_or("undefined", String::as_str);
        write!(f, "{id}:{}:{}", last + 1, self.bitfield)
    }
}

/// core's `SeriesInfo`: a video is an episode only with both numbers.
pub fn series_info(v: &Video) -> Option<(u32, u32)> {
    v.season.zip(v.episode)
}

/// core's `MetaItem.videos` order (`VideoSortedVecAdapter` over `VideoUniqueVecAdapter`): each id
/// once (the first), episodes before the rest, by season with specials (season 0) last, then
/// episode; the rest by release (oldest first in a list that has episodes, newest first in one
/// that has none), dated before undated. Stable.
// ponytail: `released` compares as RFC 3339 text, which orders as time for one addon's uniform
// format; parse it if an addon mixes offsets.
pub fn core_order(videos: &[Video]) -> Vec<&Video> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<&Video> = videos.iter().filter(|v| seen.insert(v.id.as_str())).collect();
    let is_series = out.iter().any(|v| series_info(v).is_some());
    out.sort_by(|a, b| match (series_info(a), series_info(b)) {
        (Some(a), Some(b)) => {
            let s = |s: u32| if s == 0 { u32::MAX } else { s };
            s(a.0).cmp(&s(b.0)).then(a.1.cmp(&b.1))
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => match (a.released.as_deref(), b.released.as_deref()) {
            (Some(x), Some(y)) if is_series => x.cmp(y),
            (Some(x), Some(y)) => y.cmp(x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        },
    });
    out
}

/// The video ids the bits are positions in (core's `watched_bitfield`): [`core_order`], then
/// stably by season, episode and release, a missing one first.
pub fn bitfield_order(videos: &[Video]) -> Vec<String> {
    let mut order = core_order(videos);
    let key = |v: &Video| {
        let si = series_info(v);
        (si.map_or(i64::MIN, |s| i64::from(s.0)), si.map_or(i64::MIN, |s| i64::from(s.1)))
    };
    order.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.released.as_deref().cmp(&b.released.as_deref())));
    order.into_iter().map(|v| v.id.clone()).collect()
}

/// core's `MetaItem::next_video`: the video after `video_id` in [`core_order`] (`order`), unless
/// it is a special after a numbered season or not released by `now` (RFC 3339).
pub fn next_video<'a>(order: &[&'a Video], video_id: &str, now: &str) -> Option<&'a Video> {
    let at = order.iter().position(|v| v.id == video_id)?;
    let (cur, next) = (order[at], *order.get(at + 1)?);
    let season = |v: &Video| series_info(v).map_or(0, |s| s.0);
    let released = next.released.as_deref().is_none_or(|r| r <= now);
    ((season(next) != 0 || season(cur) == season(next)) && released).then_some(next)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            out.push(if i <= c.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn base64_decode(s: &str) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = B64.iter().position(|b| *b == c).ok_or_else(|| Error(format!("invalid base64 byte {c}")))?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// stremio-core's own tests (`stremio-watched-bitfield/src/{bitfield8,watched_bitfield}.rs`).
#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_and_modify() {
        let videos = ids(&[
            "tt2934286:1:1", "tt2934286:1:2", "tt2934286:1:3", "tt2934286:1:4", "tt2934286:1:5",
            "tt2934286:1:6", "tt2934286:1:7", "tt2934286:1:8", "tt2934286:1:9",
        ]);
        let watched = "tt2934286:1:5:5:eJyTZwAAAEAAIA==";
        let mut wb = WatchedBitField::construct_and_resize(watched, videos).unwrap();
        assert!(wb.get_video("tt2934286:1:5"));
        assert!(!wb.get_video("tt2934286:1:6"));
        assert_eq!(watched, wb.to_string());
        wb.set_video("tt2934286:1:6", true);
        assert!(wb.get_video("tt2934286:1:6"));
    }

    #[test]
    fn construct_from_array() {
        let video_ids: Vec<String> = (1..500).map(|i| format!("tt2934286:1:{i}")).collect();
        let mut wb = WatchedBitField::construct_from_array(&[false; 500], video_ids.clone());
        for (i, v) in video_ids.iter().enumerate() {
            assert!(!wb.get(i) && !wb.get_video(v));
        }
        for i in 0..video_ids.len() {
            wb.set(i, i % 2 == 0);
        }
        let wb2 = WatchedBitField::construct_and_resize(&wb.to_string(), video_ids.clone()).unwrap();
        for (i, v) in video_ids.iter().enumerate() {
            assert_eq!((wb2.get(i), wb2.get_video(v)), (i % 2 == 0, i % 2 == 0));
        }
    }

    #[test]
    fn to_string_empty_and_back() {
        assert_eq!(WatchedBitField::construct_from_array(&[], vec![]).to_string(), "undefined:1:eJwDAAAAAAE=");
        let w = WatchedBitField::construct_and_resize("undefined:1:eJwDAAAAAAE=", vec![]).unwrap();
        assert_eq!(w, WatchedBitField { bitfield: BitField8::new(0), video_ids: vec![] });
    }

    #[test]
    fn watched_field_parses_an_id_with_colons() {
        let f: WatchedField = "tt7767422:3:8:24:eJz7//8/AAX9Av4=".parse().unwrap();
        assert_eq!((f.anchor_video.as_str(), f.anchor_length), ("tt7767422:3:8", 24));
        assert_eq!(f.bitfield.to_string(), "eJz7//8/AAX9Av4=");
        assert_eq!(f.to_string(), "tt7767422:3:8:24:eJz7//8/AAX9Av4=");
        assert!("x:1".parse::<WatchedField>().is_err());
    }

    #[test]
    fn the_anchor_points_to_the_last_watched_video() {
        let mut w = WatchedBitField::construct_from_array(&[], (1..=20).map(|i| format!("tt0:1:{i}")).collect());
        w.set_video("tt0:1:16", true);
        assert!(w.to_string().starts_with("tt0:1:16:16:"), "{w}");
        assert_eq!(w.last_watched(), Some("tt0:1:16"));
    }

    #[test]
    fn bitfield8_values_and_length() {
        let bf = BitField8::decode("eJyTZwAAAEAAIA==", None).unwrap();
        assert_eq!((bf.length, bf.values.clone()), (16, vec![31, 0]));
        assert_eq!(bf.to_string(), "eJyTZwAAAEAAIA==");
        assert_eq!(BitField8::new_with_values(vec![31, 0], Some(9)).to_string(), "eJyTZwAAAEAAIA==", "length is not serialized");
        assert_eq!(BitField8::decode("eJyTZwAAAEAAIA==", Some(9)).unwrap().length, 9);
        let mut bf = BitField8::new(20);
        assert_eq!(bf.length, 20);
        bf.set(15, true);
        assert_eq!(bf.last_index_of(true), Some(15));
    }

    /// The meta gained an episode before the anchor: the bits shift so they name the same videos.
    #[test]
    fn a_shifted_meta_keeps_the_same_videos_watched() {
        let old = ids(&["s:1:1", "s:1:2", "s:1:3"]);
        let mut w = WatchedBitField::construct_from_array(&[], old);
        w.set_video("s:1:2", true);
        let shifted = WatchedBitField::construct_and_resize(&w.to_string(), ids(&["s:0:1", "s:1:1", "s:1:2", "s:1:3"])).unwrap();
        assert!(shifted.get_video("s:1:2") && !shifted.get_video("s:1:1") && !shifted.get_video("s:0:1"));
        let gone = WatchedBitField::construct_and_resize(&w.to_string(), ids(&["x:1:1"])).unwrap();
        assert!(!gone.get(0), "an anchor the meta no longer lists is a blank field");
    }

    fn video(id: &str, season: Option<u32>, episode: Option<u32>, released: Option<&str>) -> Video {
        Video { id: id.into(), season, episode, released: released.map(str::to_string), ..Default::default() }
    }

    /// core's `next_video` test (`types/resource/meta_item.rs`): specials sort after the
    /// numbered seasons, and the walk does not run from a season into specials.
    #[test]
    fn core_order_and_next_video() {
        let videos = [
            video("s0e1", Some(0), Some(1), None),
            video("s0e2", Some(0), Some(2), None),
            video("s1e1", Some(1), Some(1), None),
            video("s1e2", Some(1), Some(2), None),
            video("s2e1", Some(2), Some(1), None),
            video("s0e3", Some(0), Some(3), None),
        ];
        let order = core_order(&videos);
        let got: Vec<&str> = order.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(got, ["s1e1", "s1e2", "s2e1", "s0e1", "s0e2", "s0e3"]);
        let next = |id| next_video(&order, id, "2026-01-01T00:00:00Z").map(|v| v.id.as_str());
        assert_eq!(next("s0e1"), Some("s0e2"));
        assert_eq!(next("s1e1"), Some("s1e2"));
        assert_eq!(next("s1e2"), Some("s2e1"));
        assert_eq!(next("s2e1"), None, "never into the specials");
        assert_eq!(next("s0e3"), None);
        assert_eq!(bitfield_order(&videos), ["s0e1", "s0e2", "s0e3", "s1e1", "s1e2", "s2e1"], "the bits put specials first");
        let unreleased = [video("a", Some(1), Some(1), None), video("b", Some(1), Some(2), Some("2026-01-02T00:00:00Z"))];
        assert_eq!(next_video(&core_order(&unreleased), "a", "2026-01-01T00:00:00Z"), None, "not released yet");
        assert_eq!(next_video(&core_order(&unreleased), "a", "2026-01-02T00:00:00Z").map(|v| v.id.as_str()), Some("b"));
    }
}
