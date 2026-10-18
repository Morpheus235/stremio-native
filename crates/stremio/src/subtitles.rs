//! **Subtitles from addons** for the file that plays.
//!
//! Every addon whose manifest declares the `subtitles` resource for the item's type and id prefix
//! is asked `subtitles/{type}/{video id}[/videoHash=…&videoSize=…&filename=….json]` — the video id
//! is the EPISODE's for a series, as for streams — with what the stream's addon said about the file
//! ([`FileHints`]), which is how OpenSubtitles matches an exact release. The extra segment is
//! encoded like stremio-core's `ResourcePath` (`encodeURIComponent`: a space is `%20`, brackets
//! and `&` escaped, dots and parentheses kept). Each answer is a list of `{id, url, lang}`; an
//! entry without an HTTP(S) URL is dropped.
//!
//! **Two transports.** An addon installed at `…/stremio/v1` speaks the LEGACY protocol (the old
//! "OpenSubtitles" addon of a Stremio account's default collection): one JSON-RPC GET,
//! `{base}/q.json?b={base64 of {"method":"subtitles.find","params":[null,{"query":{…}}]}}`,
//! answered `{"result":{"all":[…]}}` — stremio-core's `AddonLegacyTransport`. Asked the v3 path,
//! that server answers its HTML landing page with a 200.
//!
//! **A fallback.** A request with the file's hints that fails with a status or an answer that does
//! not decode is asked once more without them (the bare `subtitles/{type}/{id}.json`): an addon
//! that chokes on a release name still answers for the title.
//!
//! A source's URL is the addon's base, which carries its configuration, and a subtitle's URL can
//! carry it too: neither is ever logged ([`SubtitleSource`]'s and [`Subtitle`]'s `Debug` leave
//! them out).

use serde::Deserialize;

use crate::manifest::Addon;
use crate::types::{Stream, Subtitle, SubtitlesResponse};
use crate::url::resource_url;
use crate::{decode, Fetch, FetchError};

/// What a stream's addon said about the file (`behaviorHints`), which a subtitles addon matches a
/// release by. Server text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileHints {
    pub filename: Option<String>,
    pub video_size: Option<u64>,
    pub video_hash: Option<String>,
}

impl FileHints {
    pub fn of(stream: &Stream) -> FileHints {
        let h = &stream.behavior_hints;
        FileHints { filename: h.filename.clone(), video_size: h.video_size, video_hash: h.video_hash.clone() }
    }
}

/// The path that marks an addon on the legacy protocol (stremio-core's `ADDON_LEGACY_PATH`).
const LEGACY_PATH: &str = "/stremio/v1";

/// One addon to ask for subtitles.
#[derive(Clone, PartialEq, Eq)]
pub struct SubtitleSource {
    pub addon_id: String,
    /// The addon's name, for the menu ("Deutsch · OpenSubtitles") and the log. Server text.
    pub addon_name: String,
    /// The requests, in the order they are tried: with the file's hints, then (when there were
    /// any) without. Never logged: the base carries the addon's configuration.
    pub urls: Vec<String>,
    /// The answers are legacy JSON-RPC (`{"result":{"all":[…]}}`).
    pub legacy: bool,
}

impl std::fmt::Debug for SubtitleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubtitleSource").field("addon_id", &self.addon_id).field("legacy", &self.legacy).finish()
    }
}

/// One source per addon serving subtitles for `kind`/`video_id`, in addon order.
pub fn plan(addons: &[Addon], kind: &str, video_id: &str, hints: &FileHints) -> Vec<SubtitleSource> {
    let size = hints.video_size.map(|s| s.to_string()).unwrap_or_default();
    let extra = [
        ("videoHash", hints.video_hash.as_deref().unwrap_or("")),
        ("videoSize", size.as_str()),
        ("filename", hints.filename.as_deref().unwrap_or("")),
    ];
    let hinted = extra.iter().any(|(_, v)| !v.is_empty());
    addons
        .iter()
        .filter(|a| a.manifest.serves("subtitles", kind, video_id))
        .map(|a| {
            let legacy = a.base.ends_with(LEGACY_PATH);
            let url = |extra: &[(&str, &str)]| {
                if legacy {
                    legacy_url(&a.base, video_id, extra)
                } else {
                    resource_url(&a.base, "subtitles", kind, video_id, extra)
                }
            };
            let mut urls = vec![url(&extra)];
            if hinted {
                urls.push(url(&[]));
            }
            SubtitleSource {
                addon_id: a.manifest.id.clone(),
                addon_name: if a.manifest.name.is_empty() { a.manifest.id.clone() } else { a.manifest.name.clone() },
                urls,
                legacy,
            }
        })
        .collect()
}

