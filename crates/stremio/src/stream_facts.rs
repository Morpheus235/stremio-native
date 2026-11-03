//! What a stream row says about one stream, read from the text its addon wrote.
//!
//! Addons describe a stream in free text: Torrentio puts the quality in `name` ("Torrentio\n4k",
//! "[RD+] Torrentio\n4k") and the release, seeders and size in `title`; AIOStreams and Comet put a
//! debrid flag and the resolution in `name` ("[TB⚡] Comet 2160p") and the codec, HDR, audio, size
//! and languages behind emoji the TV's font lacks; MediaFusion writes "MediaFusion | ⚡️ TB 4K" over
//! a "📂 release / 💾 size 👤 seeders / 🌐 languages" description. [`facts`] is THE parser over all
//! of them: one pass over the stream's whole text, the facts in a fixed vocabulary, so the row can
//! draw chips instead of the addon's own run of emoji.
//!
//! [`same_release`] reads the facts the other way: the next episode's stream of the release that
//! is playing (Stremio's `bingeGroup` first, else the release group, resolution, source and codec).

use crate::types::{Playable, Stream};

/// One stream's facts. Every field is `None`/empty when the text does not say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The release's own name: the stream's filename hint, else the first line of its title or
    /// description that says anything (emoji kept: they are the addon's own marks, TvVoo's
    /// "[🛰️] SKY 1"), else its name.
    pub title: String,
    /// The stream's own `name` ("[TB⚡] Comet\n1080p", "Proxy"): its lines, trimmed, as Stremio
    /// shows it beside the title. Empty when the addon sent none.
    pub name: String,
    /// `behaviorHints.isLive`: a live stream.
    pub live: bool,
    /// "4K", "1440p", "1080p", "720p", "576p", "480p".
    pub resolution: Option<&'static str>,
    /// "DV", "HDR10+", "HDR", or Dolby Vision with its fallback layer: "DV HDR10+", "DV HDR"
    /// (a "DV | HDR10" release plays as HDR10 where Dolby Vision cannot).
    pub range: Option<&'static str>,
    /// The release says it was upscaled ("AI", "Upscaled"): its resolution is not native.
    pub upscaled: bool,
    /// "HEVC", "AV1", "H.264", "VP9".
    pub codec: Option<&'static str>,
    /// "TrueHD Atmos 7.1", "DD+ 5.1", "DTS-HD", …
    pub audio: Option<String>,
    /// "REMUX", "BluRay", "WEB-DL", "WEBRip", "HDTV", "HDRip", "DVDRip", or a cinema copy
    /// ([`Facts::cinema_copy`]): "CAM", "TS", "TC", "SCR".
    pub source: Option<&'static str>,
    /// The file size in bytes (the text's "12.3 GB", else the addon's `videoSize` hint).
    pub size: Option<u64>,
    pub seeders: Option<u32>,
    /// Two-letter language codes ("EN", "DE"), in the order the text names them; "MULTI" for a
    /// multi-audio release.
    pub languages: Vec<&'static str>,
    /// The subtitle languages, as codes: what AIOStreams writes after a 📝.
    pub subs: Vec<&'static str>,
    /// The debrid service's cache: `Some(true)` plays at once, `Some(false)` is a torrent the
    /// service first downloads (a placeholder clip plays meanwhile), `None` the text does not say
    /// or the row is not a debrid one ([`Facts::debrid`]).
    pub cached: Option<bool>,
    /// The stream comes through a debrid service: its name marks one ([`Facts::service`]) or
    /// AIOStreams' `streamData.type` is "debrid".
    pub debrid: bool,
    /// The release group ("NIMA4K", "GRP"): AIOStreams' `🏷️` tag, else a scene name's `-GRP`
    /// tail, else an anime release's leading `[Group]`.
    pub group: Option<String>,
}

/// Every resolution word, first match wins.
const RESOLUTIONS: &[(&[&str], &str)] = &[
    (&["2160p", "4k", "uhd"], "4K"),
    (&["1440p", "2k"], "1440p"),
    (&["1080p", "1080i", "fhd"], "1080p"),
    (&["720p", "hd"], "720p"),
    (&["576p"], "576p"),
    (&["480p", "sd"], "480p"),
];
const CODECS: &[(&[&str], &str)] = &[
    (&["hevc", "h265", "h.265", "x265"], "HEVC"),
    (&["av1"], "AV1"),
    (&["avc", "h264", "h.264", "x264"], "H.264"),
    (&["vp9"], "VP9"),
];
const AUDIO: &[(&[&str], &str)] = &[
    (&["truehd"], "TrueHD"),
    (&["dts-hd", "dts-x", "dtsx", "dts-hd-ma"], "DTS-HD"),
    (&["opus"], "Opus"),
    (&["flac"], "FLAC"),
    (&["dd+", "ddp", "eac3", "e-ac3", "ddp5.1", "ddp7.1"], "DD+"),
    (&["dts"], "DTS"),
    (&["dd", "ac3", "dd5.1"], "DD"),
    (&["aac"], "AAC"),
];
const SOURCES: &[(&[&str], &str)] = &[
    (&["remux", "bdremux"], "REMUX"),
    (&["bluray", "blu-ray", "bdrip", "brrip"], "BluRay"),
    (&["web-dl", "webdl"], "WEB-DL"),
    (&["webrip"], "WEBRip"),
    (&["hdtv"], "HDTV"),
    (&["hdrip"], "HDRip"),
    (&["dvdrip"], "DVDRip"),
    // cinema copies ([`Facts::cinema_copy`]); a bare "ts" is a file extension as often
    (&["cam", "camrip", "hdcam"], "CAM"),
    (&["telesync", "hdts", "tsrip"], "TS"),
    (&["telecine", "hdtc"], "TC"),
    (&["screener", "dvdscr", "scr"], "SCR"),
];
/// The sources that are a copy made in or for a cinema, not the release itself.
const CINEMA: [&str; 4] = ["CAM", "TS", "TC", "SCR"];
/// Language words and the code a chip shows.
const LANGUAGES: &[(&str, &str)] = &[
    ("english", "EN"), ("eng", "EN"), ("german", "DE"), ("ger", "DE"), ("deutsch", "DE"), ("french", "FR"),
    ("fre", "FR"), ("spanish", "ES"), ("spa", "ES"), ("latino", "ES"), ("italian", "IT"), ("ita", "IT"),
    ("japanese", "JA"), ("jpn", "JA"), ("portuguese", "PT"), ("russian", "RU"), ("rus", "RU"), ("korean", "KO"),
    ("hindi", "HI"), ("polish", "PL"), ("dutch", "NL"), ("turkish", "TR"), ("multi", "MULTI"),
    ("dual", "MULTI"),
];
/// Bare codes, matched in CAPITALS only ("GER | DE"): lower-case "de" and "es" are words in
/// French and Spanish titles. No "IT": that is a film.
const CODES: &[&str] = &["EN", "DE", "FR", "ES", "RU", "JA", "KO", "PT", "PL", "NL"];
/// Flag emoji (a pair of regional indicators, as Torrentio writes languages) and their language.
const FLAGS: &[(&str, &str)] = &[
    ("GB", "EN"), ("US", "EN"), ("DE", "DE"), ("FR", "FR"), ("ES", "ES"), ("MX", "ES"), ("IT", "IT"), ("JP", "JA"),
    ("PT", "PT"), ("BR", "PT"), ("RU", "RU"), ("KR", "KO"), ("IN", "HI"), ("PL", "PL"), ("NL", "NL"), ("TR", "TR"),
];

