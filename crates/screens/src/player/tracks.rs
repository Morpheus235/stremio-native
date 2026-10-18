//! **The player's menus as rows** — the audio and subtitle track pickers, the `…` menu, the show
//! info's technical summary line ([`tech_line`]) and the subtitle size/position ladders, built
//! PURE from what the media layer published
//! ([`MediaFacts`]) and the settings store, so the fit tests and the selection tests build the
//! real menus.
//!
//! A track is named the way a viewer reads it: its language (the container's ISO 639 tag, as the
//! language's own name), the file's own track name when it says more, and for audio the format
//! (`Dolby Digital Plus 5.1`). A track the file names in no language is `Track n`. Names, codec
//! and language names are the FILE's facts (server text in the menu); the words around them are
//! the app's.
//!
//! The subtitle menu is its own two-column popover (`super::subtitle_menu`): the languages, then
//! the chosen one's subtitles, the file's and the addons' ([`MediaFacts::addon_subs`]). This
//! module names the tracks it lists and holds the delay steps it opens.

use sn_platform::i18n::msg;

use crate::registry::{AddonSubFacts, ChoiceTarget, ListRow, MediaFacts, MenuKind, PlaybackView, TrackFacts};
use crate::stores::settings::{Pref, SettingsStore};

/// The language a track's tag names, in its own name (`sn_base::tracks::language_name`, the one
/// table the Settings language lists read too).
pub use sn_base::tracks::language_name;

/// An audio format as a viewer knows it: FFmpeg's codec name to the product's.
fn format_name(codec: &str, atmos: bool) -> String {
    let name = match codec.to_ascii_lowercase().as_str() {
        "" => return String::new(),
        "ac3" => "Dolby Digital",
        "eac3" if atmos => "Dolby Digital Plus Atmos",
        "eac3" => "Dolby Digital Plus",
        "truehd" if atmos => "Dolby TrueHD Atmos",
        "truehd" => "Dolby TrueHD",
        "dts" => "DTS",
        "aac" => "AAC",
        "mp3" => "MP3",
        "mp2" => "MP2",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "flac" => "FLAC",
        "alac" => "ALAC",
        c if c.starts_with("pcm") => "PCM",
        other => return other.to_ascii_uppercase(),
    };
    name.to_string()
}

/// A channel count as a layout: `2.0`, `5.1`, `7.1`.
fn channel_layout(channels: i64) -> Option<&'static str> {
    Some(match channels {
        1 => "1.0",
        2 => "2.0",
        3 => "2.1",
        6 => "5.1",
        7 => "6.1",
        8 => "7.1",
        _ => return None,
    })
}

/// The format line of an audio track: `Dolby Digital Plus 5.1`; empty when unknown. A track the
/// player converts on the CPU says what it becomes: `Dolby TrueHD 5.1 → Dolby Digital`,
/// `FLAC 2.0 → PCM`.
pub fn audio_format(t: &TrackFacts) -> String {
    let format = format_name(&t.codec, t.atmos);
    let format = match (format.is_empty(), channel_layout(t.channels)) {
        (true, _) => String::new(),
        (false, Some(layout)) => format!("{format} {layout}"),
        (false, None) => format,
    };
    // LPCM already reads `PCM 7.1`; an arrow to PCM would only repeat it.
    let repeats = t.fed == "PCM" && t.codec.to_ascii_lowercase().starts_with("pcm");
    if !format.is_empty() && !t.fed.is_empty() && !repeats {
        return format!("{format} \u{2192} {}", t.fed);
    }
    format
}

/// The video codec as a viewer knows it.
fn video_format(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "h264" => "H.264".into(),
        "hevc" | "h265" => "HEVC".into(),
        "av1" => "AV1".into(),
        "vp9" => "VP9".into(),
        "vp8" => "VP8".into(),
        "mpeg2video" => "MPEG-2".into(),
        "mpeg4" => "MPEG-4".into(),
        other => other.to_ascii_uppercase(),
    }
}

/// One track's row: `English · Director's commentary · Dolby Digital 5.1`. `n` is its position
/// (1-based), named when the file gives no language.
pub fn track_label(t: &TrackFacts, n: usize, audio: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    let lang = language_name(&t.language);
    let name = t.name.trim();
    match &lang {
        Some(l) => parts.push(l.clone()),
        None if name.is_empty() => parts.push(msg::app_player_track(n as i64)),
        None => {}
    }
    if !name.is_empty() && lang.as_deref().is_none_or(|l| !name.eq_ignore_ascii_case(l)) {
        parts.push(name.to_string());
    }
    if audio {
        let format = audio_format(t);
        if !format.is_empty() && !name.contains(&format) {
            parts.push(format);
        }
    }
    parts.join(" \u{b7} ")
}

