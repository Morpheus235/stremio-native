//! The subtitle menu's model: the groups and their order, each group's subtitles, where the menu
//! opens, identity across a late answer, the row keys, and the columns' fit in every shipped
//! language. The remote and the pointer are driven through the real dispatcher in
//! `super::super::tests`.

use super::*;
use crate::registry::TrackFacts;
use sn_ui::form::FormTable;

fn track(lang: &str, name: &str) -> TrackFacts {
    TrackFacts { language: lang.into(), name: name.into(), ..TrackFacts::default() }
}

fn sub(language: &str, addon: &str, key: u32) -> AddonSubFacts {
    AddonSubFacts { language: language.into(), addon: addon.into(), key, ..AddonSubFacts::default() }
}

fn codes(l: &[&str]) -> Vec<String> {
    l.iter().map(|c| (*c).to_string()).collect()
}

/// The file's English (SDH), untagged and French tracks; OpenSubtitles' German and English, a
/// second addon's Spanish and untagged, SubHero's German and English.
fn media() -> MediaFacts {
    let mut forced = sub("eng", "OpenSubtitles", 1);
    forced.forced = true;
    forced.release = "Film.2014.1080p.BluRay".into();
    let mut sdh = sub("en", "SubHero", 5);
    sdh.hearing_impaired = true;
    MediaFacts {
        subs: vec![track("eng", "SDH"), track("", ""), track("fre", "")],
        addon_subs: vec![sub("ger", "OpenSubtitles", 0), forced, sub("spa", "Subs Two", 2), sub("", "Subs Two", 3), sub("deu", "SubHero", 4), sdh],
        ..MediaFacts::NONE
    }
}

fn lang(c: &str) -> Group {
    Group::Lang(c.into())
}

#[test]
fn groups_are_off_embedded_the_viewers_languages_then_the_rest_by_name() {
    let _g = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let rows = groups(&media(), &codes(&["DE", "EN"]));
    let got: Vec<(Group, &str, usize)> = rows.iter().map(|r| (r.group.clone(), r.label.as_str(), r.count)).collect();
    assert_eq!(
        got,
        [
            (Group::Off, "Off", 0),
            (Group::Embedded, "Embedded", 3),
            (lang("DE"), "Deutsch", 2),
            (lang("EN"), "English", 3),
            (lang("ES"), "Español", 1),
            (lang("FR"), "Français", 1),
            (Group::Other, "Other", 1),
        ],
        "ger and deu are one language; the file's untagged track is only Embedded's"
    );
    let bare = groups(&MediaFacts { subs: vec![], ..media() }, &[]);
    assert!(!bare.iter().any(|r| r.group == Group::Embedded), "no file tracks: no Embedded group");
    assert_eq!(bare[1].group, lang("DE"), "no preference: by name");
}

#[test]
fn a_languages_subtitles_are_the_files_first_then_addon_by_addon() {
    let m = media();
    assert_eq!(variants(&m, &lang("EN")), [Variant::Embedded(0), Variant::Addon(1), Variant::Addon(5)]);
    assert_eq!(variants(&m, &lang("DE")), [Variant::Addon(0), Variant::Addon(4)], "OpenSubtitles answered first");
    assert_eq!(variants(&m, &Group::Embedded), [Variant::Embedded(0), Variant::Embedded(1), Variant::Embedded(2)]);
    assert_eq!(variants(&m, &Group::Other), [Variant::Addon(3)]);
    assert!(variants(&m, &Group::Off).is_empty());
}

/// "Auto open original / embedded": the group in use; none in use, the file's own; no file
/// tracks, the viewer's first language on offer.
#[test]
fn the_menu_opens_on_the_subtitle_in_use_else_the_embedded_else_a_preferred_language() {
    let prefer = codes(&["EN", "DE"]);
    let m = media();
    assert_eq!(opening(&m, &prefer), Group::Embedded, "nothing in use, the file has tracks");
    assert_eq!(opening(&MediaFacts { sub_sel: Some(2), ..media() }, &prefer), Group::Embedded, "a file track in use");
    assert_eq!(opening(&MediaFacts { sub_sel: Some(3 + 4), ..media() }, &prefer), lang("DE"), "SubHero's German in use");
    assert_eq!(opening(&MediaFacts { sub_sel: Some(3 + 3), ..media() }, &prefer), Group::Other);
    let no_tracks = MediaFacts { subs: vec![], ..media() };
    assert_eq!(opening(&no_tracks, &prefer), lang("EN"), "the viewer's first language on offer");
    assert_eq!(opening(&no_tracks, &codes(&["IT", "DE"])), lang("DE"), "Italian is not on offer: the next one");
    assert_eq!(opening(&no_tracks, &codes(&["IT"])), lang("DE"), "none of the viewer's: the first language");
    assert_eq!(opening(&MediaFacts::NONE, &prefer), Group::Off);
    assert_eq!(current(&MediaFacts { sub_sel: Some(3 + 1), ..media() }), Some(Variant::Addon(1)));
}