/// The facts of `s` (see the module doc).
pub fn facts(s: &Stream) -> Facts {
    let name = s.name.as_deref().unwrap_or("");
    // `title` is the protocol's old name for `description` (Stremio reads it only as a fallback)
    let body = s.description.as_deref().or(s.title.as_deref()).unwrap_or("");
    let file = s.behavior_hints.filename.as_deref().unwrap_or("");
    let text = format!("{name}\n{body}\n{file}");
    let lower = text.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|w| has_word(&lower, w));
    let first = |table: &[(&[&str], &'static str)]| table.iter().find(|(w, _)| has(w)).map(|(_, n)| *n);
    // the name says the quality an addon means ("Torrentio\n4k"); a release's "720p" in a 4K
    // remux's notes must not outvote it. "hd"/"sd" alone count only there.
    let name_lower = name.to_lowercase();
    let resolution = RESOLUTIONS
        .iter()
        .find(|(w, _)| w.iter().any(|w| has_word(&name_lower, w)))
        .or_else(|| RESOLUTIONS.iter().find(|(w, _)| w.iter().any(|w| !matches!(*w, "hd" | "sd") && has_word(&lower, w))))
        .map(|(_, r)| *r);
    let dv = has(&["dv", "dovi", "dolby-vision", "dolbyvision", "dolby vision"]);
    let range = match (dv, has(&["hdr10+", "hdr10plus"]), has(&["hdr", "hdr10", "hlg"])) {
        (true, true, _) => Some("DV HDR10+"),
        (true, false, true) => Some("DV HDR"),
        (true, false, false) => Some("DV"),
        (false, true, _) => Some("HDR10+"),
        (false, false, true) => Some("HDR"),
        (false, false, false) => None,
    };
    // a bare "AI" counts only as an item of a "|" list, as Comet's visual tags write it
    // ("📺 DV | HDR10 | AI"): in a title it is a word or a film
    let upscaled = has(&["upscale", "upscaled", "upscaling"]) || text.split('|').skip(1).any(|item| item.split_whitespace().next() == Some("AI"));
    let mut audio: Vec<&str> = AUDIO.iter().filter(|(w, _)| has(w)).map(|(_, n)| *n).take(1).collect();
    if has(&["atmos"]) {
        audio.push("Atmos");
    }
    let channels = ["7.1", "5.1", "2.0"].into_iter().find(|c| {
        lower.match_indices(c).any(|(at, _)| {
            let before = lower[..at].chars().next_back();
            let after = lower[at + c.len()..].chars().next();
            !before.is_some_and(|b| b.is_ascii_digit()) && !after.is_some_and(|a| a.is_ascii_digit())
        })
    });
    if let Some(ch) = channels {
        audio.push(ch);
    }
    // AIOStreams writes the subtitle languages after a 📝 on the same line ("🌎 German |
    // English📝 English | Greek"): each line up to its 📝 is audio, the rest subtitles
    let audio_text = text.lines().map(|l| l.split('📝').next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let subs_text = text.lines().filter_map(|l| l.split_once('📝').map(|(_, r)| r)).collect::<Vec<_>>().join("\n");
    let languages = language_codes(&audio_text);
    let subs = language_codes(&subs_text).into_iter().filter(|c| *c != "MULTI").collect();
    let title = title(name, body, file);
    // a cache mark (⚡, "cached", "[XX+]") counts only on a debrid row: a plain addon's "⚡" or a
    // "[HD+]" / "[18+]" tag says nothing about a cache
    let debrid = service(name).is_some() || s.stream_data.as_ref().and_then(|d| d.kind.as_deref()) == Some("debrid");
    Facts {
        group: group(&text, &title),
        title,
        name: lines(name).collect::<Vec<_>>().join("\n"),
        live: s.behavior_hints.is_live,
        resolution,
        range,
        upscaled,
        codec: first(CODECS),
        audio: (!audio.is_empty()).then(|| audio.join(" ")),
        source: first(SOURCES),
        size: size(&lower).or(s.behavior_hints.video_size.filter(|b| *b > 0)),
        seeders: seeders(&text),
        languages,
        subs,
        cached: debrid.then(|| cached(&lower)).flatten(),
        debrid,
    }
}

/// Where `w` first stands as a word in `hay`: nothing alphanumeric before it, no letter after it
/// (so "dd" matches "dd5.1" but not "ddp", and "hdr" matches "hdr10" but "hd" not "hdr").
fn first_word_at(hay: &str, w: &str) -> Option<usize> {
    hay.match_indices(w).map(|(at, _)| at).find(|&at| {
        let before = hay[..at].chars().next_back();
        let after = hay[at + w.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphabetic)
    })
}

fn has_word(hay: &str, w: &str) -> bool {
    first_word_at(hay, w).is_some()
}

/// The languages `text` names, as codes, in the order it names them (words, capital codes, flags).
fn language_codes(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    let mut named: Vec<(usize, &'static str)> =
        LANGUAGES.iter().filter_map(|(w, code)| first_word_at(&lower, w).map(|at| (at, *code))).collect();
    named.extend(CODES.iter().filter_map(|c| first_word_at(text, c).map(|at| (at, *c))));
    named.sort_by_key(|(at, _)| *at);
    let mut out: Vec<&'static str> = Vec::new();
    for code in named.into_iter().map(|(_, c)| c).chain(flags(text)) {
        if !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

/// AIOStreams' failure mark: its error streams are named "[❌] <upstream>".
const ERROR_MARK: &str = "[\u{274c}]";

/// **What a pseudo-stream that reports an upstream's failure says**, or `None` for a real stream.
/// AIOStreams sends a failed or timed-out upstream as a stream (`streamData.type` "error", named
/// "[❌] Comet p2p", the cause as its description, a link to its own repository as the URL): it is
/// a message, "Comet p2p: The operation was aborted due to timeout", not a row to play.
pub fn error_note(s: &Stream) -> Option<String> {
    let name = s.name.as_deref().unwrap_or("").trim();
    let typed = s.stream_data.as_ref().and_then(|d| d.kind.as_deref()) == Some("error");
    if !typed && !name.starts_with(ERROR_MARK) {
        return None;
    }
    let who = lines(name.trim_start_matches(ERROR_MARK)).collect::<Vec<_>>().join(" ");
    let why = lines(s.description.as_deref().or(s.title.as_deref()).unwrap_or("")).collect::<Vec<_>>().join(" ");
    Some(match (who.is_empty(), why.is_empty()) {
        (false, false) => format!("{who}: {why}"),
        (true, _) => why,
        (false, true) => who,
    })
}

/// **What an AIOStreams message pseudo-stream says**, or `None` for any other stream:
/// `streamData.type` "statistic" (its optional per-upstream "🟢 [Comet] Scrape Summary", the
/// status, stream count, errors and time as a multi-line, space-aligned description) or "info"
/// (an upstream's message parsed as a stream). Neither has anything to play (AIOStreams gives them
/// its repository as an `externalUrl`). The text is the name, then the description's lines joined
/// with " · " on a second line, each with its alignment runs collapsed:
/// "🟢 [Comet] Scrape Summary\n✔ Status : SUCCESS · 📦 Streams : 12 · …".
pub fn statistic_note(s: &Stream) -> Option<String> {
    let kind = s.stream_data.as_ref().and_then(|d| d.kind.as_deref())?;
    if !matches!(kind, "statistic" | "info") {
        return None;
    }
    let squash = |l: &str| l.split_whitespace().collect::<Vec<_>>().join(" ");
    let who = lines(s.name.as_deref().unwrap_or("")).map(squash).collect::<Vec<_>>().join(" ");
    let what = lines(s.description.as_deref().or(s.title.as_deref()).unwrap_or("")).map(squash).collect::<Vec<_>>().join(" · ");
    let text = [who, what].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n");
    Some(text)
}

/// Whether `s` is a message AIOStreams sent as a stream ([`error_note`], [`statistic_note`]): a
/// note under its addon, never a row and never counted.
pub fn is_note(s: &Stream) -> bool {
    error_note(s).is_some() || statistic_note(s).is_some()
}

/// `s`'s lines, trimmed, without the empty ones and the ones a serializer wrote for nothing
/// ("null", "None", "undefined"): those are no text to show.
fn lines(s: &str) -> impl Iterator<Item = &str> {
    s.lines().map(str::trim).filter(|l| !l.is_empty() && !["none", "null", "undefined"].contains(&l.to_ascii_lowercase().as_str()))
}

/// The release's own name (see [`Facts::title`]).
fn title(name: &str, body: &str, file: &str) -> String {
    if let Some(f) = lines(file).next() {
        return f.to_string();
    }
    // AIOStreams' templates end on a "📁 folder/filename" line: the release, whatever the
    // lines above it say ("🎥 BluRay 🎞️ HEVC 🏷️ GRP" is facts, not a name)
    if let Some(f) = lines(body).find_map(|l| l.strip_prefix('\u{1f4c1}')) {
        let f = f.trim();
        let f = f.rsplit('/').next().unwrap_or(f).trim();
        if !f.is_empty() {
            return f.to_string();
        }
    }
    // the first line that says anything: a run of marks alone ("⚡ ✓") is no name
    lines(body)
        .find(|l| l.chars().any(char::is_alphanumeric))
        .map(str::to_string)
        .unwrap_or_else(|| lines(name).collect::<Vec<_>>().join(" "))
}

impl Facts {
    /// The quality tag under [`Facts::name`] in a stream row's side column: the resolution, unless
    /// the name already says one ("Torrentio\n4k"), else "".
    pub fn side_tag(&self) -> &'static str {
        let named = self.name.to_lowercase();
        let says = RESOLUTIONS.iter().any(|(w, _)| w.iter().any(|w| has_word(&named, w)));
        if says { "" } else { self.resolution.unwrap_or("") }
    }

    /// [`Facts::title`] as a row reads it: a leading mark run dropped ("📄 Film" → "Film"; a
    /// bracketed "[🛰️] SKY 1" stays), and a scene name (one dotted word, "Film.2024.2160p.DDP5.1.H.265-GRP.mkv")
    /// spaced into words without its file extension: "Film 2024 2160p DDP5.1 H.265-GRP". A dot
    /// between digits ("5.1") or in "H.264" stays, so the line wraps between words, not inside one.
    pub fn label(&self) -> String {
        let t = self.title.trim_start_matches(|c: char| c.is_whitespace() || !(c.is_ascii() || c.is_alphanumeric()));
        let t = t
            .rsplit_once('.')
            .filter(|(_, ext)| matches!(ext.to_ascii_lowercase().as_str(), "mkv" | "mp4" | "avi" | "ts" | "m4v" | "webm"))
            .map_or(t, |(stem, _)| stem);
        if t.contains(char::is_whitespace) || !t.contains(['.', '_']) {
            return t.to_string();
        }
        let c: Vec<char> = t.chars().collect();
        c.iter()
            .enumerate()
            .map(|(i, &ch)| {
                // "5.1": one digit each side ("S01E01.2160p" is two words)
                let digit = |j: usize| c.get(j).is_some_and(char::is_ascii_digit);
                let digits = i > 0 && digit(i - 1) && digit(i + 1) && !(i > 1 && digit(i - 2)) && !digit(i + 2);
                let h26x = i > 0 && matches!(c[i - 1], 'h' | 'H') && c.get(i + 1).is_some_and(char::is_ascii_digit);
                match ch {
                    '_' => ' ',
                    '.' if !digits && !h26x => ' ',
                    _ => ch,
                }
            })
            .collect()
    }

    /// A copy made in or for a cinema (CAM, telesync, telecine, screener), not the release itself.
    pub fn cinema_copy(&self) -> bool {
        self.source.is_some_and(|s| CINEMA.contains(&s))
    }

    /// The debrid service the stream's name marks ("[RD+] Torrentio", "[TB⚡] Comet",
    /// "MediaFusion | ⚡️ TB 4K"): its short code, as the addons write it.
    pub fn service(&self) -> Option<&'static str> {
        service(&self.name)
    }

    /// Does the "Only debrid streams" switch keep this stream? A debrid stream stays, cached or
    /// not ([`Facts::debrid`]); one whose text says nothing about debrid does not. A
    /// live stream is exempt: a channel has no debrid to speak of, and the switch must not empty
    /// Live TV.
    pub fn debrid_only_keeps(&self) -> bool {
        self.debrid || self.live
    }
}

/// The debrid service `name` marks: a code as the addons write it ("[RD+]", "[TB⚡]", "| RD 4K")
/// or the service's own name ("TorBox", "Real-Debrid"), as its short code.
fn service(name: &str) -> Option<&'static str> {
    const CODES: [&str; 8] = ["RD", "AD", "PM", "TB", "DL", "ED", "OC", "PKP"];
    const NAMES: [(&str, &str); 10] = [
        ("real-debrid", "RD"),
        ("realdebrid", "RD"),
        ("alldebrid", "AD"),
        ("premiumize", "PM"),
        ("torbox", "TB"),
        ("debrid-link", "DL"),
        ("debridlink", "DL"),
        ("easydebrid", "ED"),
        ("offcloud", "OC"),
        ("pikpak", "PKP"),
    ];
    // codes as written, in capitals: a lowercase "pm" ("pad-Ani pm") is no Premiumize
    let lower = name.to_lowercase();
    CODES.into_iter().find(|c| has_word(name, c)).or_else(|| NAMES.iter().find(|(n, _)| lower.contains(n)).map(|(_, c)| *c))
}

/// The release group (see [`Facts::group`]).
fn group(text: &str, title: &str) -> Option<String> {
    if let Some(at) = text.find('\u{1f3f7}') {
        let tag = text[at + '\u{1f3f7}'.len_utf8()..].trim_start_matches(['\u{fe0f}', ' ']).split_whitespace().next();
        if let Some(tag) = tag.filter(|t| t.chars().any(char::is_alphanumeric)) {
            return Some(tag.to_string());
        }
    }
    let stem = title
        .rsplit_once('.')
        .filter(|(_, ext)| matches!(ext.to_ascii_lowercase().as_str(), "mkv" | "mp4" | "avi" | "ts" | "m4v" | "webm"))
        .map_or(title, |(stem, _)| stem);
    if let Some((_, tail)) = stem.rsplit_once('-') {
        // "WEB-DL", "Blu-ray", "DTS-HD" end on a hyphen too
        let word = !matches!(tail.to_ascii_lowercase().as_str(), "dl" | "ray" | "hd" | "ma" | "ac3" | "rip");
        if word && (2..=24).contains(&tail.len()) && tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Some(tail.to_string());
        }
    }
    stem.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(g, _)| g.trim()).filter(|g| !g.is_empty()).map(str::to_string)
}

