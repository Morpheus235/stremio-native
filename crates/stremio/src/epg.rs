//! XMLTV programme guides: what is on a live channel now and next.
//!
//! A guide is one XMLTV document (`<tv>` with `<channel>` and `<programme>` elements), usually
//! gzipped. A national guide is ~20 MB of XML for a week, so [`parse`] streams it through a small
//! pull tokenizer and keeps only the title, sub-title and times of the programmes that overlap the
//! window it is given; descriptions, credits and icons are skipped as they stream past.
//!
//! **Channels are matched by name.** An addon's channel carries no XMLTV id, so both sides are
//! reduced by [`channel_key`]: lowercased, `+` read as "plus", punctuation and spaces dropped, and
//! quality tags (HD, UHD, 4K…) removed. "Das Erste HD", "DAS ERSTE" and the guide's
//! `Das.Erste.de` / "Das Erste" all key as `daserste`.

use std::collections::HashMap;
use std::io::Read;

/// One programme, in Unix seconds (UTC).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Programme {
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub subtitle: Option<String>,
}

/// What a channel shows at an instant: the programme on air (if any) and the one after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NowNext<'a> {
    pub now: Option<&'a Programme>,
    pub next: Option<&'a Programme>,
}

/// A parsed guide: each channel's programmes, sorted by start, reachable by [`channel_key`].
/// Serializable, so the application can keep the parsed window instead of the document.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Guide {
    by_key: HashMap<String, usize>,
    lists: Vec<Vec<Programme>>,
}

impl Guide {
    /// How many channels have at least one programme in the window.
    pub fn channels(&self) -> usize {
        self.lists.iter().filter(|l| !l.is_empty()).count()
    }

    /// What `channel` (an addon's channel name) shows at `now`. `None` when the guide does not
    /// know the channel or has nothing for it from `now` on.
    pub fn now_next(&self, channel: &str, now: i64) -> Option<NowNext<'_>> {
        let list = &self.lists[self.find(&channel_key(channel))?];
        // The first programme starting after `now`; the one before it is on air if it has not ended.
        let after = list.partition_point(|p| p.start <= now);
        let now_p = after.checked_sub(1).map(|i| &list[i]).filter(|p| p.stop > now);
        let next = list.get(after);
        (now_p.is_some() || next.is_some()).then_some(NowNext { now: now_p, next })
    }
}

impl Guide {
    /// The list for `key`: exact, else the ONE guide key that differs from it only by a trailing
    /// letter or two ("Sky Cinema Premieren" is the guide's "Sky Cinema Premiere"). Digits never
    /// stretch a match, so "Sky Sport 11" is not "Sky Sport 1".
    fn find(&self, key: &str) -> Option<usize> {
        if let Some(&i) = self.by_key.get(key) {
            return Some(i);
        }
        let near = |long: &str, short: &str| {
            short.len() >= 4
                && long.strip_prefix(short).is_some_and(|rest| (1..=2).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphabetic()))
        };
        let mut hits = self.by_key.iter().filter(|(k, _)| near(key, k) || near(k, key)).map(|(_, &i)| i);
        let first = hits.next()?;
        hits.all(|i| i == first).then_some(first)
    }
}

/// The name both an addon's channel and a guide's channel are matched by.
pub fn channel_key(name: &str) -> String {
    const NOISE: &[&str] = &["hd", "uhd", "fhd", "fullhd", "sd", "4k", "8k", "hevc", "h265", "raw"];
    // `+` becomes its own "plus" word so "SPORT1+" never collides with "SPORT1".
    let lower = name.to_lowercase().replace('+', " plus ");
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !NOISE.contains(w))
        .collect()
}

/// A guide channel id such as `Das.Erste.de`: the trailing two-letter country segment dropped.
fn id_name(id: &str) -> &str {
    match id.rsplit_once('.') {
        Some((head, cc)) if cc.len() == 2 && cc.chars().all(|c| c.is_ascii_alphabetic()) => head,
        _ => id,
    }
}

/// The most of a document read (decompressed): a week of a national guide is ~25 MB, so this only
/// stops a corrupt or hostile gzip from inflating without end. A longer document is cut there.
const DOC_MAX: u64 = 256 << 20;
/// The most kept of one title, sub-title or channel name (the rest is dropped).
const FIELD_MAX: usize = 256;
/// The most programmes kept in all, and display names per channel: well past a real guide's
/// window (~10k programmes, 1–3 names), so memory stays bounded whatever a document holds.
const PROGRAMMES_MAX: usize = 200_000;
const NAMES_MAX: usize = 8;
const CHANNELS_MAX: usize = 20_000;

