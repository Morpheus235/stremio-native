//! **Subtitles from addons, for the playback**: what every subtitles addon answered for what plays
//! (`sn_stremio::subtitles`, asked off-frame by the bridge when the playback starts), merged into
//! the one list the player's subtitle menu shows after the file's own tracks, and the late pick
//! of one of them by the viewer's languages.
//!
//! - **The list** ([`AddonSubs::land`]): answers arrive one addon at a time, in any order. An
//!   obvious duplicate (the same URL, or the same id from the same addon) is dropped, each
//!   addon keeps at most [`PER_LANGUAGE`] entries per language, and the viewer's subtitle
//!   languages lead, in their order; the rest keep the order they came in. An entry is named by
//!   its identity (its URL here, its [`AddonSub::key`] in the menu), never its place, so a landing
//!   that re-sorts the list cannot move what is selected.
//! - **The pick** ([`AddonSubs::pick`]): the file's tracks and the addons' go through the one rule
//!   (`sn_base::tracks::TrackPrefs::subtitle_among`): the file's track in a preferred language beats
//!   an addon's, an addon's in a preferred language beats none. It runs when an answer lands and
//!   once the probe has read the file, and never after the viewer chose in the menu.
//!
//! A subtitle's URL can carry an addon's configuration: it is never logged ([`AddonSub`]'s `Debug`
//! leaves it out).

use sn_base::tracks::{language_code, SubPick, Track, TrackPrefs};
use sn_screens::registry::AddonSubFacts;
use sn_stremio::types::Subtitle;

/// How many subtitles one addon keeps per language in the menu: OpenSubtitles answers dozens per
/// language, most of them other releases' timings. Per addon, so the first addon to answer does not
/// crowd out the next one's.
pub const PER_LANGUAGE: usize = 3;

/// One addon's subtitle for what plays.
#[derive(Clone, PartialEq, Eq)]
pub struct AddonSub {
    /// The addon's language tag (`eng`, `pob`); server text.
    pub lang: String,
    /// The addon's name; server text.
    pub addon: String,
    pub id: String,
    /// Never logged.
    pub url: String,
    /// Its identity for the menu (`PlayerCmd::AddonSubtitle`): handed out in landing order and
    /// never reused within the playback.
    pub key: u32,
    /// The release or file name the addon gives; server text.
    pub release: String,
    pub forced: bool,
    pub hearing_impaired: bool,
}

impl std::fmt::Debug for AddonSub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddonSub").field("lang", &self.lang).field("addon", &self.addon).field("id", &self.id).field("key", &self.key).finish()
    }
}

/// The playback's addon subtitles and the late pick's state.
#[derive(Debug, Default)]
pub struct AddonSubs {
    pub list: Vec<AddonSub>,
    /// Which playback the answers are for: a landing for another is dropped.
    pub gen: u32,
    /// The viewer chose a subtitle in the menu: nothing is picked for them any more.
    pub chosen: bool,
    /// An answer landed since the last pick.
    due: bool,
    /// The next [`AddonSub::key`].
    next_key: u32,
}

impl AddonSubs {
    /// A new playback: nothing listed, nothing chosen, and earlier answers no longer land.
    pub fn reset(&mut self) {
        // keys carry on: a menu left open over the next playback cannot name one of its entries
        *self = AddonSubs { gen: self.gen.wrapping_add(1), next_key: self.next_key, ..AddonSubs::default() };
    }