/// The size the text states: a number followed by GB/MB/TB (with or without a space, GiB too).
fn size(lower: &str) -> Option<u64> {
    let words: Vec<&str> = lower.split(|c: char| c.is_whitespace() || c == '|' || c == ',').filter(|w| !w.is_empty()).collect();
    words.iter().enumerate().find_map(|(i, w)| {
        let w = w.trim_start_matches(|c: char| !c.is_ascii_digit());
        let num_end = w.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(w.len());
        let (num, unit) = w.split_at(num_end);
        let unit = if unit.is_empty() { words.get(i + 1).copied().unwrap_or("") } else { unit };
        let n: f64 = num.parse().ok()?;
        let scale = match unit.trim_end_matches(|c: char| !c.is_ascii_alphabetic()) {
            "tb" | "tib" => 1u64 << 40,
            "gb" | "gib" => 1u64 << 30,
            "mb" | "mib" => 1u64 << 20,
            _ => return None,
        };
        Some((n * scale as f64) as u64).filter(|b| *b > 0)
    })
}

/// The seeders the text states: the number after a person glyph (👤, 👥) or before/after
/// "seeders"/"seeds".
fn seeders(text: &str) -> Option<u32> {
    let num = |s: &str| s.trim_start_matches([' ', ':', '\u{fe0f}']).split(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse().ok());
    for glyph in ['\u{1f464}', '\u{1f465}'] {
        if let Some(at) = text.find(glyph) {
            if let Some(n) = num(&text[at + glyph.len_utf8()..]) {
                return Some(n);
            }
        }
    }
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| c.is_whitespace() || c == '|').filter(|w| !w.is_empty()).collect();
    words.iter().enumerate().find_map(|(i, w)| {
        let w = w.trim_end_matches(':');
        if w != "seeders" && w != "seeds" {
            return None;
        }
        words.get(i + 1).and_then(|n| num(n)).or_else(|| i.checked_sub(1).and_then(|j| num(words[j])))
    })
}

