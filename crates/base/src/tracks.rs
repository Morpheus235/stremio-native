//! **Which audio and subtitle tracks a playback starts on**, by the viewer's language
//! preferences, as plain data. PURE.
//!
//! The settings store (`sn_screens::stores::settings`) turns its rungs into a [`TrackPrefs`]; the
//! media layer (`sn_media`), which cannot name the screens, reads one off the playback's start and
//! picks the tracks against what the probe found before the Load. Both pick through this module,
//! so the Settings page and the player can never disagree about what a preference means.

/// A track's language tag (`"eng"`, `"de-DE"`, `"fre"`) as an upper-case ISO 639-1 code
/// (`"EN"`, `"DE"`, `"FR"`); empty for an untagged track. A code it does not know is its own
/// base tag upper-cased.
pub fn language_code(tag: &str) -> String {
    let base = tag.split(['-', '_']).next().unwrap_or("").trim().to_ascii_lowercase();
    language_index(&base).map_or(base, |i| LANGUAGES[i].0.to_string()).to_ascii_uppercase()
}

/// Where the language `tag` names (ISO 639-1 or -2, any case, no region) stands in [`LANGUAGES`].
pub fn language_index(tag: &str) -> Option<usize> {
    let tag = tag.trim().to_ascii_lowercase();
    LANGUAGES.iter().position(|(short, long, _)| *short == tag || long.contains(&tag.as_str()))
}

/// The language a track's tag names, in its own name (`"ger"` → `Deutsch`), or `None` for no tag,
/// `und` (undetermined) and the other non-languages. A region suffix (`pt-BR`) is read by its
/// language; an unknown code is shown as the file gives it.
pub fn language_name(tag: &str) -> Option<String> {
    let tag = tag.trim();
    let lang = tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    if lang.is_empty() || matches!(lang.as_str(), "und" | "unk" | "mis" | "mul" | "zxx" | "qaa") {
        return None;
    }
    Some(language_index(&lang).map_or_else(|| tag.to_string(), |i| LANGUAGES[i].2.to_string()))
}