    /// `addon`'s answer for playback `gen`, merged in (`prefer`: the viewer's subtitle languages,
    /// as `language_code` codes). `false` when it was for another playback.
    pub fn land(&mut self, gen: u32, addon: &str, subs: Vec<Subtitle>, prefer: &[String]) -> bool {
        if gen != self.gen {
            return false;
        }
        for s in subs {
            let code = language_code(&s.lang);
            let dup = self.list.iter().any(|o| o.url == s.url || (!s.id.is_empty() && o.addon == addon && o.id == s.id));
            let full = self.list.iter().filter(|o| o.addon == addon && language_code(&o.lang) == code).count() >= PER_LANGUAGE;
            if !dup && !full {
                let (release, forced, hearing_impaired) = (s.release().unwrap_or_default().to_string(), s.forced(), s.hearing_impaired());
                let key = self.next_key;
                self.next_key += 1;
                self.list.push(AddonSub { lang: s.lang, addon: addon.to_string(), id: s.id, url: s.url, key, release, forced, hearing_impaired });
            }
        }
        let rank = |s: &AddonSub| {
            let code = language_code(&s.lang);
            prefer.iter().position(|p| *p == code).unwrap_or(prefer.len())
        };
        self.list.sort_by_key(rank); // stable: arrival order within a rank
        self.due = true;
        true
    }

    /// What the menu lists.
    pub fn facts(&self) -> Vec<AddonSubFacts> {
        self.list
            .iter()
            .map(|s| AddonSubFacts {
                language: s.lang.clone(),
                addon: s.addon.clone(),
                key: s.key,
                release: s.release.clone(),
                forced: s.forced,
                hearing_impaired: s.hearing_impaired,
            })
            .collect()
    }

    /// The place of the subtitle the menu names `key`.
    pub fn position_of_key(&self, key: u32) -> Option<usize> {
        self.list.iter().position(|s| s.key == key)
    }

    /// The place of the subtitle at `url` in the list.
    pub fn position(&self, url: &str) -> Option<usize> {
        self.list.iter().position(|s| s.url == url)
    }