/// Parse an XMLTV document (plain or gzipped), keeping the programmes that overlap `from..to`
/// (Unix seconds).
pub fn parse(body: &[u8], from: i64, to: i64) -> Result<Guide, String> {
    if body.starts_with(&[0x1f, 0x8b]) {
        parse_reader(flate2::read::GzDecoder::new(body).take(DOC_MAX), from, to)
    } else {
        parse_reader(body, from, to)
    }
}

#[derive(Default)]
struct Pending {
    channel: String,
    start: Option<i64>,
    stop: Option<i64>,
    title: Option<String>,
    subtitle: Option<String>,
}

fn parse_reader(r: impl Read, from: i64, to: i64) -> Result<Guide, String> {
    let mut tok = Tokenizer::new(r);
    // channel id -> its display names; channel id -> its programmes
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    let mut progs: HashMap<String, Vec<Programme>> = HashMap::new();
    let mut channel: Option<String> = None; // inside <channel id=…>
    let mut prog: Option<Pending> = None; // inside <programme …>
    let mut text_for: Option<&'static str> = None; // which child's text is being collected
    let mut text = String::new();
    let mut saw_tv = false;
    let mut kept = 0usize;

    while let Some(ev) = tok.next()? {
        match ev {
            Event::Start { name, attrs, empty } => {
                match name.as_str() {
                    "tv" => saw_tv = true,
                    "channel" if !empty => channel = attr(&attrs, "id"),
                    "programme" if !empty => {
                        prog = Some(Pending {
                            channel: attr(&attrs, "channel").unwrap_or_default(),
                            start: attr(&attrs, "start").and_then(|s| xmltv_time(&s)),
                            stop: attr(&attrs, "stop").and_then(|s| xmltv_time(&s)),
                            ..Pending::default()
                        })
                    }
                    "display-name" if channel.is_some() && !empty => text_for = Some("display-name"),
                    "title" if prog.is_some() && !empty => text_for = Some("title"),
                    "sub-title" if prog.is_some() && !empty => text_for = Some("sub-title"),
                    _ => {}
                }
                text.clear();
            }
            Event::Text(t) => {
                if text_for.is_some() && text.len() < FIELD_MAX {
                    text.push_str(&t);
                }
            }
            Event::End(name) => match name.as_str() {
                "display-name" | "title" | "sub-title" if text_for == Some(leak(&name)) => {
                    let value = clip(text.trim(), FIELD_MAX).to_string();
                    text_for = None;
                    if value.is_empty() {
                        continue;
                    }
                    match name.as_str() {
                        "display-name" => {
                            if let Some(id) = &channel {
                                if names.len() < CHANNELS_MAX || names.contains_key(id) {
                                    let list = names.entry(id.clone()).or_default();
                                    if list.len() < NAMES_MAX {
                                        list.push(value);
                                    }
                                }
                            }
                        }
                        "title" => {
                            if let Some(p) = prog.as_mut() {
                                p.title.get_or_insert(value); // the first title (the guide's own language)
                            }
                        }
                        _ => {
                            if let Some(p) = prog.as_mut() {
                                p.subtitle.get_or_insert(value);
                            }
                        }
                    }
                }
                "channel" => channel = None,
                "programme" => {
                    if let Some(p) = prog.take() {
                        if let (Some(start), Some(stop), Some(title)) = (p.start, p.stop, p.title) {
                            if stop > from && start < to && stop > start && kept < PROGRAMMES_MAX {
                                kept += 1;
                                progs.entry(p.channel).or_default().push(Programme { start, stop, title, subtitle: p.subtitle });
                            }
                        }
                    }
                }
                _ => {}
            },
        }
    }
    if !saw_tv {
        return Err("not an XMLTV document".into());
    }

    let mut guide = Guide::default();
    for (id, mut list) in progs {
        list.sort_by_key(|p| p.start);
        list.dedup_by(|b, a| a.start == b.start); // two sources merged into one guide repeat slots
        let idx = guide.lists.len();
        guide.lists.push(list);
        // The id's own name first; a display name never takes a key another channel already has.
        let keys = std::iter::once(id_name(&id).to_string()).chain(names.remove(&id).unwrap_or_default());
        for k in keys.map(|n| channel_key(&n)).filter(|k| !k.is_empty()) {
            guide.by_key.entry(k).or_insert(idx);
        }
    }
    Ok(guide)
}