/// The languages the text's flag emoji name (two regional indicators each).
fn flags(text: &str) -> Vec<&'static str> {
    let ri = |c: char| (0x1f1e6..=0x1f1ff).contains(&(c as u32)).then(|| (b'A' + (c as u32 - 0x1f1e6) as u8) as char);
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < chars.len() {
        if let (Some(a), Some(b)) = (ri(chars[i]), ri(chars[i + 1])) {
            let cc: String = [a, b].iter().collect();
            if let Some((_, code)) = FLAGS.iter().find(|(c, _)| *c == cc) {
                out.push(*code);
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// The debrid cache flag (see [`Facts::cached`]). Uncached is checked first: it is the one that
/// changes what OK does (a download, a placeholder clip), so a text carrying both reads uncached.
///
/// - Torrentio, and AIOStreams' Torrentio-style formatter: `[RD+]` / `[TB+]` cached,
///   `[RD download]` / `[TB download]` not;
/// - AIOStreams' default formatters, Comet and MediaFusion: `⚡` cached, `⏳` (or `⬇️`) not;
/// - the plain words "cached" / "uncached" / "not cached".
fn cached(lower: &str) -> Option<bool> {
    let uncached = lower.contains('\u{23f3}')
        || lower.contains('\u{2b07}')
        || lower.contains(" download]")
        || lower.contains("uncached")
        || lower.contains("not cached");
    if uncached {
        return Some(false);
    }
    let bracket_plus = lower.match_indices("+]").any(|(at, _)| {
        let head = &lower[..at];
        head.rfind('[').is_some_and(|open| {
            let tag = &head[open + 1..];
            (2..=4).contains(&tag.len()) && tag.chars().all(|c| c.is_ascii_alphanumeric())
        })
    });
    (lower.contains('\u{26a1}') || bracket_plus || has_word(lower, "cached")).then_some(true)
}

/// The stream that plays, as the next episode's pick reads it: its addon, its `bingeGroup` and
/// its facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The addon's manifest id.
    pub addon_id: String,
    pub binge_group: Option<String>,
    pub facts: Facts,
}

impl Release {
    pub fn of(addon_id: &str, s: &Stream) -> Self {
        Release { addon_id: addon_id.to_string(), binge_group: s.behavior_hints.binge_group.clone(), facts: facts(s) }
    }
}

/// **The next episode's stream of the release `old`**, among `streams` (the same addon's, in its
/// order): one with the same `bingeGroup` (Stremio's binge rule), or one whose facts match —
/// the same release group, or the same resolution, source and codec, nothing known that differs.
/// A cached stream comes first whatever the match, then the binge group, then the closest facts
/// (matching audio languages a plus), then the addon's order. A stream marked NOT cached is never
/// picked. With `debrid_only` (Settings > Only debrid streams) a stream the switch hides is never
/// picked either. `None` when nothing plausibly is the same release: the viewer picks.
pub fn same_release(old: &Release, streams: &[Stream], debrid_only: bool) -> Option<usize> {
    let o = &old.facts;
    streams
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.playable(), Some(Playable::Http { .. })))
        .filter_map(|(i, s)| {
            let binge = old.binge_group.is_some() && s.behavior_hints.binge_group == old.binge_group;
            let n = facts(s);
            let eq = |a: Option<&str>, b: Option<&str>| a.is_some() && a == b;
            let clash = |a: Option<&str>, b: Option<&str>| a.is_some() && b.is_some() && a != b;
            let group = |f: &Facts| f.group.as_deref().map(str::to_ascii_lowercase);
            let (og, ng) = (group(o), group(&n));
            let same_group = og.is_some() && og == ng;
            let clashes = clash(o.resolution, n.resolution)
                || clash(o.source, n.source)
                || clash(o.codec, n.codec)
                || clash(o.range, n.range)
                || clash(og.as_deref(), ng.as_deref());
            let facts_match = !clashes && (same_group || (eq(o.resolution, n.resolution) && eq(o.source, n.source) && eq(o.codec, n.codec)));
            // an uncached copy would only start a download and play its placeholder clip: the
            // viewer is better served by the list
            if (!binge && !facts_match) || n.cached == Some(false) || (debrid_only && !n.debrid_only_keeps()) {
                return None;
            }
            let score = 4 * u8::from(same_group)
                + 2 * u8::from(eq(o.resolution, n.resolution))
                + u8::from(eq(o.source, n.source))
                + u8::from(eq(o.codec, n.codec))
                + u8::from(eq(o.range, n.range))
                + u8::from(!o.languages.is_empty() && o.languages == n.languages);
            Some(((n.cached == Some(true), binge, score), std::cmp::Reverse(i)))
        })
        .max()
        .map(|(_, std::cmp::Reverse(i))| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StreamHints;

    fn stream(name: &str, title: &str) -> Stream {
        Stream { url: Some("https://example.com/s".into()), name: Some(name.into()), title: Some(title.into()), ..Default::default() }
    }

    /// Comet's rows through TorBox, as Stremio prints them raw.
    #[test]
    fn comet_rows_read_range_layers_upscale_and_subtitles() {
        let comet = |description: &str| Stream {
            url: Some("https://example.com/s".into()),
            name: Some("[TB⚡] Comet\n2160p".into()),
            description: Some(description.into()),
            ..Default::default()
        };
        let f = facts(&comet("🎞️ HEVC 🏷️ DirtyHippie\n📺 DV | HDR10 | AI 🎧 TrueHD | Atmos 🔊 7.1\n📦 24.8 GB (19.9 Mbps) ⏱️ 2h:46m\n🌎 German | English | French 📝 English | German\n📁 Dune Part Two 2024 2160p UHD AI x265-DirtyHippie.mkv"));
        assert_eq!((f.resolution, f.range, f.codec, f.upscaled), (Some("4K"), Some("DV HDR"), Some("HEVC"), true));
        assert_eq!((f.group.as_deref(), f.audio.as_deref()), (Some("DirtyHippie"), Some("TrueHD Atmos 7.1")));
        assert_eq!((f.languages.clone(), f.subs.clone()), (vec!["DE", "EN", "FR"], vec!["EN", "DE"]));
        assert_eq!(f.label(), "Dune Part Two 2024 2160p UHD AI x265-DirtyHippie", "a spaced name loses its extension too");
        assert_eq!((f.cached, f.service()), (Some(true), Some("TB")));
        let f = facts(&comet("🎥 BluRay 🎞️ AV1 🏷️ dAV1nci\n📺 DV | HDR10 🎧 OPUS 🔊 7.1\n🌎 German | English | French\n📁 Dune.Part.Two.2024.2160p.BluRay.AV1-dAV1nci.mkv"));
        assert_eq!((f.range, f.codec, f.source, f.upscaled), (Some("DV HDR"), Some("AV1"), Some("BluRay"), false));
        assert!(f.subs.is_empty());
        let f = facts(&comet("📺 HDR10+\n📁 Ai.no.Kusabi.2012.1080p.WEB-DL.mkv"));
        assert_eq!((f.range, f.upscaled), (Some("HDR10+"), false), "a title's \"Ai\" is not an upscale");
    }

    #[test]
    fn a_cinema_copy_is_named_and_a_title_word_is_not() {
        let f = facts(&stream("Torrentio\n720p", "Film.2026.HDCAM.x264-GRP\n👤 900"));
        assert_eq!(f.source, Some("CAM"));
        assert!(f.cinema_copy());
        let f = facts(&stream("Torrentio\n1080p", "Cam.2018.1080p.WEB-DL.x264-GRP"));
        assert_eq!(f.source, Some("WEB-DL"), "the film \"Cam\": the release's own source wins");
        assert!(!f.cinema_copy());
    }

    #[test]
    fn description_outranks_the_old_title_field() {
        let s = Stream { name: Some("X".into()), title: Some("Old.720p".into()), description: Some("New.2160p".into()), ..Default::default() };
        assert_eq!(facts(&s).title, "New.2160p");
    }

    #[test]
    fn a_torrentio_debrid_row() {
        let f = facts(&stream(
            "[RD+] Torrentio\n4k DV | HDR",
            "Movie.Name.2023.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-GRP\n👤 42 💾 61.3 GB ⚙️ ThePirateBay\nMulti Audio / 🇬🇧 / 🇩🇪",
        ));
        assert_eq!(f.title, "Movie.Name.2023.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-GRP");
        assert_eq!(f.resolution, Some("4K"));
        assert_eq!(f.range, Some("DV HDR"));
        assert_eq!(f.codec, Some("HEVC"));
        assert_eq!(f.audio.as_deref(), Some("TrueHD Atmos 7.1"));
        assert_eq!(f.source, Some("REMUX"));
        assert_eq!(f.size, Some((61.3 * (1u64 << 30) as f64) as u64));
        assert_eq!(f.seeders, Some(42));
        assert_eq!(f.languages, vec!["MULTI", "EN", "DE"]);
        assert_eq!(f.cached, Some(true));
    }

    #[test]
    fn an_aiostreams_default_template_row() {
        // the output of AIOStreams' stock name and description templates
        let s = Stream {
            url: Some("https://example.com/s".into()),
            name: Some("🔑 [TB⚡] AIOStreams 2160p (Best)".into()),
            description: Some(
                "🎥 BluRay REMUX 🎞️ HEVC 🏷️ FGT 🏆 Extended\n📺 DV | HDR10 🎧 Atmos | TrueHD 🔊 7.1\n📦 58.3 GB (62 Mbps) ⏱️ 2h:43m 👥 41 🔍 Torrentio\n🌎 German | English📝 English | Greek\n📁 Film (2024)/Film.2024.2160p.BluRay.REMUX.DV.HEVC.TrueHD.Atmos.7.1-FGT.mkv"
                    .into(),
            ),
            ..Default::default()
        };
        let f = facts(&s);
        assert_eq!(f.title, "Film.2024.2160p.BluRay.REMUX.DV.HEVC.TrueHD.Atmos.7.1-FGT.mkv", "the 📁 line, without its folder");
        assert_eq!(f.label(), "Film 2024 2160p BluRay REMUX DV HEVC TrueHD Atmos 7.1-FGT");
        assert_eq!((f.resolution, f.range, f.codec, f.source), (Some("4K"), Some("DV HDR"), Some("HEVC"), Some("REMUX")));
        assert_eq!(f.audio.as_deref(), Some("TrueHD Atmos 7.1"));
        assert_eq!(f.group.as_deref(), Some("FGT"));
        assert_eq!(f.subs, vec!["EN"], "the 📝 run is the subtitles (Greek has no code here)");
        assert_eq!(f.seeders, Some(41));
        assert_eq!(f.languages, vec!["DE", "EN"], "audio only: the 📝 subtitles are not counted");
        assert_eq!((f.cached, f.service()), (Some(true), Some("TB")));
        let uncached = Stream { name: Some("[RD⏳] AIOStreams 1080p".into()), ..s };
        assert_eq!((facts(&uncached).cached, facts(&uncached).service()), (Some(false), Some("RD")));
    }

    /// Real addons' rows: a debrid row is one whatever its cache mark (the switch keeps it), and a
    /// plain row's "⚡", "[HD+]", "[18+]" or "Cached" title is no cache mark.
    #[test]
    fn debrid_rows_are_told_by_their_service_not_by_a_cache_mark() {
        let row = |name: &str, body: &str, kind: Option<&str>| {
            let mut s = stream(name, body);
            s.stream_data = kind.map(|k| serde_json::from_value(serde_json::json!({ "type": k })).unwrap());
            facts(&s)
        };
        for (name, body, kind, cached) in [
            ("[RD+] Torrentio\n4k", "Film.2024.2160p", None, Some(true)),
            ("[RD download] Torrentio\n4k", "Film.2024.2160p", None, Some(false)),
            ("[TB⚡] Comet 2160p", "📄 Film.2024.2160p.mkv", None, Some(true)),
            ("MediaFusion | RD ⏳ 4K", "Film 2024", None, Some(false)),
            ("MediaFusion | RD 4K", "Film 2024", None, None),
            ("[RD] Debridio 4K", "Film.2024.2160p", None, None),
            ("STREMTHRU [TB] 4K", "Film.2024.2160p", None, None),
            ("TorBox\n4K", "Film.2024.2160p.mkv\nType: Torrent | Cached: Yes", None, Some(true)),
            ("AIOStreams 4K", "Film.2024.2160p", Some("debrid"), None),
        ] {
            let f = row(name, body, kind);
            assert!(f.debrid && f.debrid_only_keeps(), "{name}");
            assert_eq!(f.cached, cached, "{name}");
        }
        for (name, body) in [
            ("Torrentio\n4k", "Film.2024.2160p"),
            ("Easynews+\n1080p", "Film.2024.1080p.WEB"),
            ("⚡ FastHTTP 1080p", "Film 2024"),
            ("[HD+] Kanal", "Film 2024"),
            ("Addon 1080p", "Film [18+] 2024"),
            ("Archive 1080p", "Cached.Memories.2024.1080p"),
            ("PenguPlay 1080p\n1080pHLS pad-Ani pm", "Film 2024"),
        ] {
            let f = row(name, body, Some("http"));
            assert!(!f.debrid && !f.debrid_only_keeps(), "{name}");
            assert_eq!(f.cached, None, "{name}");
        }
    }

    #[test]
    fn a_row_reads_a_spaced_label_and_the_service() {
        let label = |t: &str| Facts { title: t.into(), ..Default::default() }.label();
        assert_eq!(label("Film.2024.2160p.WEB-DL.DDP5.1.Atmos.H.265-GRP.mkv"), "Film 2024 2160p WEB-DL DDP5.1 Atmos H.265-GRP");
        assert_eq!(label("Film_2024_1080p_x264"), "Film 2024 1080p x264");
        assert_eq!(label("Show.S01E01.2160p.TrueHD.7.1"), "Show S01E01 2160p TrueHD 7.1");
        assert_eq!(label("📄 Film 2024 2160p WEB-DL"), "Film 2024 2160p WEB-DL", "a leading mark is dropped");
        assert_eq!(label("[🛰️] SKY 1"), "[🛰️] SKY 1", "a bracketed mark stays");
        assert_eq!(label("Mr. Robot S01E01 720p"), "Mr. Robot S01E01 720p", "spaced text is left alone");
        let service = |n: &str| facts(&stream(n, "x")).service();
        assert_eq!(service("[RD+] Torrentio\n4k"), Some("RD"));
        assert_eq!(service("[TB⚡] Comet 2160p"), Some("TB"));
        assert_eq!(service("MediaFusion | ⚡️ TB 4K"), Some("TB"));
        assert_eq!(service("Torrentio\n720p"), None);
    }

    #[test]
    fn a_torrentio_row_without_debrid_and_an_uncached_one() {
        let plain = facts(&stream("Torrentio\n1080p", "Show.S01E05.1080p.WEB-DL.DDP5.1.H.264-GRP\n👤 7 💾 1.4 GB ⚙️ EZTV"));
        assert_eq!((plain.resolution, plain.codec, plain.source), (Some("1080p"), Some("H.264"), Some("WEB-DL")));
        assert_eq!(plain.audio.as_deref(), Some("DD+ 5.1"));
        assert_eq!(plain.cached, None, "a bare torrent says nothing about a cache");
        let rd = facts(&stream("[RD download] Torrentio\n1080p", "Show.S01E05.1080p.WEB-DL"));
        assert_eq!(rd.cached, Some(false));
        let ger = facts(&stream("Torrentio\n1080p", "Film.2019.German.DL.1080p.BluRay.x264\nGER | ENG"));
        assert_eq!(ger.languages, vec!["DE", "EN"]);
        let codes = facts(&stream("[TB+] AIOStreams 1080p", "Film\nDE · EN · Dual Audio"));
        assert_eq!(codes.languages, vec!["DE", "EN", "MULTI"]);
        assert!(facts(&stream("Torrentio\n1080p", "Le Fabuleux Destin de Amelie")).languages.is_empty(), "a lower-case 'de' is a word");
        let tb = facts(&stream("[TB download] AIOStreams\n4K", "Film.2160p"));
        assert_eq!(tb.cached, Some(false));
        assert_eq!(facts(&stream("[TB+] AIOStreams\n4K", "Film.2160p")).cached, Some(true));
    }

    #[test]
    fn an_aiostreams_row() {
        let mut s = stream(
            "[TB\u{26a1}] Comet 2160p",
            "\u{1f39e}\u{fe0f} AV1 \u{1f3f7}\u{fe0f} UH \u{1f4fa} HDR10 \u{1f3a7} OPUS \u{1f50a} 5.1\n\u{1f4e6} 18.1 GB \u{1f30e} German | English | Italian | French",
        );
        s.behavior_hints = StreamHints { filename: Some("Film.2024.2160p.WEB-DL.AV1.Opus.5.1.mkv".into()), ..Default::default() };
        let f = facts(&s);
        assert_eq!(f.title, "Film.2024.2160p.WEB-DL.AV1.Opus.5.1.mkv", "the filename hint is the release");
        assert_eq!((f.resolution, f.range, f.codec), (Some("4K"), Some("HDR"), Some("AV1")));
        assert_eq!(f.audio.as_deref(), Some("Opus 5.1"));
        assert_eq!(f.languages, vec!["DE", "EN", "IT", "FR"]);
        assert_eq!(f.cached, Some(true));
        let waiting = facts(&stream("[TB\u{23f3}] Comet 1080p", "Film"));
        assert_eq!(waiting.cached, Some(false), "⏳ is AIOStreams' uncached mark");
    }

    #[test]
    fn a_mediafusion_row() {
        let f = facts(&Stream {
            url: Some("https://example.com/s".into()),
            name: Some("MediaFusion | \u{26a1}\u{fe0f} TB 4K".into()),
            description: Some("📂 Film 2023 2160p BluRay x265 DTS-HD\n💾 22.4 GB 👤 120\n🔗 Prowlarr\n🌐 English + Spanish".into()),
            ..Default::default()
        });
        assert_eq!(f.title, "📂 Film 2023 2160p BluRay x265 DTS-HD", "the addon's own mark stays");
        assert_eq!((f.resolution, f.codec, f.source), (Some("4K"), Some("HEVC"), Some("BluRay")));
        assert_eq!(f.audio.as_deref(), Some("DTS-HD"));
        assert_eq!(f.seeders, Some(120));
        assert_eq!(f.languages, vec!["EN", "ES"]);
        assert_eq!(f.cached, Some(true));
        let slow = facts(&stream("MediaFusion | \u{23f3} TB 1080p", "Film"));
        assert_eq!(slow.cached, Some(false));
    }

    #[test]
    fn a_plain_http_row_says_little_and_the_hint_sizes_it() {
        let mut s = stream("Example", "Watch now");
        s.behavior_hints.video_size = Some(2_000_000_000);
        let f = facts(&s);
        assert_eq!(f.title, "Watch now");
        assert_eq!((f.resolution, f.range, f.codec, f.source, f.cached), (None, None, None, None, None));
        assert_eq!(f.size, Some(2_000_000_000));
        assert!(f.languages.is_empty());
        let bare = facts(&Stream { name: Some("Only\nName".into()), ..Default::default() });
        assert_eq!(bare.title, "Only Name");
    }

    #[test]
    fn an_aiostreams_error_stream_is_a_note_and_a_real_one_is_not() {
        let parse = |j: &str| serde_json::from_str::<Stream>(j).unwrap();
        let typed = parse(r#"{"name":"[❌] Comet p2p","description":"The operation was aborted due to timeout","externalUrl":"https://example.com/repo","streamData":{"type":"error","error":{"title":"[❌] Comet p2p"}}}"#);
        assert_eq!(error_note(&typed).as_deref(), Some("Comet p2p: The operation was aborted due to timeout"));
        let marked = stream("[❌] Comet p2p", "No results");
        assert_eq!(error_note(&marked).as_deref(), Some("Comet p2p: No results"), "the mark alone, without streamData");
        assert_eq!(error_note(&stream("[TB⚡] Comet\n1080p", "Film")), None);
        assert_eq!(error_note(&stream("Torrentio ❌ 4k", "Film")), None, "only the leading mark");
    }

    /// The shape AIOStreams' `toStatisticStream` sends (`streams/fetcher.ts`'s Scrape Summary).
    const SCRAPE_SUMMARY: &str = r#"{"name":"🟠 [Comet] Scrape Summary","description":"✔ Status      : PARTIAL SUCCESS\n📦 Streams    : 12\n📋 Details    : Fetched streams with errors:\n    • [❌] Comet p2p: No results\n⏱️ Time       : 3.2s\n","externalUrl":"https://example.com/repo","streamData":{"type":"statistic"}}"#;

    #[test]
    fn an_aiostreams_statistic_stream_is_a_two_part_note() {
        let parse = |j: &str| serde_json::from_str::<Stream>(j).unwrap();
        let stat = parse(SCRAPE_SUMMARY);
        assert_eq!(
            statistic_note(&stat).as_deref(),
            Some("🟠 [Comet] Scrape Summary\n✔ Status : PARTIAL SUCCESS · 📦 Streams : 12 · 📋 Details : Fetched streams with errors: · • [❌] Comet p2p: No results · ⏱️ Time : 3.2s")
        );
        assert_eq!(error_note(&stat), None, "a summary is no failure, even when it lists one");
        assert!(is_note(&stat));
        let info = parse(r#"{"name":"Example","description":"No results for this title","streamData":{"type":"info"}}"#);
        assert_eq!(statistic_note(&info).as_deref(), Some("Example\nNo results for this title"));
        let real = parse(r#"{"name":"[TB⚡] Comet","description":"Film 1080p","url":"https://example.com/v.mkv","streamData":{"type":"debrid"}}"#);
        assert_eq!(statistic_note(&real), None);
        assert!(!is_note(&real));
        assert_eq!(statistic_note(&stream("🟢 [Comet] Scrape Summary", "x")), None, "only the type marks one");
    }

    #[test]
    fn a_name_lists_its_lines_and_a_placeholder_says_nothing() {
        let f = facts(&stream("  [TB⚡] Comet \n\n 1080p ", "null\n⚡ ✓\n[🛰️] SKY 1"));
        assert_eq!(f.name, "[TB⚡] Comet\n1080p");
        assert_eq!(f.title, "[🛰️] SKY 1", "a placeholder and a run of marks are no title; the emoji stay");
        assert_eq!(f.side_tag(), "", "the name says its quality");
        assert_eq!(facts(&stream("None", "Film.720p")).name, "");
        assert_eq!(facts(&stream("Proxy", "Film.720p")).side_tag(), "720p");
    }

    #[test]
    fn subtitle_languages_after_the_memo_mark_are_not_audio() {
        let f = facts(&stream(
            "[TB⚡] Comet 1080p",
            "🎞️ HEVC 🏷️ GRP\n🎧 DD 🔊 5.1\n🌎 Spanish | Italian📝 English | German\n📁 Show.S01E01.1080p.mkv",
        ));
        assert_eq!(f.languages, vec!["ES", "IT"]);
        assert_eq!(f.cached, Some(true));
    }


    /// One of the viewer's AIOStreams rows (Comet over TorBox), with its binge group and file.
    fn aio(name: &str, binge: &str, desc: &str, file: &str) -> Stream {
        let mut s = stream(name, desc);
        s.behavior_hints = StreamHints { binge_group: Some(binge.into()), filename: Some(file.into()), ..Default::default() };
        s
    }

    const IDN: &str = "com.aiostreams.viren070|1080p|iDN_CreW";

    fn e1() -> Release {
        Release::of("aiostreams", &aio("[TB⚡] Comet 1080p", IDN, "🎥 WEB-DL 🎞️ HEVC 🏷️ iDN_CreW\n📺 SDR\n🌎 Italian | English | Spanish", "Breaking.Bad.1x01.Pilot.1080p.H265.ITA.ENG.SPA.AC3.5.1-iDN_CreW.mkv"))
    }

    #[test]
    fn the_release_group_is_read_from_the_tag_the_scene_name_or_the_anime_bracket() {
        assert_eq!(e1().facts.group.as_deref(), Some("iDN_CreW"));
        let g = |title: &str| facts(&stream("Torrentio\n1080p", title)).group;
        assert_eq!(g("Show.S01E05.1080p.WEB-DL.DDP5.1.H.264-GRP\n👤 7"), Some("GRP".into()));
        assert_eq!(g("Show.S01E05.1080p.WEB-DL"), None, "WEB-DL's hyphen is no group");
        assert_eq!(g("[SubsPlease] Show - 01 (1080p).mkv"), Some("SubsPlease".into()));
    }

    #[test]
    fn the_next_episode_keeps_the_binge_group() {
        let streams = [
            aio("[TB⏳] Comet 2160p", "com.aiostreams.viren070|2160p|WEB-DL|HDSource", "🎥 WEB-DL 🎞️ HEVC 🏷️ HDSource", "Breaking.Bad.S01E02.2160p.WEB-DL.mkv"),
            aio("[TB⏳] Comet 2160p", "com.aiostreams.viren070|2160p|WEB-DL|NIMA4K", "🎥 WEB-DL 🎞️ HEVC 🏷️ NIMA4K\n📺 SDR\n📦 5.78 GB\n🌎 German | English📝 English | German", "Breaking.Bad.S01E02.2160p.WEB-DL.mkv"),
            aio("[TB⚡] Comet 1080p", IDN, "🎥 WEB-DL 🎞️ HEVC", "Breaking.Bad.1x02.Cat.s.in.the.Bag.1080p.H265.mkv"),
        ];
        assert_eq!(same_release(&e1(), &streams, false), Some(2));
    }

    #[test]
    fn without_a_binge_group_the_facts_choose() {
        let old = Release::of("torrentio", &stream("[RD+] Torrentio\n1080p", "Show.S01E01.1080p.WEB-DL.H264-GRP\n👤 7"));
        let streams = [
            stream("[RD+] Torrentio\n4k", "Show.S01E02.2160p.WEB-DL.H264-GRP"),
            stream("[RD+] Torrentio\n1080p", "Show.S01E02.1080p.WEB-DL.H264-OTHER"),
            stream("[RD+] Torrentio\n1080p", "Show.S01E02.1080p.WEB-DL.H264-GRP"),
        ];
        assert_eq!(same_release(&old, &streams, false), Some(2), "same resolution and group; another group's 1080p is another release");
        // nothing else names a group: the same resolution, source and codec is the release
        let streams = [stream("[RD+] Torrentio\n720p", "Show.S01E02.720p.WEB-DL.H264"), stream("[RD+] Torrentio\n1080p", "Show.S01E02.1080p.WEB-DL.H264")];
        assert_eq!(same_release(&old, &streams[..1], false), None, "a 720p is not the release");
        assert_eq!(same_release(&old, &streams, false), Some(1));
    }

    #[test]
    fn a_cached_match_comes_first_and_no_match_is_none() {
        let streams = [
            aio("[TB⏳] Comet 1080p", IDN, "🏷️ iDN_CreW", "Breaking.Bad.1x02.1080p.H265-iDN_CreW.mkv"),
            aio("[RD⚡] Comet 1080p", "com.aiostreams.viren070|1080p|iDN_CreW|rd", "🏷️ iDN_CreW", "Breaking.Bad.1x02.1080p.H265-iDN_CreW.mkv"),
        ];
        assert_eq!(same_release(&e1(), &streams, false), Some(1), "the cached copy of the release, over the waiting binge match");
        let other = [aio("[TB⚡] Comet 2160p", "com.aiostreams.viren070|2160p|WEB-DL|NIMA4K", "🏷️ NIMA4K", "Breaking.Bad.S01E02.2160p.WEB-DL.mkv")];
        assert_eq!(same_release(&e1(), &other, false), None, "a cached stream of another release is never picked");
        let waiting_only = [aio("[TB⏳] Comet 1080p", IDN, "🏷️ iDN_CreW", "Breaking.Bad.1x02.1080p.H265-iDN_CreW.mkv")];
        assert_eq!(same_release(&e1(), &waiting_only, false), None, "an uncached match would only play the download placeholder");
        let unplayable = Stream { info_hash: Some("ab".into()), ..aio("[TB⚡] Comet 1080p", IDN, "", "x.mkv") };
        assert_eq!(same_release(&e1(), &[Stream { url: None, ..unplayable }], false), None, "only an HTTP stream plays");
    }

    /// "Only debrid streams": a debrid stream, cached or not, is kept; one whose text says nothing
    /// about debrid is not, unless it is live. The next episode's pick follows the same switch.
    #[test]
    fn only_debrid_keeps_debrid_streams_and_the_next_episode_pick_obeys_it() {
        let keeps = |name: &str, desc: &str| facts(&stream(name, desc)).debrid_only_keeps();
        assert!(keeps("[RD+] Torrentio\n4k", "Film.2160p"), "cached");
        assert!(keeps("[TB download] AIOStreams\n4K", "Film.2160p"), "uncached debrid, which the service downloads first");
        assert!(!keeps("Torrentio\n1080p", "Film.1080p\n\u{1f464} 12"), "the text says nothing about debrid");
        let mut live = stream("Vavoo", "[SKY 1]");
        live.behavior_hints.is_live = true;
        assert!(facts(&live).debrid_only_keeps(), "a live stream is exempt");

        // a plain match of the release is picked while the switch is off, never while it is on
        let plain = [aio("Comet 1080p", IDN, "\u{1f3a5} WEB-DL \u{1f39e}\u{fe0f} HEVC", "Breaking.Bad.1x02.1080p.H265.mkv")];
        assert_eq!(facts(&plain[0]).cached, None, "the premise: nothing says debrid");
        assert_eq!(same_release(&e1(), &plain, false), Some(0));
        assert_eq!(same_release(&e1(), &plain, true), None);
        // a cached one still wins, and is still picked, with the switch on
        let mixed = [plain[0].clone(), aio("[TB\u{26a1}] Comet 1080p", IDN, "\u{1f3a5} WEB-DL", "Breaking.Bad.1x02.1080p.H265.mkv")];
        assert_eq!(same_release(&e1(), &mixed, true), Some(1));
    }
}