/// The audio track menu: one row per track, the one fed checked. `None` when the stream has no
/// track list yet (the menu then says so).
/// The audio menu: the stream's tracks, then *Audio sync* with the sync in force, which opens its
/// steps (`crate::dropdown::audio_sync_menu`; a pick of index `audio.len()`).
pub fn audio_menu(view: &crate::registry::PlaybackView) -> MenuKind {
    let media = &view.media;
    if media.audio.is_empty() {
        return empty(msg::app_player_audio(), ChoiceTarget::PlayerAudio);
    }
    let sync = format!("{} \u{b7} {}", msg::app_player_audio_sync(), delay_label(view.audio_delay_ms));
    MenuKind::Choice {
        title: msg::app_player_audio().to_string(),
        options: media.audio.iter().enumerate().map(|(i, t)| track_label(t, i + 1, true)).chain([sync]).collect(),
        current: media.audio_sel.unwrap_or(usize::MAX),
        target: ChoiceTarget::PlayerAudio,
    }
}

/// An addon's subtitle as the menu names it: `Deutsch · OpenSubtitles`, the addon alone for an
/// untagged one.
pub fn addon_sub_label(t: &AddonSubFacts) -> String {
    match language_name(&t.language) {
        Some(lang) => msg::app_player_subtitle_addon(&t.addon, &lang),
        None => t.addon.clone(),
    }
}

pub use crate::dropdown::{delay_label, DELAY_STEP_MS};

/// The delay menu's rows, in order: earlier and later by [`DELAY_STEP_MS`], and back to none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelayRow {
    Earlier,
    Later,
    Reset,
}

pub const DELAY_ROWS: [DelayRow; 3] = [DelayRow::Earlier, DelayRow::Later, DelayRow::Reset];

/// The delay row `i` asks for, from `delay_ms` (`PlayerCmd::SubtitleDelay`'s value).
pub fn delay_pick(i: usize, delay_ms: i64) -> Option<i64> {
    Some(match DELAY_ROWS.get(i)? {
        DelayRow::Earlier => delay_ms - DELAY_STEP_MS,
        DelayRow::Later => delay_ms + DELAY_STEP_MS,
        DelayRow::Reset => 0,
    })
}

/// A menu with nothing to pick: one row saying so.
fn empty(title: &str, target: ChoiceTarget) -> MenuKind {
    MenuKind::List {
        title: title.to_string(),
        rows: vec![ListRow { label: msg::widgets_tracks_empty().to_string(), value: String::new(), server_value: false, opens: false, switch: None }],
        target,
    }
}

/// The `…` menu's rows, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoreRow {
    /// The show info: what plays, as its meta tells it (`super::show_info`).
    Info,
    /// This playback's aspect mode: opens [`aspect_menu`].
    Aspect,
    /// Stats for nerds, on or off: the press toggles it (`PlayerCmd::ToggleStats`). Last, so
    /// the read-out, which is skipped while a menu is up, never hides its own switch.
    Stats,
}

pub const MORE_ROWS: [MoreRow; 3] = [MoreRow::Info, MoreRow::Aspect, MoreRow::Stats];

/// The aspect modes' names, in the media layer's `AspectMode::ALL` order: a pick's index is
/// `PlayerCmd::Aspect`'s (the application's test holds the two lists the same length).
pub fn aspect_labels() -> [&'static str; 5] {
    [
        msg::app_player_aspect_auto(),
        msg::app_player_aspect_fill(),
        msg::app_player_aspect_stretch(),
        msg::app_player_aspect_zoom_4_3(),
        msg::app_player_aspect_zoom_21_9(),
    ]
}

/// The Stats for nerds row's value.
pub fn stats_value(on: bool) -> &'static str {
    if on {
        msg::app_player_stats_on()
    } else {
        msg::app_player_stats_off()
    }
}

/// The `…` menu: Show info, this playback's aspect mode and Stats for nerds (subtitle styling is
/// the subtitle menu's Style tab).
pub fn more_menu(view: &PlaybackView) -> MenuKind {
    let aspect = aspect_labels().get(usize::from(view.aspect)).copied().unwrap_or_else(msg::app_player_aspect_auto);
    MenuKind::List {
        title: msg::app_player_more().to_string(),
        rows: MORE_ROWS
            .iter()
            .map(|r| match r {
                MoreRow::Info => ListRow { label: msg::app_player_show_info().to_string(), value: String::new(), server_value: false, opens: true, switch: None },
                MoreRow::Aspect => ListRow { label: msg::app_player_aspect().to_string(), value: aspect.to_string(), server_value: false, opens: true, switch: None },
                MoreRow::Stats => {
                    ListRow { label: msg::app_player_stats().to_string(), value: stats_value(view.stats).to_string(), server_value: false, opens: false, switch: None }
                }
            })
            .collect(),
        target: ChoiceTarget::PlayerMore,
    }
}