/// **Every language the app names**: its ISO 639-1 code, the ISO 639-2 codes (bibliographic and
/// terminology, plus the odd OpenSubtitles spelling: `pob` Brazilian Portuguese, `scc` Serbian)
/// and Norwegian's 639-1 variants that mean it, and the language's own name. Data, not translated
/// text: a language is listed the way its speakers write it, as every track picker does.
pub const LANGUAGES: &[(&str, &[&str], &str)] = &[
    ("en", &["eng"], "English"),
    ("es", &["spa", "spl", "spn"], "Español"),
    ("fr", &["fre", "fra"], "Français"),
    ("de", &["ger", "deu"], "Deutsch"),
    ("it", &["ita"], "Italiano"),
    ("pt", &["por", "pob", "pom", "pb"], "Português"),
    ("ru", &["rus"], "Русский"),
    ("be", &["bel"], "Беларуская"),
    ("uk", &["ukr"], "Українська"),
    ("pl", &["pol"], "Polski"),
    ("cs", &["cze", "ces"], "Čeština"),
    ("sk", &["slo", "slk"], "Slovenčina"),
    ("hu", &["hun"], "Magyar"),
    ("ro", &["rum", "ron"], "Română"),
    ("bg", &["bul"], "Български"),
    ("sr", &["srp", "scc"], "Српски"),
    ("hr", &["hrv", "scr"], "Hrvatski"),
    ("bs", &["bos"], "Bosanski"),
    ("sl", &["slv"], "Slovenščina"),
    ("mk", &["mac", "mkd"], "Македонски"),
    ("el", &["gre", "ell"], "Ελληνικά"),
    ("tr", &["tur"], "Türkçe"),
    ("nl", &["dut", "nld"], "Nederlands"),
    ("sv", &["swe"], "Svenska"),
    ("no", &["nor", "nb", "nob", "nn", "nno"], "Norsk"),
    ("da", &["dan"], "Dansk"),
    ("fi", &["fin"], "Suomi"),
    ("is", &["ice", "isl"], "Íslenska"),
    ("et", &["est"], "Eesti"),
    ("lv", &["lav"], "Latviešu"),
    ("lt", &["lit"], "Lietuvių"),
    ("ga", &["gle"], "Gaeilge"),
    ("cy", &["wel", "cym"], "Cymraeg"),
    ("ca", &["cat"], "Català"),
    ("eu", &["baq", "eus"], "Euskara"),
    ("gl", &["glg"], "Galego"),
    ("ar", &["ara"], "العربية"),
    ("he", &["heb"], "עברית"),
    ("fa", &["per", "fas"], "فارسی"),
    ("ur", &["urd"], "اردو"),
    ("hi", &["hin"], "हिन्दी"),
    ("bn", &["ben"], "বাংলা"),
    ("ta", &["tam"], "தமிழ்"),
    ("te", &["tel"], "తెలుగు"),
    ("ml", &["mal"], "മലയാളം"),
    ("kn", &["kan"], "ಕನ್ನಡ"),
    ("mr", &["mar"], "मराठी"),
    ("pa", &["pan"], "ਪੰਜਾਬੀ"),
    ("th", &["tha"], "ไทย"),
    ("vi", &["vie"], "Tiếng Việt"),
    ("id", &["ind"], "Bahasa Indonesia"),
    ("ms", &["may", "msa"], "Bahasa Melayu"),
    ("tl", &["tgl", "fil"], "Filipino"),
    ("zh", &["chi", "zho", "zht", "zhe", "chs", "cht"], "中文"),
    ("ja", &["jpn"], "日本語"),
    ("ko", &["kor"], "한국어"),
    ("ka", &["geo", "kat"], "ქართული"),
    ("hy", &["arm", "hye"], "Հայերեն"),
    ("az", &["aze"], "Azərbaycan"),
    ("kk", &["kaz"], "Қазақ"),
    ("uz", &["uzb"], "Oʻzbek"),
    ("sq", &["alb", "sqi"], "Shqip"),
    ("af", &["afr"], "Afrikaans"),
    ("sw", &["swa"], "Kiswahili"),
    ("la", &["lat"], "Latina"),
];

/// The languages a preference list offers to add, in the order the picker lists them: the common
/// languages of films and subtitles, as [`language_code`] codes.
pub const COMMON: [&str; 33] = [
    "EN", "DE", "FR", "ES", "IT", "PT", "RU", "UK", "PL", "NL", "SV", "NO", "DA", "FI", "CS", "SK", "HU", "RO",
    "BG", "EL", "TR", "HR", "SR", "AR", "HE", "HI", "JA", "KO", "ZH", "TH", "VI", "ID", "BE",
];

/// One audio or subtitle track as the pick reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Track<'a> {
    /// The container's language tag (ISO 639-1 or -2, maybe with a region); `""` when untagged.
    pub language: &'a str,
    /// A forced (signs and foreign dialogue) track: the file's flag, or "forced" in its title.
    pub forced: bool,
    /// The file's own default flag.
    pub default: bool,
}

/// Whether subtitles show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubsMode {
    Off,
    /// Always, the preferred language's.
    On,
    /// Only a forced track (signs and foreign dialogue).
    Forced,
    /// On only when the audio is not in [`TrackPrefs::native`]; Forced when it is.
    #[default]
    Auto,
}

/// The viewer's track preferences, as [`language_code`] codes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackPrefs {
    /// The audio languages, first preferred first; empty plays the file's default track.
    pub audio: Vec<String>,
    /// The subtitle languages, first preferred first.
    pub subtitles: Vec<String>,
    pub mode: SubsMode,
    /// The language [`SubsMode::Auto`] counts as understood: the first audio language, or the
    /// app's while audio follows the file.
    pub native: String,
    /// Forced only (and Auto over understood audio) also accepts a forced track in a subtitle
    /// language when the audio's own language has none.
    pub forced_fallback: bool,
}

