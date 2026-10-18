//! **Intro, recap and credits markers** for the player's Skip button and its Up Next timing, from
//! two public, keyless, crowd-sourced services:
//!
//! - **introdb.app** for IMDb ids (`tt…` a movie, `tt…:S:E` an episode):
//!   `GET https://api.introdb.app/segments?imdb_id=…&season=…&episode=…` (`&is_movie=true` for a
//!   movie) answers `{intro, recap, outro, post_credits}`, each `null` or
//!   `{start_ms, end_ms, …}`. An id it does not know is a 200 with every segment `null`.
//! - **AniSkip** for anime (`mal:ID:EP`, `kitsu:ID:EP`, and an IMDb episode introdb has nothing
//!   for, whose season arm.haglund.dev maps to one MyAnimeList id):
//!   `GET https://api.aniskip.com/v2/skip-times/{mal}/{ep}?types[]=op…&episodeLength={s}` answers
//!   `{found, results: [{interval: {startTime, endTime}, skipType, episodeLength}]}`, and a 404
//!   (`found: false`) when it has nothing. A Kitsu id is mapped to MyAnimeList's through
//!   `kitsu.io/api/edge/anime/{id}/mappings`, once per show for the session.
//!
//! Crowd-sourced markers are taken only when they are plausible for THIS stream ([`valid`]): an
//! intro or recap of 3–240 s inside the duration, credits starting in the second half, and an
//! AniSkip submission timed against an encode within ±5 s of ours — a marker made on another
//! encode would skip the wrong part, which is worse than offering nothing.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;

use crate::{Fetch, FetchError};

/// A stretch of the stream, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Span {
    pub fn contains(&self, ms: u64) -> bool {
        self.start_ms <= ms && ms < self.end_ms
    }
}

/// What a marker marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SegKind {
    Intro,
    Recap,
    Credits,
}

/// A skippable stretch the playhead is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Segment {
    pub kind: SegKind,
    pub span: Span,
}

/// One playback's validated markers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Skips {
    pub intro: Option<Span>,
    pub recap: Option<Span>,
    pub credits: Option<Span>,
}

impl Skips {
    pub const NONE: Skips = Skips { intro: None, recap: None, credits: None };

    /// The intro or recap the playhead `pos` is inside (the credits are Up Next's, not a skip).
    pub fn at(&self, pos: u64) -> Option<Segment> {
        [(SegKind::Recap, self.recap), (SegKind::Intro, self.intro)]
            .into_iter()
            .find_map(|(kind, s)| s.filter(|s| s.contains(pos)).map(|span| Segment { kind, span }))
    }

    pub fn is_empty(&self) -> bool {
        *self == Skips::NONE
    }

    /// These markers, each missing one taken from `other`.
    pub fn or(self, other: Skips) -> Skips {
        Skips { intro: self.intro.or(other.intro), recap: self.recap.or(other.recap), credits: self.credits.or(other.credits) }
    }

    fn put(&mut self, kind: SegKind, span: Span) {
        let slot = match kind {
            SegKind::Intro => &mut self.intro,
            SegKind::Recap => &mut self.recap,
            SegKind::Credits => &mut self.credits,
        };
        slot.get_or_insert(span);
    }
}

/// The shortest and longest plausible intro or recap.
pub const SKIP_MIN_MS: u64 = 3_000;
pub const SKIP_MAX_MS: u64 = 240_000;
/// How far AniSkip's `episodeLength` may be from our duration.
pub const LENGTH_TOLERANCE_MS: u64 = 5_000;

/// `span` as a `kind` marker of a stream `dur_ms` long, or `None` when it is implausible. Credits
/// may run past the end (another encode's tail) and are cut to it.
pub fn valid(kind: SegKind, span: Span, dur_ms: u64) -> Option<Span> {
    if dur_ms == 0 || span.start_ms >= span.end_ms {
        return None;
    }
    match kind {
        SegKind::Intro | SegKind::Recap => {
            let len = span.end_ms - span.start_ms;
            (span.end_ms <= dur_ms && (SKIP_MIN_MS..=SKIP_MAX_MS).contains(&len)).then_some(span)
        }
        SegKind::Credits => (span.start_ms < dur_ms && span.start_ms >= dur_ms / 2)
            .then_some(Span { start_ms: span.start_ms, end_ms: span.end_ms.min(dur_ms) }),
    }
}