/// The legacy protocol's `subtitles.find` (stremio-core's `build_legacy_req`): the item is
/// `itemHash` (the id's `:` as spaces), the hints go in as they are, the size as a number. The
/// standard base64 alphabet, not the URL-safe one, as stremio-addons' `rpc.js` does.
fn legacy_url(base: &str, video_id: &str, extra: &[(&str, &str)]) -> String {
    let mut query = serde_json::Map::new();
    query.insert("itemHash".into(), video_id.replace(':', " ").into());
    for (k, v) in extra.iter().filter(|(_, v)| !v.is_empty()) {
        let value = match (*k, v.parse::<u64>()) {
            ("videoSize", Ok(n)) => n.into(),
            _ => (*v).into(),
        };
        query.insert((*k).into(), value);
    }
    let rpc = serde_json::json!({
        "params": [serde_json::Value::Null, {"query": query}],
        "method": "subtitles.find",
        "id": 1,
        "jsonrpc": "2.0",
    });
    format!("{}/q.json?b={}", base.trim_end_matches('/'), sn_base::b64::encode(rpc.to_string().as_bytes()))
}

#[derive(Deserialize)]
struct LegacyAnswer {
    #[serde(default)]
    result: Option<LegacyResult>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct LegacyResult {
    #[serde(default)]
    all: SubtitlesList,
}

/// The legacy `all` list, read as leniently as a v3 `subtitles` list.
#[derive(Default)]
struct SubtitlesList(Vec<Subtitle>);

impl<'de> Deserialize<'de> for SubtitlesList {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        let r: SubtitlesResponse = serde_json::from_value(serde_json::json!({ "subtitles": v })).unwrap_or_default();
        Ok(SubtitlesList(r.subtitles))
    }
}

/// One request's answer.
fn ask(fetch: &dyn Fetch, url: &str, legacy: bool) -> Result<Vec<Subtitle>, FetchError> {
    let body = fetch.get(url, &[])?;
    if !legacy {
        return decode::<SubtitlesResponse>(&body).map(|r| r.subtitles);
    }
    match decode::<LegacyAnswer>(&body)? {
        LegacyAnswer { result: Some(r), .. } => Ok(r.all.0),
        LegacyAnswer { error: Some(_), .. } => Err(FetchError::Decode("legacy addon answered an error".into())),
        LegacyAnswer { .. } => Err(FetchError::Decode("legacy answer without a result".into())),
    }
}