/// The aspect modes, the current one checked; a pick `i` is `PlayerCmd::Aspect(i)`.
pub fn aspect_menu(view: &PlaybackView) -> MenuKind {
    MenuKind::Choice {
        title: msg::app_player_aspect().to_string(),
        options: aspect_labels().iter().map(|s| s.to_string()).collect(),
        current: usize::from(view.aspect),
        target: ChoiceTarget::PlayerAspect,
    }
}

/// A preference's ladder, the current rung checked; a pick is `SettingsCmd::Set(pref, i)`.
pub fn pref_menu(settings: &SettingsStore, pref: Pref) -> MenuKind {
    MenuKind::Choice {
        title: pref.label().to_string(),
        options: (0..pref.rungs()).map(|r| pref.rung_label(r).to_string()).collect(),
        current: usize::from(settings.get(pref)),
        target: match pref {
            Pref::SubtitleSize => ChoiceTarget::PlayerSubtitleSize,
            Pref::SubtitlePosition => ChoiceTarget::PlayerSubtitlePosition,
            Pref::SkipIntros => ChoiceTarget::PlayerSkipIntros,
            Pref::DvProfile7 => ChoiceTarget::PlayerDvProfile7,
            // ponytail: the player never offers these (they are Settings' own); its
            // `…` menu is where a stray pick would land harmlessly
            Pref::SubtitleMode
            | Pref::NextEpisode
            | Pref::ForcedFallback
            | Pref::SubtitleColor
            | Pref::SubtitleBackground
            | Pref::SubtitleOutline
            | Pref::SubtitleOpacity
            | Pref::AssStyling
            | Pref::AutoPlay
            | Pref::UpNextAt
            | Pref::Language
            | Pref::MultichannelAudio => ChoiceTarget::PlayerMore,
        },
    }
}

/// A picture height as a viewer names it: `4K`, `1080p`.
fn resolution_name(width: i32, height: i32) -> Option<String> {
    Some(match (width, height) {
        (w, h) if w >= 3200 || h >= 2000 => "4K".into(),
        (w, h) if w >= 2400 || h >= 1400 => "1440p".into(),
        (w, h) if w >= 1800 || h >= 1000 => "1080p".into(),
        (w, h) if w >= 1200 || h >= 700 => "720p".into(),
        (_, h) if h > 0 => format!("{h}p"),
        _ => return None,
    })
}