/// Which service a video id is asked of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// `tt…` and, for an episode, its season and number.
    IntroDb { imdb: String, episode: Option<(u32, u32)> },
    /// A MyAnimeList id and the episode.
    AniSkip { mal: u64, episode: u32 },
    /// A Kitsu id (mapped to MyAnimeList's first) and the episode.
    Kitsu { kitsu: u64, episode: u32 },
}

impl Lookup {
    /// The lookup a Stremio video id calls for; `None` for a scheme no service knows.
    pub fn of(video_id: &str) -> Option<Lookup> {
        let parts: Vec<&str> = video_id.split(':').collect();
        let num = |s: &str| (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u64>().ok()).flatten();
        let ep = |s: &str| num(s).and_then(|n| u32::try_from(n).ok());
        match parts.as_slice() {
            [tt] if tt.starts_with("tt") && num(&tt[2..]).is_some() => Some(Lookup::IntroDb { imdb: tt.to_string(), episode: None }),
            [tt, s, e] if tt.starts_with("tt") && num(&tt[2..]).is_some() => {
                Some(Lookup::IntroDb { imdb: tt.to_string(), episode: Some((ep(s)?, ep(e)?)) })
            }
            ["mal", id, e] => Some(Lookup::AniSkip { mal: num(id)?, episode: ep(e)? }),
            // a Kitsu entry is one season: the episode is the last segment
            ["kitsu", id, .., e] => Some(Lookup::Kitsu { kitsu: num(id)?, episode: ep(e)? }),
            _ => None,
        }
    }

    /// The lookup for an id no service knows (`tmdb:…`, `tvdb:…`) of a work whose IMDb id is
    /// `imdb`: `scheme:id:S:E` an episode, `scheme:id` a movie.
    pub fn with_imdb(video_id: &str, imdb: &str) -> Option<Lookup> {
        let Lookup::IntroDb { imdb, episode: None } = Lookup::of(imdb)? else { return None };
        match video_id.split(':').collect::<Vec<_>>().as_slice() {
            [_, _] => Some(Lookup::IntroDb { imdb, episode: None }),
            [_, _, s, e] => Some(Lookup::IntroDb { imdb, episode: Some((s.parse().ok()?, e.parse().ok()?)) }),
            _ => None,
        }
    }

    /// The service's name, for the log.
    pub fn source(&self) -> &'static str {
        match self {
            Lookup::IntroDb { .. } => "introdb",
            Lookup::AniSkip { .. } | Lookup::Kitsu { .. } => "aniskip",
        }
    }
}

// ---- introdb.app -------------------------------------------------------------------------------

#[derive(Deserialize)]
struct IdbSeg {
    start_ms: f64,
    end_ms: f64,
}

#[derive(Deserialize)]
struct IdbResponse {
    #[serde(default)]
    intro: Option<IdbSeg>,
    #[serde(default)]
    recap: Option<IdbSeg>,
    #[serde(default)]
    outro: Option<IdbSeg>,
}

fn ms(v: f64) -> u64 {
    if v.is_finite() && v > 0.0 {
        v.round() as u64
    } else {
        0
    }
}

/// introdb's answer, validated against `dur_ms`.
pub fn parse_introdb(body: &[u8], dur_ms: u64) -> Result<Skips, FetchError> {
    let r: IdbResponse = serde_json::from_slice(body).map_err(|e| FetchError::Decode(e.to_string()))?;
    let mut out = Skips::NONE;
    for (kind, seg) in [(SegKind::Intro, r.intro), (SegKind::Recap, r.recap), (SegKind::Credits, r.outro)] {
        if let Some(span) = seg.and_then(|s| valid(kind, Span { start_ms: ms(s.start_ms), end_ms: ms(s.end_ms) }, dur_ms)) {
            out.put(kind, span);
        }
    }
    Ok(out)
}

pub fn introdb_url(imdb: &str, episode: Option<(u32, u32)>) -> String {
    match episode {
        Some((s, e)) => format!("https://api.introdb.app/segments?imdb_id={imdb}&season={s}&episode={e}"),
        None => format!("https://api.introdb.app/segments?imdb_id={imdb}&is_movie=true"),
    }
}

// ---- AniSkip -----------------------------------------------------------------------------------

#[derive(Deserialize)]
struct AsInterval {
    #[serde(rename = "startTime")]
    start: f64,
    #[serde(rename = "endTime")]
    end: f64,
}