    /// **The late pick**: when an answer has landed since the last one and the viewer has not
    /// chosen, the addon subtitle `prefs` pick over audio in `audio_tag` from the file's `embedded`
    /// tracks and the list — `None` when the file's own track (or none) is the pick, which the
    /// probe already applied.
    pub fn pick(&mut self, prefs: &TrackPrefs, audio_tag: &str, embedded: &[Track<'_>]) -> Option<usize> {
        if !std::mem::take(&mut self.due) || self.chosen {
            return None;
        }
        let addon: Vec<Track<'_>> = self.list.iter().map(|s| Track { language: &s.lang, forced: false, default: false }).collect();
        match prefs.subtitle_among(audio_tag, embedded, &addon) {
            Some(SubPick::Addon(j)) => Some(j),
            _ => None,
        }
    }

    /// Answers landed that the pick has not seen yet.
    pub fn due(&self) -> bool {
        self.due && !self.chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_base::tracks::SubsMode;

    fn sub(id: &str, lang: &str) -> Subtitle {
        Subtitle { id: id.into(), url: format!("https://dl.example/{id}.srt"), lang: lang.into(), ..Subtitle::default() }
    }

    fn codes(l: &[&str]) -> Vec<String> {
        l.iter().map(|c| (*c).to_string()).collect()
    }

    /// The menu's list: duplicates dropped, a few per language, the viewer's languages first in
    /// their order, the rest as they came; a landing for another playback is dropped.
    #[test]
    fn answers_merge_deduped_capped_and_in_the_viewers_order() {
        let mut s = AddonSubs::default();
        s.reset();
        let prefer = codes(&["DE", "EN"]);
        let first = vec![sub("1", "eng"), sub("2", "fre"), sub("3", "ger"), sub("1", "eng"), sub("4", "eng"), sub("5", "eng"), sub("6", "eng")];
        assert!(s.land(s.gen, "OpenSubtitles", first, &prefer));
        let mut again = sub("9", "ger");
        again.url = "https://dl.example/3.srt".into();
        let mut sdh = sub("10", "eng");
        sdh.file_name = Some("Film.2014.SDH.srt".into());
        sdh.release_name = Some("Film.2014.1080p".into());
        assert!(s.land(s.gen, "Other", vec![again, sub("7", "de"), sub("8", "spa"), sdh], &prefer));
        let listed: Vec<(&str, &str)> = s.list.iter().map(|x| (x.id.as_str(), x.addon.as_str())).collect();
        assert_eq!(
            listed,
            [
                ("3", "OpenSubtitles"),
                ("7", "Other"),
                ("1", "OpenSubtitles"),
                ("4", "OpenSubtitles"),
                ("5", "OpenSubtitles"),
                ("10", "Other"),
                ("2", "OpenSubtitles"),
                ("8", "Other")
            ],
            "German, English (three of OpenSubtitles' four, and the other addon's own), then French and Spanish as they came; the same file from another addon once"
        );
        assert_eq!(s.facts()[0], AddonSubFacts { language: "ger".into(), addon: "OpenSubtitles".into(), key: 2, ..AddonSubFacts::default() });
        let f = &s.facts()[5];
        assert_eq!((f.release.as_str(), f.hearing_impaired, f.forced), ("Film.2014.1080p", true, false));
        assert_eq!(s.position("https://dl.example/7.srt"), Some(1));
        let keys: Vec<u32> = s.list.iter().map(|x| x.key).collect();
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "every entry its own key");
        assert_eq!(s.position_of_key(s.list[6].key), Some(6), "a key names its entry wherever the sort put it");
        let stale = s.gen.wrapping_sub(1);
        assert!(!s.land(stale, "Late", vec![sub("x", "eng")], &prefer), "an earlier playback's answer");
        assert!(!format!("{:?}", s.list[0]).contains("dl.example"), "the Debug never shows a URL");
        s.reset();
        assert!(s.list.is_empty() && !s.chosen);
    }

    fn prefs(subs: &[&str], mode: SubsMode) -> TrackPrefs {
        TrackPrefs { audio: codes(&["EN"]), subtitles: codes(subs), mode, native: "EN".into(), forced_fallback: false }
    }

    /// Addon answers arrive after the playback has started: the pick runs on each landing. The
    /// file's track in a preferred language wins; an addon's in a preferred language beats none,
    /// and beats a file track nobody asked for; an addon's is never a mere fallback.
    #[test]
    fn a_late_answer_is_picked_by_the_same_rule_as_the_files_tracks() {
        let on = prefs(&["DE"], SubsMode::On);
        let embedded_de = [Track { language: "ger", forced: false, default: false }];
        let embedded_fr = [Track { language: "fre", forced: false, default: true }];
        let mut s = AddonSubs::default();
        assert_eq!(s.pick(&on, "eng", &embedded_fr), None, "nothing landed: nothing to pick");
        s.land(s.gen, "Subs", vec![sub("1", "eng"), sub("2", "ger")], &on.subtitles);
        assert_eq!(s.list[0].id, "2", "German first");
        assert_eq!(s.pick(&on, "eng", &embedded_de), None, "the file's German track stands");
        s.land(s.gen, "More", vec![], &on.subtitles);
        assert_eq!(s.pick(&on, "eng", &embedded_fr), Some(0), "an addon's German beats the file's French");
        assert_eq!(s.pick(&on, "eng", &embedded_fr), None, "once per landing");
        s.land(s.gen, "More", vec![], &on.subtitles);
        assert_eq!(s.pick(&on, "eng", &[]), Some(0), "an addon's beats none");
        let mut other = AddonSubs::default();
        other.land(other.gen, "Subs", vec![sub("1", "spa")], &on.subtitles);
        assert_eq!(other.pick(&on, "eng", &[]), None, "a language nobody asked for is not picked");
        // Auto over audio in the understood language wants a forced track only: never an addon's
        let auto = prefs(&["EN"], SubsMode::Auto);
        other.land(other.gen, "Subs", vec![sub("2", "eng")], &auto.subtitles);
        assert_eq!(other.pick(&auto, "eng", &[]), None);
        assert_eq!(other.pick(&prefs(&["EN"], SubsMode::Off), "eng", &[]), None);
    }

    /// A choice in the menu is the viewer's for the rest of the playback.
    #[test]
    fn a_menu_choice_is_never_overridden_by_a_late_answer() {
        let on = prefs(&["DE"], SubsMode::On);
        let mut s = AddonSubs::default();
        s.chosen = true;
        s.land(s.gen, "Subs", vec![sub("1", "ger")], &on.subtitles);
        assert!(!s.due());
        assert_eq!(s.pick(&on, "eng", &[]), None);
    }
}