/// The show info's one technical line: the picture's size and Dolby Vision profile, the audio
/// in use and the addon the stream came from (`4K · DV P8 · Dolby Digital Plus Atmos 5.1 ·
/// AIOStreams`). A fact the media layer does not know is left out. The detail lives in Stats for
/// nerds.
pub fn tech_line(media: &MediaFacts, addon: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = &media.video {
        parts.extend(resolution_name(v.width, v.height));
        match v.dovi_profile {
            Some(p) => parts.push(format!("DV P{p}")),
            None if !v.codec.is_empty() => parts.push(video_format(&v.codec)),
            None => {}
        }
    }
    if let Some(t) = media.audio_sel.and_then(|i| media.audio.get(i)) {
        let f = audio_format(t);
        if !f.is_empty() {
            parts.push(f);
        }
    }
    if !addon.trim().is_empty() {
        parts.push(addon.trim().to_string());
    }
    parts.join(" \u{b7} ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::VideoFacts;

    fn audio(lang: &str, name: &str, codec: &str, channels: i64) -> TrackFacts {
        TrackFacts { language: lang.into(), name: name.into(), codec: codec.into(), channels, atmos: false, fed: "" }
    }

    #[test]
    fn a_language_tag_reads_as_the_language() {
        assert_eq!(language_name("eng").as_deref(), Some("English"));
        assert_eq!(language_name("ger").as_deref(), Some("Deutsch"), "the bibliographic code");
        assert_eq!(language_name("deu").as_deref(), Some("Deutsch"), "the terminology code");
        assert_eq!(language_name("pt-BR").as_deref(), Some("Português"), "a region reads by its language");
        assert_eq!(language_name("FRE").as_deref(), Some("Français"));
        assert_eq!(language_name("und"), None, "undetermined is no language");
        assert_eq!(language_name(""), None);
        assert_eq!(language_name("tlh").as_deref(), Some("tlh"), "an unknown code is shown as given");
    }

    #[test]
    fn a_track_reads_language_name_and_format() {
        let _g = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        assert_eq!(track_label(&audio("eng", "", "eac3", 6), 1, true), "English \u{b7} Dolby Digital Plus 5.1");
        assert_eq!(track_label(&audio("rus", "Дубляж", "ac3", 2), 2, true), "Русский \u{b7} Дубляж \u{b7} Dolby Digital 2.0");
        assert_eq!(track_label(&audio("", "", "aac", 2), 3, true), "Track 3 \u{b7} AAC 2.0", "no language: by position");
        assert_eq!(track_label(&audio("", "Commentary", "", 0), 4, true), "Commentary");
        let fed = |t: TrackFacts, fed| TrackFacts { fed, ..t };
        assert_eq!(track_label(&fed(audio("eng", "", "flac", 2), "PCM"), 1, true), "English \u{b7} FLAC 2.0 \u{2192} PCM", "decoded on the CPU");
        assert_eq!(track_label(&fed(audio("", "", "pcm_bluray", 8), "PCM"), 2, true), "Track 2 \u{b7} PCM 7.1");
        assert_eq!(
            track_label(&fed(audio("eng", "", "truehd", 6), "Dolby Digital"), 1, true),
            "English \u{b7} Dolby TrueHD 5.1 \u{2192} Dolby Digital",
            "re-encoded for an optical link"
        );
        assert_eq!(track_label(&fed(audio("", "", "pcm_bluray", 8), "Dolby Digital"), 2, true), "Track 2 \u{b7} PCM 7.1 \u{2192} Dolby Digital");
        assert_eq!(track_label(&fed(audio("eng", "", "ac3", 6), "Pro Logic II"), 1, true), "English \u{b7} Dolby Digital 5.1 \u{2192} Pro Logic II");
        assert_eq!(track_label(&audio("eng", "English", "", 0), 1, false), "English", "a name that only repeats the language");
        assert_eq!(track_label(&audio("spa", "Forced", "subrip", 0), 1, false), "Español \u{b7} Forced", "a subtitle has no format line");
    }

    fn media() -> MediaFacts {
        MediaFacts {
            audio: vec![audio("eng", "", "eac3", 6), audio("fre", "", "ac3", 2)],
            audio_sel: Some(1),
            subs: vec![audio("eng", "SDH", "", 0), audio("ger", "", "", 0)],
            addon_subs: Vec::new(),
            sub_sel: None,
            video: Some(VideoFacts { codec: "hevc".into(), width: 3840, height: 2160, fps_milli: 23_976, dovi_profile: Some(8) }),
            chapters: Vec::new(),
        }
    }

    #[test]
    fn the_audio_menu_checks_what_plays() {
        let view = |media: MediaFacts| crate::registry::PlaybackView { media, audio_delay_ms: -1500, ..crate::registry::PlaybackView::IDLE };
        let MenuKind::Choice { options, current, target, .. } = audio_menu(&view(media())) else { panic!("a choice") };
        assert_eq!((options.len(), current, target), (3, 1, ChoiceTarget::PlayerAudio), "the tracks, then the audio sync");
        assert!(options[2].ends_with("\u{2212}1.50 s"), "the sync row says the sync in force: {}", options[2]);
        assert!(matches!(audio_menu(&view(MediaFacts::NONE)), MenuKind::List { .. }), "no tracks yet: a row saying so");
    }

    #[test]
    fn the_delay_reads_signed() {
        assert_eq!((delay_label(0), delay_label(1250), delay_label(-250)), ("0.00 s".into(), "+1.25 s".into(), "\u{2212}0.25 s".into()));
    }

    #[test]
    fn the_delay_steps_a_quarter_second_and_resets() {
        assert_eq!(delay_pick(0, 0), Some(-250));
        assert_eq!(delay_pick(1, 500), Some(750));
        assert_eq!(delay_pick(2, 750), Some(0));
        assert_eq!(delay_pick(3, 0), None);
    }

    #[test]
    fn the_tech_line_says_what_the_stream_is_and_leaves_out_what_is_unknown() {
        let _g = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        let atmos = TrackFacts { atmos: true, ..audio("eng", "", "eac3", 6) };
        let m = MediaFacts { audio: vec![atmos], audio_sel: Some(0), ..media() };
        assert_eq!(tech_line(&m, "AIOStreams"), "4K \u{b7} DV P8 \u{b7} Dolby Digital Plus Atmos 5.1 \u{b7} AIOStreams");
        let hd = MediaFacts { video: Some(VideoFacts { codec: "h264".into(), width: 1920, height: 800, fps_milli: 24_000, dovi_profile: None }), ..MediaFacts::NONE };
        assert_eq!(tech_line(&hd, ""), "1080p \u{b7} H.264", "a scope 1080p is 1080p; no Dolby Vision, the codec");
        assert_eq!(tech_line(&MediaFacts::NONE, " "), "", "nothing known, nothing said");
    }
}