/// Ask `source`: its subtitles with an HTTP(S) URL, in the addon's order. A request that fails
/// with a status or a body that does not decode falls back to the next one (the request without
/// the file's hints); a transport failure or a cancel does not (the network, not the request,
/// failed). The error is the LAST request's.
pub fn fetch(fetch: &dyn Fetch, source: &SubtitleSource) -> Result<Vec<Subtitle>, FetchError> {
    let mut last = FetchError::Transport("no request".into());
    for url in &source.urls {
        match ask(fetch, url, source.legacy) {
            Ok(subs) => return Ok(subs.into_iter().filter(|s| s.url.starts_with("https://") || s.url.starts_with("http://")).collect()),
            Err(e @ (FetchError::Status(_) | FetchError::Decode(_))) => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFetch;
    use crate::manifest::Manifest;

    fn addons() -> Vec<Addon> {
        let a = |url: &str, json: &str| Addon::new(url, Manifest::parse(json.as_bytes()).unwrap());
        vec![
            a("https://cinemeta.example/manifest.json", r#"{"id":"cinemeta","resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"]}"#),
            a("https://subs.example/cfg=k/manifest.json", r#"{"id":"subs","name":"Subs One","resources":["subtitles"],"types":["movie","series"],"idPrefixes":["tt"]}"#),
            a("https://anime.example/manifest.json", r#"{"id":"kitsu-subs","name":"Kitsu Subs","resources":[{"name":"subtitles","types":["anime"],"idPrefixes":["kitsu:"]}],"types":["anime"]}"#),
            a("https://streams.example/manifest.json", r#"{"id":"s1","name":"Streams","resources":["stream"],"types":["movie","series"],"idPrefixes":["tt"]}"#),
        ]
    }

    /// The legacy OpenSubtitles of an account's default collection: no idPrefixes, installed at
    /// `…/stremio/v1` (the collection carries a v3-shaped manifest for it).
    fn legacy() -> Addon {
        Addon::new(
            "https://legacy.example/stremio/v1",
            Manifest::parse(br#"{"id":"org.stremio.opensubtitles","name":"OpenSubtitles","resources":["subtitles"],"types":["series","movie","other"]}"#).unwrap(),
        )
    }

    #[test]
    fn only_addons_declaring_subtitles_for_the_type_and_prefix_are_asked() {
        let hints = FileHints { filename: Some("Show S02E05 1080p.mkv".into()), video_size: Some(1_234_567), video_hash: Some("8e245d9679d31e12".into()) };
        let s = plan(&addons(), "series", "tt0903747:2:5", &hints);
        assert_eq!(s.len(), 1, "cinemeta and a stream addon serve no subtitles; the anime addon not this prefix");
        assert_eq!(s[0].addon_name, "Subs One");
        assert_eq!(
            s[0].urls,
            [
                "https://subs.example/cfg=k/subtitles/series/tt0903747%3A2%3A5/videoHash=8e245d9679d31e12&videoSize=1234567&filename=Show%20S02E05%201080p.mkv.json",
                "https://subs.example/cfg=k/subtitles/series/tt0903747%3A2%3A5.json",
            ],
            "with the hints, then without"
        );
        let bare = plan(&addons(), "movie", "tt1", &FileHints::default());
        assert_eq!(bare[0].urls, ["https://subs.example/cfg=k/subtitles/movie/tt1.json"], "no hints: no extra segment, nothing to fall back to");
        assert_eq!(plan(&addons(), "anime", "kitsu:1:2", &FileHints::default())[0].addon_id, "kitsu-subs");
        assert!(!format!("{:?}", s[0]).contains("cfg=k"), "the source's Debug never shows its URL");
    }

    /// A release name with every character a filename carries is ONE value, encoded the way
    /// stremio-core encodes `extra` (`encodeURIComponent`): the URL the public OpenSubtitles v3
    /// addon answers (checked by hand with curl).
    #[test]
    fn a_nasty_filename_is_one_encoded_extra_value() {
        let hints = FileHints {
            filename: Some("Interstellar (2014) [2160p] {x265} 10bit & HDR+DV, 50% #1.mkv".into()),
            video_size: Some(68_719_476_736),
            video_hash: None,
        };
        let s = plan(&addons(), "movie", "tt0816692", &hints);
        assert_eq!(
            s[0].urls[0],
            "https://subs.example/cfg=k/subtitles/movie/tt0816692/videoSize=68719476736&filename=Interstellar%20(2014)%20%5B2160p%5D%20%7Bx265%7D%2010bit%20%26%20HDR%2BDV%2C%2050%25%20%231.mkv.json"
        );
        let unicode = FileHints { filename: Some("Amélie/Ä ö.mkv".into()), ..FileHints::default() };
        assert_eq!(plan(&addons(), "movie", "tt1", &unicode)[0].urls[0], "https://subs.example/cfg=k/subtitles/movie/tt1/filename=Am%C3%A9lie%2F%C3%84%20%C3%B6.mkv.json");
    }

    #[test]
    fn an_answer_keeps_its_http_subtitles_in_order_and_skips_the_rest() {
        let source = plan(&addons(), "movie", "tt1", &FileHints::default()).remove(0);
        let body = r#"{"subtitles":[
            {"id":"1","url":"https://dl.example/1.srt","lang":"eng","movieReleaseName":"Film.2014.1080p","subtitleFileName":"Film.SDH.srt"},
            {"id":"2","url":"magnet:?x","lang":"ger"},
            {"id":"3","lang":"fre"},
            {"id":"4","url":"http://dl.example/4.vtt","lang":"ger","SubEncoding":"CP1252","label":"Forced","movieReleaseName":7}
        ]}"#;
        let f = FakeFetch::default().with(&source.urls[0], body);
        let subs = fetch(&f, &source).unwrap();
        assert_eq!(subs.iter().map(|s| (s.id.as_str(), s.lang.as_str())).collect::<Vec<_>>(), [("1", "eng"), ("4", "ger")]);
        assert_eq!((subs[0].release(), subs[0].hearing_impaired(), subs[0].forced()), (Some("Film.2014.1080p"), true, false));
        assert_eq!((subs[1].release(), subs[1].forced()), (Some("Forced"), true), "the label leads; a number for a name is read leniently");
        assert!(!format!("{:?}", subs[0]).contains("dl.example"), "a subtitle's Debug never shows its URL");
        let empty = FakeFetch::default().with(&source.urls[0], "{}");
        assert_eq!(fetch(&empty, &source), Ok(vec![]), "no list is no subtitles, not a failure");
    }

    /// What the TV logged for every playback ("expected value at line 1 column 1") was an HTML
    /// page: the failure now says what the body was, never the URL or the text.
    #[test]
    fn a_body_that_is_not_json_says_what_it_was() {
        let source = plan(&addons(), "movie", "tt1", &FileHints::default()).remove(0);
        for (body, kind) in [("<!DOCTYPE html><head>", "html"), ("  <?xml version=\"1.0\"?>", "xml"), ("", "empty"), ("Too Many Requests", "text")] {
            let f = FakeFetch::default().with(&source.urls[0], body);
            let Err(FetchError::Decode(why)) = fetch(&f, &source) else { panic!("{kind}: a decode failure") };
            assert_eq!(why, format!("not JSON: {kind}, {} bytes", body.len()));
            assert!(!why.contains("subs.example"));
        }
        assert_eq!(crate::body_kind(&[0x1f, 0x8b, 8, 0]), "gzip");
        let f = FakeFetch::default().with(&source.urls[0], r#"{"subtitles":"#);
        assert!(matches!(fetch(&f, &source), Err(FetchError::Decode(why)) if !why.starts_with("not JSON")), "broken JSON is the parser's word");
    }

    /// A request with the file's hints that answers HTML or an error status is asked again
    /// without them; a transport failure is not repeated.
    #[test]
    fn a_failed_request_with_hints_falls_back_to_the_bare_one() {
        let hints = FileHints { filename: Some("Film [1080p].mkv".into()), ..FileHints::default() };
        let source = plan(&addons(), "movie", "tt1", &hints).remove(0);
        let ok = r#"{"subtitles":[{"id":"1","url":"https://dl.example/1.srt","lang":"eng"}]}"#;
        let html = FakeFetch::default().with(&source.urls[0], "<html>").with(&source.urls[1], ok);
        assert_eq!(fetch(&html, &source).unwrap().len(), 1);
        assert_eq!(*html.asked.borrow(), source.urls, "the hinted request, then the bare one");
        let status = FakeFetch::default().failing(&source.urls[0], FetchError::Status(500)).with(&source.urls[1], ok);
        assert_eq!(fetch(&status, &source).unwrap().len(), 1);
        let both = FakeFetch::default().with(&source.urls[0], "<html>").with(&source.urls[1], "");
        assert_eq!(fetch(&both, &source), Err(FetchError::Decode("not JSON: empty, 0 bytes".into())), "the last request's failure");
        let down = FakeFetch::default().failing(&source.urls[0], FetchError::Transport("timeout".into())).with(&source.urls[1], ok);
        assert!(matches!(fetch(&down, &source), Err(FetchError::Transport(_))));
        assert_eq!(down.asked.borrow().len(), 1, "the network failed, not the request");
    }

    /// The legacy addon is asked over its JSON-RPC: the query stremio-core builds, base64 in
    /// `q.json?b=`, and the `result.all` list read like a v3 answer.
    #[test]
    fn a_legacy_addon_is_asked_over_json_rpc() {
        let hints = FileHints { filename: Some("Show S01E01.mkv".into()), video_size: Some(42), video_hash: Some("abc".into()) };
        let s = plan(&[legacy()], "series", "tt0903747:1:1", &hints);
        assert_eq!(s.len(), 1, "no idPrefixes: it serves every id");
        assert!(s[0].legacy);
        let decoded = |url: &str| {
            let b = url.strip_prefix("https://legacy.example/stremio/v1/q.json?b=").expect("the legacy endpoint");
            serde_json::from_slice::<serde_json::Value>(&sn_base::b64::decode(b).unwrap()).unwrap()
        };
        assert_eq!(
            decoded(&s[0].urls[0]),
            serde_json::json!({"params":[null,{"query":{"itemHash":"tt0903747 1 1","videoHash":"abc","videoSize":42,"filename":"Show S01E01.mkv"}}],"method":"subtitles.find","id":1,"jsonrpc":"2.0"})
        );
        assert_eq!(decoded(&s[0].urls[1])["params"][1]["query"], serde_json::json!({"itemHash":"tt0903747 1 1"}), "the fallback: the item alone");
        let answer = r#"{"jsonrpc":"2.0","id":1,"result":{"id":"tt0903747 1 1","all":[{"id":"9","url":"https://dl.example/9.srt","lang":"ger"},{"id":"x"}]}}"#;
        let f = FakeFetch::default().with(&s[0].urls[0], answer);
        assert_eq!(fetch(&f, &s[0]).unwrap().iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["9"]);
        let none = FakeFetch::default().with(&s[0].urls[0], r#"{"jsonrpc":"2.0","id":1,"result":{"id":"tt1","all":[]}}"#);
        assert_eq!(fetch(&none, &s[0]), Ok(vec![]), "what the live service answers today: nothing, not a failure");
        let err = FakeFetch::default().with(&s[0].urls[0], r#"{"error":{"code":-32601,"message":"no"}}"#).with(&s[0].urls[1], r#"{"error":{}}"#);
        assert!(matches!(fetch(&err, &s[0]), Err(FetchError::Decode(_))));
    }
}