/// `s` cut to at most `max` bytes, on a character boundary.
fn clip(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The static name of a collected child element (so `text_for` can stay `&'static str`).
fn leak(name: &str) -> &'static str {
    match name {
        "display-name" => "display-name",
        "title" => "title",
        _ => "sub-title",
    }
}

/// `20261024030000 +0000` (seconds and the offset optional) as Unix seconds.
fn xmltv_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let (stamp, zone) = s.split_once(' ').map_or((s, ""), |(a, b)| (a, b.trim()));
    let num = |r: std::ops::Range<usize>| stamp.get(r)?.parse::<i64>().ok();
    if stamp.len() < 12 || !stamp.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (y, mo, d, h, mi) = (num(0..4)?, num(4..6)?, num(6..8)?, num(8..10)?, num(10..12)?);
    let sec = if stamp.len() >= 14 { num(12..14)? } else { 0 };
    let offset = match zone.as_bytes() {
        [sign @ (b'+' | b'-'), ..] if zone.len() == 5 && zone[1..].bytes().all(|b| b.is_ascii_digit()) => {
            let m = zone[1..3].parse::<i64>().ok()? * 60 + zone[3..5].parse::<i64>().ok()?;
            if *sign == b'-' { -m * 60 } else { m * 60 }
        }
        _ => 0,
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn attr(attrs: &str, key: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(eq) = rest.find('=') {
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let end = after[1..].find(quote)? + 1;
        if name == key {
            return Some(unescape(&after[1..end]));
        }
        rest = &after[end + 1..];
    }
    None
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|&i| i <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

enum Event {
    Start { name: String, attrs: String, empty: bool },
    End(String),
    Text(String),
}

/// A pull tokenizer over a byte stream: start/end tags and text, with comments, processing
/// instructions, doctypes and CDATA handled. Holds at most one unfinished token in memory.
struct Tokenizer<R> {
    r: R,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
}

/// Longest single token (tag or text run) the tokenizer will hold; a guide's longest is a few KB
/// of description, so this only stops a corrupt document from growing the buffer without bound.
const TOKEN_MAX: usize = 1 << 20;

impl<R: Read> Tokenizer<R> {
    fn new(r: R) -> Self {
        Tokenizer { r, buf: Vec::with_capacity(64 * 1024), pos: 0, eof: false }
    }

    /// Read more input; `false` at end of stream.
    fn fill(&mut self) -> Result<bool, String> {
        if self.eof {
            return Ok(false);
        }
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        if self.buf.len() > TOKEN_MAX {
            return Err("XMLTV token too long".into());
        }
        let mut chunk = [0u8; 64 * 1024];
        let n = self.r.read(&mut chunk).map_err(|e| format!("reading the guide: {e}"))?;
        if n == 0 {
            self.eof = true;
            return Ok(false);
        }
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(true)
    }

    /// Find `needle` at or after `from` (relative to `pos`), reading more as needed.
    fn find(&mut self, from: usize, needle: &[u8]) -> Result<Option<usize>, String> {
        let mut scanned = from;
        loop {
            let hay = &self.buf[self.pos..];
            if let Some(i) = hay.get(scanned..).and_then(|h| h.windows(needle.len()).position(|w| w == needle)) {
                return Ok(Some(scanned + i));
            }
            scanned = hay.len().saturating_sub(needle.len() - 1).max(from);
            if !self.fill()? {
                return Ok(None);
            }
        }
    }

    fn next(&mut self) -> Result<Option<Event>, String> {
        loop {
            if self.pos >= self.buf.len() && !self.fill()? {
                return Ok(None);
            }
            if self.buf[self.pos] != b'<' {
                // Text up to the next tag (or the end of the document).
                let end = self.find(0, b"<")?.unwrap_or(self.buf.len() - self.pos);
                let raw = String::from_utf8_lossy(&self.buf[self.pos..self.pos + end]).into_owned();
                self.pos += end;
                return Ok(Some(Event::Text(unescape(&raw))));
            }
            // Make sure enough is buffered to recognise the markup kind.
            while self.buf.len() - self.pos < 9 && self.fill()? {}
            let head = &self.buf[self.pos..];
            if head.starts_with(b"<!--") {
                let end = self.find(4, b"-->")?.ok_or("unterminated comment")?;
                self.pos += end + 3;
                continue;
            }
            if head.starts_with(b"<![CDATA[") {
                let end = self.find(9, b"]]>")?.ok_or("unterminated CDATA")?;
                let raw = String::from_utf8_lossy(&self.buf[self.pos + 9..self.pos + end]).into_owned();
                self.pos += end + 3;
                return Ok(Some(Event::Text(raw)));
            }
            if head.starts_with(b"<?") || head.starts_with(b"<!") {
                let end = self.find(2, b">")?.ok_or("unterminated declaration")?;
                self.pos += end + 1;
                continue;
            }
            let end = self.tag_end()?.ok_or("unterminated tag")?;
            let inner = String::from_utf8_lossy(&self.buf[self.pos + 1..self.pos + end]).into_owned();
            self.pos += end + 1;
            if let Some(name) = inner.strip_prefix('/') {
                return Ok(Some(Event::End(name.trim().to_string())));
            }
            let (body, empty) = match inner.strip_suffix('/') {
                Some(b) => (b, true),
                None => (inner.as_str(), false),
            };
            let split = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
            return Ok(Some(Event::Start { name: body[..split].to_string(), attrs: body[split..].to_string(), empty }));
        }
    }

    /// The offset of the `>` closing the tag at `pos`, skipping any `>` inside quoted attributes.
    fn tag_end(&mut self) -> Result<Option<usize>, String> {
        let mut i = 1;
        let mut quote = 0u8;
        loop {
            while self.pos + i < self.buf.len() {
                let b = self.buf[self.pos + i];
                match (quote, b) {
                    (0, b'"' | b'\'') => quote = b,
                    (0, b'>') => return Ok(Some(i)),
                    (q, b) if q == b => quote = 0,
                    _ => {}
                }
                i += 1;
            }
            if !self.fill()? {
                return Ok(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="x">
  <channel id="Das.Erste.de"><display-name lang="de">Das Erste</display-name></channel>
  <channel id="SPORT1+.de"><display-name>SPORT1+</display-name></channel>
  <channel id="SPORT1.de"><display-name>SPORT1</display-name></channel>
  <!-- a comment with <programme> inside -->
  <programme start="20261025120000 +0000" stop="20261025130000 +0000" channel="Das.Erste.de">
    <title lang="de">Tagesschau &amp; Wetter</title>
    <sub-title lang="de"><![CDATA[Folge <1>]]></sub-title>
    <desc lang="de">A long text &gt; with markup-like bits</desc>
    <icon src="http://x/y.jpg" />
  </programme>
  <programme start="20261025130000 +0000" stop="20261025140000 +0000" channel="Das.Erste.de">
    <title>Mittagsmagazin</title>
  </programme>
  <programme start="20261025150000 +0200" stop="20261025160000 +0200" channel="SPORT1+.de">
    <title>Darts</title>
  </programme>
  <programme start="20261025100000 +0000" stop="20261025110000 +0000" channel="SPORT1.de">
    <title>Old news</title>
  </programme>
</tv>"#;

    fn t(s: &str) -> i64 {
        xmltv_time(s).unwrap()
    }

    #[test]
    fn times_are_utc_seconds_with_their_offset_applied() {
        assert_eq!(t("19700101000000 +0000"), 0);
        assert_eq!(t("20261025120000 +0000"), 1_792_929_600);
        assert_eq!(t("20261025140000 +0200"), t("20261025120000 +0000"));
        assert_eq!(t("20261025100000 -0130"), t("20261025113000 +0000"));
        assert_eq!(t("202610251200"), t("20261025120000 +0000"), "seconds and the offset are optional");
        assert_eq!(xmltv_time("2026-10-25"), None);
    }

    #[test]
    fn names_key_alike_across_quality_tags_case_and_punctuation() {
        assert_eq!(channel_key("Das Erste HD"), "daserste");
        assert_eq!(channel_key("DAS ERSTE"), "daserste");
        assert_eq!(channel_key(id_name("Das.Erste.de")), "daserste");
        assert_eq!(channel_key("SKY CINEMA SPECIAL HD"), channel_key("Sky Cinema Special"));
        assert_eq!(channel_key("Servus TV"), channel_key("ServusTV"));
        assert_eq!(channel_key("SPORT 1+"), "sport1plus");
        assert_ne!(channel_key("SPORT1+"), channel_key("SPORT1"), "the + channel is another channel");
        assert_eq!(id_name("SWR/SR.de"), "SWR/SR");
        assert_eq!(id_name("RTL"), "RTL");
    }

    #[test]
    fn a_guide_streams_into_now_and_next_per_channel() {
        let from = t("20261025110000 +0000");
        let g = parse(DOC.as_bytes(), from, from + 86_400).unwrap();
        assert_eq!(g.channels(), 2, "SPORT1's only programme ended before the window");

        let nn = g.now_next("Das Erste HD", t("20261025123000 +0000")).unwrap();
        let now = nn.now.unwrap();
        assert_eq!(now.title, "Tagesschau & Wetter", "entities decoded");
        assert_eq!(now.subtitle.as_deref(), Some("Folge <1>"), "CDATA kept verbatim");
        assert_eq!(nn.next.unwrap().title, "Mittagsmagazin");

        // between two programmes' boundary: the later one is on air
        let nn = g.now_next("das erste", t("20261025130000 +0000")).unwrap();
        assert_eq!(nn.now.unwrap().title, "Mittagsmagazin");
        assert_eq!(nn.next, None);

        // before anything starts: nothing on air, the first programme next
        let nn = g.now_next("SPORT 1+ HD", t("20261025120000 +0000")).unwrap();
        assert_eq!(nn.now, None);
        assert_eq!(nn.next.unwrap().title, "Darts");

        assert_eq!(g.now_next("Das Erste", t("20261025150000 +0000")), None, "all over");
        assert_eq!(g.now_next("Unknown TV", from), None);
        assert_eq!(g.now_next("Das Ersten", t("20261025123000 +0000")).unwrap().now.unwrap().title, "Tagesschau & Wetter", "a trailing letter or two still matches");
        assert_eq!(g.now_next("SPORT1 4", from), None, "a trailing digit never does");
    }

    #[test]
    fn a_gzipped_guide_reads_the_same_and_tokens_split_across_reads_still_parse() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(DOC.as_bytes()).unwrap();
        let body = gz.finish().unwrap();
        let from = t("20261025110000 +0000");
        let g = parse(&body, from, from + 86_400).unwrap();
        assert_eq!(g.now_next("Das Erste", t("20261025123000 +0000")).unwrap().now.unwrap().title, "Tagesschau & Wetter");

        // One byte per read: every token straddles a refill.
        struct Trickle<'a>(&'a [u8]);
        impl Read for Trickle<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let Some((&b, rest)) = self.0.split_first() else { return Ok(0) };
                out[0] = b;
                self.0 = rest;
                Ok(1)
            }
        }
        let g = parse_reader(Trickle(DOC.as_bytes()), from, from + 86_400).unwrap();
        assert_eq!(g.now_next("Das Erste", t("20261025123000 +0000")).unwrap().next.unwrap().title, "Mittagsmagazin");
    }

    #[test]
    fn a_huge_title_is_clipped_and_a_channel_keeps_only_a_few_names() {
        let long = "Ä".repeat(10_000);
        let names: String = (0..50).map(|i| format!("<display-name>Name {i}</display-name>")).collect();
        let doc = format!(
            r#"<tv><channel id="X.de">{names}</channel><programme start="20261025120000 +0000" stop="20261025130000 +0000" channel="X.de"><title>{long}</title></programme></tv>"#
        );
        let from = t("20261025110000 +0000");
        let g = parse(doc.as_bytes(), from, from + 86_400).unwrap();
        let title = &g.now_next("X", t("20261025123000 +0000")).unwrap().now.unwrap().title;
        assert!(title.len() <= FIELD_MAX && title.starts_with('Ä'), "clipped on a character boundary");
        assert!(g.now_next("Name 7", from).is_some() && g.now_next("Name 8", from).is_none(), "the first names only");
    }

    #[test]
    fn a_document_that_is_not_a_guide_is_an_error() {
        assert!(parse(b"<html><body>Not found</body></html>", 0, 1).is_err());
        assert!(parse(b"", 0, 1).is_err());
    }
}
