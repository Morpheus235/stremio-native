//! The `stremionative-playurl` trigger: a stream to play and the Load payload declaration to play it
//! with, parsed beside the one module that acts on it ([`super::engine`]'s `start_bufferfeed`).
//!
//! It is a TYPED dev trigger: its value is a JSON object that becomes a
//! [`crate::session::PlaybackStart`], so it can live neither with the trigger primitives
//! (`sn_base::devtrig` names no media type) nor in the application. A release build folds it away
//! at COMPILE time: `read` is `None` without the `devtriggers` feature, so [`playurl`] answers
//! `None` and no declaration can be injected.

use std::collections::BTreeMap;

/// A stream to play and **the Load payload declaration to play it with**, with nothing behind it —
/// `/tmp/stremionative-playurl`, the player-PIPELINE test tier's one entry point.
///
/// `stremionative-url` hands the engine a URL alone; the Load payload's codecs, frame rate and
/// Dolby nodes come from the declaration, so a tier that cannot set it cannot test HEVC-vs-H264
/// payload selection, the `"AC3 PLUS"` naming trap, the DTS policy or either Dolby node.
///
/// JSON, whole-file, one object; JSON contains no apostrophes, which matters because the harness
/// writes triggers through a single-quoted `printf` with no escaping.
///
/// ```jsonc
/// {"url":"http://192.0.2.10:8020/pipe_hevc_eac3_4k_dovi_p8.mkv",
///  "vcodec":"hevc", "acodec":"eac3", "channels":6, "fps":23.976,
///  "dovi":{"profile":8,"bl_compat":1,"el_present":false},
///  "atmos":false, "headers":{"Referer":"https://live.example/"}}
/// ```
///
/// Deliberately **not** `Debug`: `url` and `headers` are free strings, and a derived `Debug` is how
/// a credential in either reaches a log the day someone points this trigger at a real addon.
#[derive(serde::Deserialize, Clone)]
pub struct PlayUrl {
    /// `http(s)://…`.
    #[serde(default)]
    pub url: String,
    /// The Load payload's video codec, FFmpeg's spelling: `"h264"` or empty, `"hevc"`, `"av1"`,
    /// `"vp9"` (the last two only on a set whose table lists that decoder).
    #[serde(default)]
    pub vcodec: String,
    /// The audio codec, in FFmpeg's spelling (`"eac3"`, not `"AC3 PLUS"`) — the engine does the
    /// LG-side renaming, which is the trap this tier exists to keep testing.
    #[serde(default)]
    pub acodec: String,
    /// The audio track's channel count; 0 = unknown (which a DTS track never fits).
    #[serde(default)]
    pub channels: i64,
    /// Source frame rate for the Load `esInfo`; 0 omits it.
    #[serde(default)]
    pub fps: f64,
    /// Dolby Vision layering, for the payload's `contents.DolbyHdrInfo` node. Absent = none.
    #[serde(default)]
    pub dovi: PlayDovi,
    /// Dolby Atmos, for the payload's `contents.immersive` node.
    #[serde(default)]
    pub atmos: bool,
    /// The coded raster `[w, h]`, for the H.264 sink envelope. Absent = unknown.
    #[serde(default)]
    pub raster: Option<[u16; 2]>,
    /// The stream's request headers (`behaviorHints.proxyHeaders.request`).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

/// The four Dolby Vision fields the Load payload decides on.
#[derive(serde::Deserialize, Clone, Copy, Default)]
pub struct PlayDovi {
    /// 5 / 7 / 8. **Zero means no Dolby Vision at all** — it is what drives `present`, rather
    /// than a separate flag that could disagree with it.
    #[serde(default)]
    pub profile: i64,
    /// 0 none (P5) / 1 HDR10 / 2 SDR / 4 HLG.
    #[serde(default)]
    pub bl_compat: i64,
    /// An enhancement layer is present (P7).
    #[serde(default)]
    pub el_present: bool,
}

impl PlayDovi {
    /// The engine-facing record. `present` is DERIVED from a non-zero profile.
    pub fn to_dovi(self) -> crate::stream::Dovi {
        crate::stream::Dovi {
            present: self.profile > 0,
            profile: self.profile,
            bl_compat: self.bl_compat,
            el_present: self.el_present,
        }
    }
}

impl PlayUrl {
    /// The playback this trigger asks for. `Err` names a header the media plane refuses.
    pub fn to_start(&self) -> Result<crate::session::PlaybackStart, crate::headers::HeaderRefusal> {
        let pairs: Vec<(&str, &str)> = self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mut start = crate::session::PlaybackStart::new(self.url.clone());
        start.headers = crate::headers::RequestHeaders::from_pairs(&pairs)?;
        start.stream = crate::stream::StreamDeclaration {
            vcodec: self.vcodec.clone(),
            acodec: self.acodec.clone(),
            audio_channels: self.channels,
            fps: self.fps,
            raster: self.raster.map_or((0, 0), |[w, h]| (w, h)),
            dovi: self.dovi.to_dovi(),
            atmos: self.atmos,
        };
        Ok(start)
    }
}

/// Parse the `playurl` trigger's content. Pure, so the host suite can pin it.
///
/// An `Err` rather than a defaulted object on malformed input: a run whose declaration was
/// silently dropped grades as "the payload is wrong", when the fault is a typo in the harness. An
/// empty `url` is an `Err` too — an all-defaults object would send the engine looking for
/// `stremionative-url` instead and the case would play something else entirely.
#[cfg(any(feature = "devtriggers", test))]
fn parse_playurl(s: &str) -> Result<PlayUrl, String> {
    let p: PlayUrl = serde_json::from_str(s).map_err(|e| e.to_string())?;
    if p.url.is_empty() {
        return Err("no `url`".to_string());
    }
    Ok(p)
}

/// This boot's URL-and-declaration, if one was armed — `/tmp/stremionative-playurl`.
///
/// `None` = not armed; `Some(Err)` = armed but unreadable, which the caller logs. **Not memoized**:
/// every start re-reads it, because a seek that escalates to a full reload builds the payload
/// again, and a declaration that applied only to the first `Load` would make the second one wrong.
#[cfg(feature = "devtriggers")]
pub fn playurl() -> Option<Result<PlayUrl, String>> {
    sn_base::devtrig::read("playurl").map(|s| parse_playurl(&s))
}
#[cfg(not(feature = "devtriggers"))]
pub fn playurl() -> Option<Result<PlayUrl, String>> {
    None
}

#[cfg(test)]
mod tests {
    /// The payload the harness writes, verbatim: nothing links the two languages at build time.
    #[test]
    fn the_harness_payload_parses_to_the_declaration_it_names() {
        let p = super::parse_playurl(
            r#"{"url":"http://192.0.2.10:8020/pipe_hevc_eac3_4k_dovi_p8.mkv","vcodec":"hevc",
                "acodec":"eac3","channels":6,"fps":23.976,
                "dovi":{"profile":8,"bl_compat":1,"el_present":false},"atmos":false,
                "headers":{"User-Agent":"LiveClient/2.6","Referer":"https://live.example/"}}"#,
        )
        .unwrap();
        let start = p.to_start().unwrap();
        assert_eq!(start.url, "http://192.0.2.10:8020/pipe_hevc_eac3_4k_dovi_p8.mkv");
        assert_eq!((start.stream.vcodec.as_str(), start.stream.acodec.as_str()), ("hevc", "eac3"));
        assert_eq!(start.stream.audio_channels, 6);
        assert!((start.stream.fps - 23.976).abs() < 1e-9);
        assert!(!start.stream.atmos);
        let dv = start.stream.dovi;
        assert!(dv.present, "profile 8 must derive present=true");
        assert_eq!((dv.profile, dv.bl_compat, dv.el_present), (8, 1, false));
        assert_eq!(start.headers.user_agent(), Some("LiveClient/2.6"));
        assert_eq!(start.headers.lines(true).collect::<Vec<_>>(), ["Referer: https://live.example/"]);
    }