/// An answer landing while the menu is open re-sorts the addons' list; the focused and the
/// checked subtitle stay the same subtitle because rows are keyed by identity.
#[test]
fn a_selection_survives_a_late_answer() {
    let _g = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let in_use = MediaFacts { sub_sel: Some(3 + 4), ..media() };
    let mut right = FormTable::<Variant, Variant, Infallible>::new(RIGHT_BAND);
    right.open(right_form(&in_use, &lang("DE"), "Deutsch"), Some(&Variant::Addon(4)));
    assert_eq!(right.selected_id(), Some(&Variant::Addon(4)));
    // a third addon's German lands ahead of SubHero's: the list and the index in use both move
    let mut late = in_use.clone();
    late.addon_subs.insert(0, sub("ger", "Late", 9));
    late.sub_sel = Some(3 + 5);
    assert_eq!(current(&late), Some(Variant::Addon(4)), "the app re-points the index; the key is the same");
    right.set_sliding(right_form(&late, &lang("DE"), "Deutsch"), right.selected_id().copied().as_ref());
    assert_eq!(right.selected_id(), Some(&Variant::Addon(4)), "the focus stays on SubHero's");
    let checked: Vec<bool> = right.table.sections[0].rows.iter().map(|r| r.checked).collect();
    let at = right.index_of(&Variant::Addon(4)).unwrap();
    assert!(checked[at] && checked.iter().filter(|c| **c).count() == 1, "and so does the check");
    let mut left = FormTable::<Group, Group, Infallible>::new(LEFT_BAND);
    left.open(left_form(&in_use, &[]), Some(&lang("DE")));
    left.set_sliding(left_form(&late, &[]), left.selected_id().cloned().as_ref());
    assert_eq!(left.selected_id(), Some(&lang("DE")));
}

#[test]
fn a_subtitle_row_names_its_addon_release_and_marks() {
    let _g = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut t = FormTable::<Variant, Variant, Infallible>::new(RIGHT_BAND);
    t.set(right_form(&media(), &lang("EN"), "English"), None);
    let rows: Vec<(&str, &str, &str)> = t.table.sections[0].rows.iter().map(|r| (r.label.as_str(), r.detail.as_str(), r.value.as_deref().unwrap_or(""))).collect();
    assert_eq!(
        rows,
        [("English \u{b7} SDH", "", "Embedded"), ("OpenSubtitles", "Film.2014.1080p.BluRay", "Forced"), ("SubHero", "", "SDH")]
    );
    t.set(right_form(&media(), &Group::Embedded, "Embedded"), None);
    assert_eq!(t.table.sections[0].rows[1].label, "Track 2", "an untagged file track by its place");
    assert!(t.table.sections[0].rows.iter().all(|r| r.value.is_none()), "the Embedded group does not repeat its own name");
}

#[test]
fn every_row_key_is_unique_and_in_its_columns_band() {
    let mut codes: Vec<String> = sn_base::tracks::LANGUAGES.iter().flat_map(|(short, long, _)| std::iter::once(*short).chain(long.iter().copied())).map(str::to_ascii_uppercase).collect();
    codes.push("ZZZ".into());
    codes.sort();
    codes.dedup();
    let mut keys: Vec<u32> = [Group::Off, Group::Embedded, Group::Other].iter().map(|g| g.key().0).collect();
    keys.extend(codes.iter().map(|c| lang(c).key().0));
    assert!(keys.iter().all(|k| *k < LEFT_BAND));
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), keys.len(), "one key per code");
    let right: Vec<u32> = (0..64).map(|i| Variant::Embedded(i).key().0).chain((0..64).map(|k| Variant::Addon(k).key().0)).collect();
    assert!(right.iter().all(|k| (LEFT_BAND..RIGHT_BAND).contains(k)));
    let mut unique = right.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), right.len());
}

/// The app's words in both columns — Off, Embedded, Other, the Timing tab with the longest delay, the Style tab,
/// the Languages header, the Embedded and Forced · SDH marks — fit their column in every shipped
/// language. Language, addon and release names are server text and may elide.
#[test]
fn both_columns_fit_in_every_shipped_language() {
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
    let mut marked = media();
    for s in &mut marked.addon_subs {
        s.forced = true;
        s.hearing_impaired = true;
    }
    let mut out = Vec::new();
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        let tag = language.tag();
        let mut left = FormTable::<Group, Group, Infallible>::new(LEFT_BAND);
        left.table.compact = true;
        left.set(left_form(&marked, &codes(&["DE"])), None);
        out.extend(left.table.app_fit_failures(theme::layout::SUBTITLE_MENU_GROUPS_W, &format!("{tag} groups")));
        // the Timing and Style tabs over both columns, the longest delay and every rung
        let both = theme::layout::SUBTITLE_MENU_GROUPS_W + theme::layout::SUBTITLE_MENU_VARIANTS_W;
        let mut opts = FormTable::<Opt, Opt, Infallible>::new(KEY_CEIL);
        opts.table.compact = true;
        opts.set(timing_form(-12_750), None);
        out.extend(opts.table.app_fit_failures(both, &format!("{tag} timing")));
        for p in STYLE_PREFS {
            for r in 0..p.rungs() {
                let mut settings = crate::stores::settings::SettingsStore::default();
                settings.set(p, r);
                opts.set(style_form(&settings), None);
                out.extend(opts.table.app_fit_failures(both, &format!("{tag} {p:?} {r}")));
            }
        }
        for g in [Group::Embedded, lang("EN"), lang("DE"), Group::Other] {
            let mut right = FormTable::<Variant, Variant, Infallible>::new(RIGHT_BAND);
            right.table.compact = true;
            right.set(right_form(&marked, &g, "Deutsch"), None);
            out.extend(right.table.app_fit_failures(theme::layout::SUBTITLE_MENU_VARIANTS_W, &format!("{tag} {g:?}")));
        }
    }
    sn_ui::table::assert_no_fit_failures(&out);
}

/// A Style row steps inside its ladder with LEFT/RIGHT and goes round with OK.
#[test]
fn a_style_step_stays_on_the_ladder_and_ok_goes_round() {
    let p = Pref::SubtitleSize;
    let last = p.rungs() - 1;
    assert_eq!(style_step(p, last, 1, false), last, "RIGHT stops at the last rung");
    assert_eq!(style_step(p, 0, -1, false), 0, "LEFT stops at the first");
    assert_eq!(style_step(p, last, 1, true), 0, "OK goes round");
    assert_eq!(style_step(p, 1, 1, false), 2);
}