#[derive(Deserialize)]
struct AsResult {
    interval: AsInterval,
    #[serde(rename = "skipType")]
    kind: String,
    #[serde(rename = "episodeLength")]
    length: f64,
}

#[derive(Deserialize)]
struct AsResponse {
    #[serde(default)]
    found: bool,
    #[serde(default)]
    results: Vec<AsResult>,
}

/// AniSkip's answer, validated against `dur_ms`: only submissions timed on an encode within
/// [`LENGTH_TOLERANCE_MS`] of ours, a plain opening/ending before a mixed one.
pub fn parse_aniskip(body: &[u8], dur_ms: u64) -> Result<Skips, FetchError> {
    let r: AsResponse = serde_json::from_slice(body).map_err(|e| FetchError::Decode(e.to_string()))?;
    let mut out = Skips::NONE;
    if !r.found {
        return Ok(out);
    }
    for want in ["op", "ed", "recap", "mixed-op", "mixed-ed"] {
        let kind = match want {
            "op" | "mixed-op" => SegKind::Intro,
            "ed" | "mixed-ed" => SegKind::Credits,
            _ => SegKind::Recap,
        };
        for res in r.results.iter().filter(|r| r.kind == want) {
            if (ms(res.length * 1000.0)).abs_diff(dur_ms) > LENGTH_TOLERANCE_MS {
                continue;
            }
            let span = Span { start_ms: ms(res.interval.start * 1000.0), end_ms: ms(res.interval.end * 1000.0) };
            if let Some(span) = valid(kind, span, dur_ms) {
                out.put(kind, span);
            }
        }
    }
    Ok(out)
}

pub fn aniskip_url(mal: u64, episode: u32, dur_ms: u64) -> String {
    let t = "types%5B%5D=";
    format!(
        "https://api.aniskip.com/v2/skip-times/{mal}/{episode}?{t}op&{t}ed&{t}mixed-op&{t}mixed-ed&{t}recap&episodeLength={}",
        dur_ms / 1000
    )
}

// ---- Kitsu → MyAnimeList -----------------------------------------------------------------------

#[derive(Deserialize)]
struct KitsuAttrs {
    #[serde(rename = "externalSite")]
    site: String,
    #[serde(rename = "externalId")]
    id: String,
}

#[derive(Deserialize)]
struct KitsuMapping {
    attributes: KitsuAttrs,
}

#[derive(Deserialize)]
struct KitsuMappings {
    #[serde(default)]
    data: Vec<KitsuMapping>,
}

/// The MyAnimeList id in a Kitsu mappings answer.
pub fn parse_kitsu_mal(body: &[u8]) -> Result<Option<u64>, FetchError> {
    let r: KitsuMappings = serde_json::from_slice(body).map_err(|e| FetchError::Decode(e.to_string()))?;
    Ok(r.data.iter().find(|m| m.attributes.site == "myanimelist/anime").and_then(|m| m.attributes.id.parse().ok()))
}

pub fn kitsu_url(kitsu: u64) -> String {
    format!("https://kitsu.io/api/edge/anime/{kitsu}/mappings?filter%5BexternalSite%5D=myanimelist%2Fanime")
}

/// Kitsu → MyAnimeList, per show, for the session (an answer, found or not, is kept; a failure
/// is asked again next time).
static KITSU_MAL: Mutex<Option<HashMap<u64, Option<u64>>>> = Mutex::new(None);

fn kitsu_mal(fetch: &dyn Fetch, kitsu: u64) -> Result<Option<u64>, FetchError> {
    let cached = KITSU_MAL.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&kitsu).copied());
    if let Some(mal) = cached {
        return Ok(mal);
    }
    let mal = match fetch.get(&kitsu_url(kitsu), &[]) {
        Ok(body) => parse_kitsu_mal(&body)?,
        Err(FetchError::Status(404)) => None,
        Err(e) => return Err(e),
    };
    KITSU_MAL.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(kitsu, mal);
    Ok(mal)
}

// ---- IMDb season → MyAnimeList (arm.haglund.dev) ----------------------------------------------

#[derive(Deserialize)]
struct ArmEntry {
    #[serde(default)]
    myanimelist: Option<u64>,
    #[serde(default, rename = "thetvdb-season")]
    tvdb_season: Option<i64>,
    #[serde(default)]
    media: Option<String>,
}

pub fn arm_url(imdb: &str) -> String {
    format!("https://arm.haglund.dev/api/v2/imdb?id={imdb}")
}