    /// Omitted fields must mean "no Dolby anything, no headers" — not a parse failure.
    #[test]
    fn omitted_fields_default_to_a_silent_declaration() {
        let p = super::parse_playurl(r#"{"url":"http://10.0.0.2:8020/a.mkv","vcodec":"h264","acodec":"ac3"}"#).unwrap();
        let start = p.to_start().unwrap();
        assert_eq!(start.stream.fps, 0.0);
        assert!(!start.stream.atmos);
        assert_eq!(start.stream.dovi, crate::stream::Dovi::NONE, "an absent dovi node must be silence itself");
        assert!(start.headers.is_empty());
        assert_eq!(start.stream.raster, (0, 0));
    }

    /// A header the media plane refuses refuses the trigger, rather than playing without it.
    #[test]
    fn a_refused_header_is_an_error_not_a_silent_drop() {
        let p = super::parse_playurl(r#"{"url":"http://10.0.0.2/a.mkv","headers":{"Range":"bytes=0-"}}"#).unwrap();
        assert!(p.to_start().is_err());
    }

    /// `present` is DERIVED, so it cannot disagree with the profile in either direction.
    #[test]
    fn dovi_presence_follows_the_profile() {
        let none = super::PlayDovi { profile: 0, bl_compat: 1, el_present: true }.to_dovi();
        assert!(!none.present, "profile 0 is not Dolby Vision whatever else is set");
        let p7 = super::PlayDovi { profile: 7, bl_compat: 6, el_present: true }.to_dovi();
        assert!(p7.present);
        assert_eq!((p7.profile, p7.bl_compat, p7.el_present), (7, 6, true));
    }

    /// An empty `url` must be an Err, not an all-defaults object: on `Ok` the engine would take the
    /// empty URL, fall through to `stremionative-url` or a local sample, and play something else.
    #[test]
    fn an_empty_url_is_refused_rather_than_defaulted() {
        assert!(super::parse_playurl(r#"{"vcodec":"hevc"}"#).is_err());
        assert!(super::parse_playurl(r#"{"url":""}"#).is_err());
        assert!(super::parse_playurl("").is_err());
        assert!(super::parse_playurl("not json at all").is_err());
    }

    /// The harness writes triggers through a single-quoted `printf` with NO escaping, so an
    /// apostrophe anywhere in the payload would end the quoting.
    #[test]
    fn the_harness_payload_carries_no_apostrophe() {
        let payload = r#"{"url":"http://192.0.2.10:8020/pipe_h264_ac3_1080p.mkv","vcodec":"h264","acodec":"ac3","fps":24.0}"#;
        assert!(!payload.contains('\''), "would break apply_triggers' single-quoted printf");
        assert!(super::parse_playurl(payload).is_ok());
    }
}