/// Where [`TrackPrefs::subtitle_among`] landed: a track of the file, or one an addon offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubPick {
    Embedded(usize),
    Addon(usize),
}

impl TrackPrefs {
    /// Where `tag` stands in the audio order: 0 the first preferred language; `audio.len()` (last)
    /// for a language not in it, and for every track while audio follows the file.
    pub fn audio_rank(&self, tag: &str) -> usize {
        let code = language_code(tag);
        self.audio.iter().position(|l| *l == code).unwrap_or(self.audio.len())
    }

    /// The audio track: the first in the first preferred language that has one, else the file's
    /// default track, else the first.
    pub fn audio(&self, audio: &[Track<'_>]) -> Option<usize> {
        first_in(audio, &self.audio, &|_| true).or_else(|| fallback(audio))
    }

    /// The subtitle track to show over audio in `audio_tag`, or `None` (off). Off is none; On the
    /// first non-forced track in the first subtitle language that has one (then a forced one, then
    /// the file's default, then the first); Forced only a forced track in the audio's language
    /// (else none); Auto is Forced when the audio is in
    /// [`Self::native`] and On otherwise.
    pub fn subtitle(&self, audio_tag: &str, subs: &[Track<'_>]) -> Option<usize> {
        let a_lang = language_code(audio_tag);
        // the forced track belongs to the audio it is muxed for (the signs and foreign lines of
        // what is being heard); another language's forced track is never shown
        let forced = || {
            subs.iter()
                .position(|t| t.forced && !a_lang.is_empty() && language_code(t.language) == a_lang)
                .or_else(|| self.forced_fallback.then(|| first_in(subs, &self.subtitles, &|t| t.forced)).flatten())
        };
        let on = || {
            first_in(subs, &self.subtitles, &|t| !t.forced)
                .or_else(|| first_in(subs, &self.subtitles, &|_| true))
                .or_else(|| fallback(subs))
        };
        match self.mode {
            SubsMode::Off => None,
            SubsMode::On => on(),
            SubsMode::Forced => forced(),
            SubsMode::Auto if !a_lang.is_empty() && a_lang == self.native => forced(),
            SubsMode::Auto => on(),
        }
    }

    /// The subtitle to show over audio in `audio_tag` from the file's `embedded` tracks and the
    /// `addon` ones (which arrive later, from subtitle addons). The file's track in a preferred
    /// language beats an addon's in the same language (the pick reads the file's first); an
    /// addon's in a preferred language beats a file track in no preferred language; an addon's
    /// track is never a mere fallback (a language nobody asked for), so with no preferred match
    /// the pick is the file's alone.
    pub fn subtitle_among(&self, audio_tag: &str, embedded: &[Track<'_>], addon: &[Track<'_>]) -> Option<SubPick> {
        let own = || self.subtitle(audio_tag, embedded).map(SubPick::Embedded);
        if addon.is_empty() {
            return own();
        }
        let all: Vec<Track<'_>> = embedded.iter().chain(addon).copied().collect();
        match self.subtitle(audio_tag, &all) {
            Some(i) if i >= embedded.len() && self.subtitles.contains(&language_code(all[i].language)) => {
                Some(SubPick::Addon(i - embedded.len()))
            }
            _ => own(),
        }
    }
}

/// Does a track's title name it forced (`"Forced"`, `"English (forced)"`)?
pub fn titled_forced(title: &str) -> bool {
    title.to_ascii_lowercase().contains("forced")
}

/// The first track that passes `ok` in the first of `langs` that has one.
fn first_in(tracks: &[Track<'_>], langs: &[String], ok: &dyn Fn(&Track<'_>) -> bool) -> Option<usize> {
    langs.iter().find_map(|l| tracks.iter().position(|t| ok(t) && language_code(t.language) == *l))
}

/// The file's default track, else the first.
fn fallback(tracks: &[Track<'_>]) -> Option<usize> {
    tracks.iter().position(|t| t.default).or((!tracks.is_empty()).then_some(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(language: &str, forced: bool) -> Track<'_> {
        Track { language, forced, default: false }
    }

    #[test]
    fn forced_needs_the_audio_language() {
        let p = TrackPrefs { subtitles: vec!["FR".into()], mode: SubsMode::Forced, ..TrackPrefs::default() };
        let subs = [t("eng", false), t("fre", true), t("eng", true)];
        assert_eq!(p.subtitle("eng", &subs), Some(2), "the audio's forced track");
        assert_eq!(p.subtitle("ita", &subs), None, "a forced track in a subtitle language is not the audio's");
        assert_eq!(p.audio_rank("ger"), 0, "no audio order: every track ranks last (0 of 0)");
        assert!(titled_forced("English (Forced)") && !titled_forced("SDH"));
    }

    #[test]
    fn every_spelling_of_a_language_is_its_one_code() {
        for (tag, code) in [("ger", "DE"), ("deu", "DE"), ("cze", "CS"), ("pob", "PT"), ("pt-BR", "PT"), ("nob", "NO"), ("scc", "SR"), ("chi", "ZH"), ("EL", "EL"), ("tlh", "TLH")] {
            assert_eq!(language_code(tag), code, "{tag}");
        }
        assert_eq!(language_name("cze").as_deref(), Some("Čeština"));
        assert!(COMMON.iter().all(|c| language_index(c).is_some() && language_code(c) == *c), "every common language is named");
    }

    /// Forced fallback: Forced only takes a forced track in a subtitle language when the audio's
    /// own language has none — and only when asked to.
    #[test]
    fn forced_fallback_accepts_a_subtitle_languages_forced_track() {
        let mut p = TrackPrefs { subtitles: vec!["DE".into(), "FR".into()], mode: SubsMode::Forced, ..TrackPrefs::default() };
        let subs = [t("fre", true), t("ger", false), t("ger", true)];
        assert_eq!(p.subtitle("eng", &subs), None, "off: the audio's forced track or none");
        p.forced_fallback = true;
        assert_eq!(p.subtitle("eng", &subs), Some(2), "the first subtitle language's forced track");
        assert_eq!(p.subtitle("eng", &[t("eng", true), t("ger", true)]), Some(0), "the audio's own still first");
        assert_eq!(p.subtitle("eng", &[t("ger", false)]), None, "never a full track");
    }

    /// The file's tracks and an addon's, by one rule: the file's track in a preferred language
    /// beats an addon's; an addon's in a preferred language beats a file track in none and beats
    /// nothing; an addon's in a language nobody asked for is never picked.
    #[test]
    fn an_embedded_track_beats_an_addons_and_an_addons_beats_none() {
        let p = TrackPrefs { subtitles: vec!["DE".into(), "EN".into()], mode: SubsMode::On, ..TrackPrefs::default() };
        let addon = [t("eng", false), t("ger", false)];
        assert_eq!(p.subtitle_among("eng", &[t("ger", false)], &addon), Some(SubPick::Embedded(0)));
        assert_eq!(p.subtitle_among("eng", &[t("eng", false)], &addon), Some(SubPick::Addon(1)), "the addon's first-choice language");
        assert_eq!(p.subtitle_among("eng", &[t("ita", false)], &addon), Some(SubPick::Addon(1)), "over a language nobody asked for");
        assert_eq!(p.subtitle_among("eng", &[], &addon), Some(SubPick::Addon(1)), "over none");
        assert_eq!(p.subtitle_among("eng", &[t("ita", false)], &[t("spa", false)]), Some(SubPick::Embedded(0)), "the file's fallback, never an addon's");
        assert_eq!(p.subtitle_among("eng", &[], &[t("spa", false)]), None);
        assert_eq!(p.subtitle_among("eng", &[t("ger", false)], &[]), Some(SubPick::Embedded(0)));
    }
}