/// The MyAnimeList id of `season` in an arm answer: its one TV/ONA entry. A season split into
/// parts maps to several and is not guessed (arm carries no episode offset), and `[]` is not an
/// anime at all.
pub fn parse_arm_mal(body: &[u8], season: u32) -> Result<Option<u64>, FetchError> {
    let entries: Vec<ArmEntry> = serde_json::from_slice(body).map_err(|e| FetchError::Decode(e.to_string()))?;
    let mut hits = entries.iter().filter(|e| {
        e.tvdb_season == Some(i64::from(season)) && matches!(e.media.as_deref(), Some("TV" | "ONA"))
    });
    Ok(match (hits.next(), hits.next()) {
        (Some(e), None) => e.myanimelist,
        _ => None,
    })
}

/// IMDb id + season → MyAnimeList, per show and season, for the session (as [`KITSU_MAL`]).
static ARM_MAL: Mutex<Option<HashMap<(String, u32), Option<u64>>>> = Mutex::new(None);

fn imdb_mal(fetch: &dyn Fetch, imdb: &str, season: u32) -> Result<Option<u64>, FetchError> {
    let key = (imdb.to_owned(), season);
    let cached = ARM_MAL.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key).copied());
    if let Some(mal) = cached {
        return Ok(mal);
    }
    let mal = match fetch.get(&arm_url(imdb), &[]) {
        Ok(body) => parse_arm_mal(&body, season)?,
        Err(FetchError::Status(404)) => None,
        Err(e) => return Err(e),
    };
    ARM_MAL.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, mal);
    Ok(mal)
}

/// What a chapter's title says it is: "Intro", "Opening", "OP2", "Main Titles", "Recap",
/// "Previously on…", "Credits", "Ending", "ED", "Outro" (and the German/French names releases use).
pub fn chapter_kind(title: &str) -> Option<SegKind> {
    let t: String = title.chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { ' ' }).collect();
    let mut w = t.split_whitespace();
    let first = w.next()?.trim_end_matches(|c: char| c.is_ascii_digit());
    let second = w.next().unwrap_or("");
    match (first, second) {
        ("intro" | "op" | "opening" | "vorspann" | "generique" | "générique", _) | ("main", "title" | "titles") | ("title", "sequence") | ("titles", "") => {
            Some(SegKind::Intro)
        }
        ("recap" | "previously" | "rückblick" | "zusammenfassung", _) | ("last", "time") => Some(SegKind::Recap),
        ("credits" | "ending" | "ed" | "outro" | "abspann", _) | ("end" | "closing", "credits" | "titles" | "") => Some(SegKind::Credits),
        _ => None,
    }
}

/// The markers a file's own chapters name: each chapter runs to the next one's start (the last to
/// the end) and is held to [`valid`] like a service's. They are exact for THIS encode.
pub fn from_chapters<'a>(chapters: impl IntoIterator<Item = (u64, &'a str)>, dur_ms: u64) -> Skips {
    let ch: Vec<(u64, &str)> = chapters.into_iter().collect();
    let mut s = Skips::NONE;
    for (i, (start, title)) in ch.iter().enumerate() {
        let end = ch.get(i + 1).map_or(dur_ms, |n| n.0);
        if let Some(kind) = chapter_kind(title) {
            if let Some(span) = valid(kind, Span { start_ms: *start, end_ms: end }, dur_ms) {
                s.put(kind, span);
            }
        }
    }
    s
}

/// Ask `lookup`'s service for the markers of a stream `dur_ms` long (blocking: a worker's job).
/// Nothing known is `Ok(Skips::NONE)`.
pub fn fetch_skips(fetch: &dyn Fetch, lookup: &Lookup, dur_ms: u64) -> Result<Skips, FetchError> {
    let aniskip = |mal: u64, episode: u32| match fetch.get(&aniskip_url(mal, episode, dur_ms), &[]) {
        Ok(body) => parse_aniskip(&body, dur_ms),
        // AniSkip says "nothing" with a 404
        Err(FetchError::Status(404)) => Ok(Skips::NONE),
        Err(e) => Err(e),
    };
    match lookup {
        Lookup::IntroDb { imdb, episode } => {
            let found = parse_introdb(&fetch.get(&introdb_url(imdb, *episode), &[])?, dur_ms)?;
            // nothing on introdb: an anime episode (Cinemeta ids) may be on AniSkip by its season
            match (found.is_empty(), episode) {
                (true, Some((season, e))) => match imdb_mal(fetch, imdb, *season) {
                    Ok(Some(mal)) => aniskip(mal, *e),
                    _ => Ok(found),
                },
                _ => Ok(found),
            }
        }
        Lookup::AniSkip { mal, episode } => aniskip(*mal, *episode),
        Lookup::Kitsu { kitsu, episode } => match kitsu_mal(fetch, *kitsu)? {
            Some(mal) => aniskip(mal, *episode),
            None => Ok(Skips::NONE),
        },
    }
}

/// The markers, for the log: `intro 314.5-331.0 credits 2843.0-2901.0`, or `none`.
pub fn describe(s: &Skips) -> String {
    let parts: Vec<String> = [("intro", s.intro), ("recap", s.recap), ("credits", s.credits)]
        .into_iter()
        .filter_map(|(n, sp)| sp.map(|sp| format!("{n} {:.1}-{:.1}", sp.start_ms as f64 / 1000.0, sp.end_ms as f64 / 1000.0)))
        .collect();
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFetch;

    const BB_S1E2: &str = include_str!("fixtures/introdb_episode.json");
    const RECAP: &str = include_str!("fixtures/introdb_episode_recap.json");
    const MOVIE: &str = include_str!("fixtures/introdb_movie.json");
    const UNKNOWN: &str = include_str!("fixtures/introdb_unknown.json");
    const ANISKIP: &str = include_str!("fixtures/aniskip_episode.json");
    const ANISKIP_NONE: &str = include_str!("fixtures/aniskip_not_found.json");
    const KITSU: &str = include_str!("fixtures/kitsu_mappings.json");

    fn span(a: u64, b: u64) -> Span {
        Span { start_ms: a, end_ms: b }
    }

    #[test]
    fn chapter_titles_name_their_markers() {
        for (t, k) in [
            ("Intro", Some(SegKind::Intro)),
            ("OP2 - Song", Some(SegKind::Intro)),
            ("Opening Credits", Some(SegKind::Intro)),
            ("Main Titles", Some(SegKind::Intro)),
            ("Previously on...", Some(SegKind::Recap)),
            ("Recap", Some(SegKind::Recap)),
            ("End Credits", Some(SegKind::Credits)),
            ("ED", Some(SegKind::Credits)),
            ("Abspann", Some(SegKind::Credits)),
            ("Chapter 01", None),
            ("Opportunity", None),
            ("The End of Days", None),
            ("", None),
        ] {
            assert_eq!(chapter_kind(t), k, "{t}");
        }
        let ch = [(0, "Prologue"), (90_000, "Opening"), (180_000, "Part A"), (2_500_000, "Ending"), (2_590_000, "Preview")];
        let s = from_chapters(ch, 2_640_000);
        assert_eq!((s.intro, s.recap, s.credits), (Some(span(90_000, 180_000)), None, Some(span(2_500_000, 2_590_000))));
        // a whole story chapter named like a marker is too long to be one
        assert_eq!(from_chapters([(0, "Intro"), (600_000, "Rest")], 3_000_000), Skips::NONE);
        assert_eq!(Skips { intro: Some(span(1, 5000)), ..Skips::NONE }.or(s).intro, Some(span(1, 5000)));
        assert_eq!(Skips::NONE.or(s), s);
    }

    #[test]
    fn an_id_no_service_knows_is_asked_by_its_imdb_id() {
        assert_eq!(Lookup::with_imdb("tmdb:1396:1:2", "tt0903747"), Some(Lookup::IntroDb { imdb: "tt0903747".into(), episode: Some((1, 2)) }));
        assert_eq!(Lookup::with_imdb("tmdb:157336", "tt0816692"), Some(Lookup::IntroDb { imdb: "tt0816692".into(), episode: None }));
        for (v, i) in [("tmdb:1:1:1", "tt1:1:1"), ("tmdb:1:1:1", "x"), ("tmdb:1:a:1", "tt1"), ("tmdb:1:1", "tt1")] {
            assert_eq!(Lookup::with_imdb(v, i), None, "{v} {i}");
        }
    }

    #[test]
    fn the_service_follows_the_id_scheme() {
        assert_eq!(Lookup::of("tt0903747:1:2"), Some(Lookup::IntroDb { imdb: "tt0903747".into(), episode: Some((1, 2)) }));
        assert_eq!(Lookup::of("tt0816692"), Some(Lookup::IntroDb { imdb: "tt0816692".into(), episode: None }));
        assert_eq!(Lookup::of("mal:16498:1"), Some(Lookup::AniSkip { mal: 16498, episode: 1 }));
        assert_eq!(Lookup::of("kitsu:7442:3"), Some(Lookup::Kitsu { kitsu: 7442, episode: 3 }));
        for none in ["", "tt", "ttx1", "tt1:a:2", "tt1:1", "kitsu:7442", "mal:x:1", "yt_id:abc", "tmdb:1:1:1", "tt1&x=1"] {
            assert_eq!(Lookup::of(none), None, "{none}");
        }
        assert_eq!(Lookup::of("mal:1:1").unwrap().source(), "aniskip");
    }

    #[test]
    fn introdb_answers_parse_and_validate() {
        let s = parse_introdb(BB_S1E2.as_bytes(), 2_901_000).unwrap();
        assert_eq!(s, Skips { intro: Some(span(314_500, 331_000)), recap: None, credits: Some(span(2_843_000, 2_901_000)) });
        let r = parse_introdb(RECAP.as_bytes(), 3_270_000).unwrap();
        assert_eq!(r.recap, Some(span(20, 83_979)));
        assert_eq!(r.at(50_000).map(|s| s.kind), Some(SegKind::Recap));
        assert_eq!(r.at(300_000).map(|s| s.kind), Some(SegKind::Intro));
        assert_eq!(r.at(200_000), None);
        // a movie's credits, cut to a shorter encode's end
        let m = parse_introdb(MOVIE.as_bytes(), 10_000_000).unwrap();
        assert_eq!(m, Skips { credits: Some(span(9_839_801, 10_000_000)), ..Skips::NONE });
        assert!(parse_introdb(UNKNOWN.as_bytes(), 3_000_000).unwrap().is_empty(), "an unknown episode is all nulls");
        assert!(parse_introdb(b"{\"error\":\"Invalid query params.\"}", 3_000_000).unwrap().is_empty());
        assert!(matches!(parse_introdb(b"<html>", 1), Err(FetchError::Decode(_))));
        assert_eq!(describe(&s), "intro 314.5-331.0 credits 2843.0-2901.0");
    }

    #[test]
    fn implausible_markers_are_dropped() {
        let d = 3_000_000;
        assert_eq!(valid(SegKind::Intro, span(10_000, 70_000), d), Some(span(10_000, 70_000)));
        assert_eq!(valid(SegKind::Intro, span(10_000, 12_000), d), None, "under 3 s");
        assert_eq!(valid(SegKind::Recap, span(0, 241_000), d), None, "over 240 s");
        assert_eq!(valid(SegKind::Intro, span(50_000, 40_000), d), None, "backwards");
        assert_eq!(valid(SegKind::Intro, span(2_990_000, 3_010_000), d), None, "past the end");
        assert_eq!(valid(SegKind::Intro, span(1, 10_000), 0), None, "no duration (live)");
        assert_eq!(valid(SegKind::Credits, span(1_400_000, 1_500_000), d), None, "credits before the middle");
        assert_eq!(valid(SegKind::Credits, span(3_000_000, 3_100_000), d), None, "credits after the end");
        assert_eq!(valid(SegKind::Credits, span(2_900_000, 3_100_000), d), Some(span(2_900_000, 3_000_000)));
    }

    #[test]
    fn aniskip_takes_only_submissions_timed_on_our_encode() {
        // the fixture holds submissions from 1420 s to 1540 s encodes
        let ours = parse_aniskip(ANISKIP.as_bytes(), 1_447_000).unwrap();
        assert_eq!(ours, Skips { credits: Some(span(1_342_795, 1_430_616)), ..Skips::NONE }, "only the 1447 s ending");
        let long = parse_aniskip(ANISKIP.as_bytes(), 1_540_000).unwrap();
        assert_eq!(long, Skips { intro: Some(span(47_365, 137_365)), ..Skips::NONE });
        assert!(parse_aniskip(ANISKIP.as_bytes(), 1_200_000).unwrap().is_empty(), "no encode within 5 s");
        assert!(parse_aniskip(ANISKIP_NONE.as_bytes(), 1_447_000).unwrap().is_empty());
        // a mixed opening stands in only when there is no plain one
        let mixed = r#"{"found":true,"results":[
            {"interval":{"startTime":75,"endTime":135},"skipType":"mixed-op","episodeLength":1420},
            {"interval":{"startTime":80,"endTime":170},"skipType":"op","episodeLength":1421}]}"#;
        assert_eq!(parse_aniskip(mixed.as_bytes(), 1_420_000).unwrap().intro, Some(span(80_000, 170_000)));
    }

    /// An anime opened by its IMDb id (Cinemeta) that introdb has nothing for: its season is
    /// mapped to a MyAnimeList id (arm.haglund.dev, real answer trimmed) and asked of AniSkip.
    /// A season split into parts (Attack on Titan S3) maps to two entries and is not guessed.
    #[test]
    fn an_imdb_anime_falls_back_to_aniskip_through_its_season() {
        const ARM: &str = r#"[{"myanimelist":16498,"thetvdb-season":1,"themoviedb-season":1,"media":"TV"},
            {"myanimelist":18397,"thetvdb-season":0,"media":"OVA"},
            {"myanimelist":25777,"thetvdb-season":2,"media":"TV"},
            {"myanimelist":35760,"thetvdb-season":3,"media":"TV"},
            {"myanimelist":38524,"thetvdb-season":3,"media":"TV"}]"#;
        let dur = 1_447_000;
        let fetch = FakeFetch::default()
            .with(&introdb_url("tt2560140", Some((1, 1))), UNKNOWN)
            .with(&arm_url("tt2560140"), ARM)
            .with(&aniskip_url(16498, 1, dur), ANISKIP);
        let ep = |s, e| Lookup::IntroDb { imdb: "tt2560140".into(), episode: Some((s, e)) };
        assert_eq!(fetch_skips(&fetch, &ep(1, 1), dur).unwrap().credits, Some(span(1_342_795, 1_430_616)));
        assert_eq!(parse_arm_mal(ARM.as_bytes(), 3).unwrap(), None, "two parts: not guessed");
        assert_eq!(parse_arm_mal(ARM.as_bytes(), 2).unwrap(), Some(25777));
        assert_eq!(parse_arm_mal(b"[]", 1).unwrap(), None, "not an anime");
        fetch_skips(&fetch, &ep(1, 1), dur).unwrap();
        assert_eq!(fetch.asked.borrow().iter().filter(|u| u.contains("arm.haglund")).count(), 1, "mapped once per show");
        // introdb's own markers win: the mapping is never asked
        let bb = FakeFetch::default().with(&introdb_url("tt0903747", Some((1, 2))), BB_S1E2);
        fetch_skips(&bb, &Lookup::IntroDb { imdb: "tt0903747".into(), episode: Some((1, 2)) }, 2_900_000).unwrap();
        assert!(bb.asked.borrow().iter().all(|u| !u.contains("arm.haglund")));
    }

    #[test]
    fn a_kitsu_id_is_mapped_once_then_asked_of_aniskip() {
        let dur = 1_447_000;
        let fetch = FakeFetch::default().with(&kitsu_url(7442), KITSU).with(&aniskip_url(16498, 1, dur), ANISKIP);
        let lookup = Lookup::Kitsu { kitsu: 7442, episode: 1 };
        assert_eq!(fetch_skips(&fetch, &lookup, dur).unwrap().credits, Some(span(1_342_795, 1_430_616)));
        assert_eq!(fetch_skips(&fetch, &lookup, dur).unwrap().credits, Some(span(1_342_795, 1_430_616)));
        let kitsu_asks = fetch.asked.borrow().iter().filter(|u| u.contains("kitsu.io")).count();
        assert_eq!(kitsu_asks, 1, "the mapping is kept for the session");
        assert!(aniskip_url(16498, 1, dur).ends_with("&episodeLength=1447"));
        // AniSkip's 404 and an unknown introdb id are nothing, a transport failure is a failure
        let none = FakeFetch::default();
        assert_eq!(fetch_skips(&none, &Lookup::AniSkip { mal: 1, episode: 1 }, dur), Ok(Skips::NONE));
        assert!(fetch_skips(&none, &Lookup::IntroDb { imdb: "tt1".into(), episode: None }, dur).is_err());
        let down = FakeFetch::default().failing(&kitsu_url(9), FetchError::Transport("timed out".into()));
        assert!(fetch_skips(&down, &Lookup::Kitsu { kitsu: 9, episode: 1 }, dur).is_err());
    }
}
