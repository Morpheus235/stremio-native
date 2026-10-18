//! **The type filter** — what Home's and the Library's type dropdown (`crate::dropdown`) offers to
//! narrow the page to one content type (Movies, Series, Anime, Channels, TV …), like Stremio's own
//! type selector.
//!
//! - **The options** are the types actually present ([`chips`]): *All* first, then Movies, Series,
//!   then the rest alphabetically by label. Home reads its rows' types, the Library its addons'
//!   catalog types and its items'. With fewer than two types there is nothing to choose and no
//!   dropdown is shown.
//! - **Labels**: a known type is a localised word (`registry::kind_label`), any other its type
//!   string title-cased ([`label`]).
//! - **Memory**: the choice is kept per page for the session ([`remembered`]), never on disk;
//!   Discover keeps its sub-tab here too.

use std::cell::RefCell;

use sn_platform::i18n::msg;
use sn_ui::theme;

use crate::registry::kind_label;

/// The *All* chip's type: no filter.
pub const ALL: &str = "";
/// A chip's height (the top bar's pill, as Search's chips).
pub const CHIP_H: f32 = sn_ui::widgets::TAB_PILL_H;
/// The chip row's band: the chips and the air under them.
pub const BAND: f32 = CHIP_H + theme::space::LG;

/// The pages that keep a choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    Library,
    /// Discover's sub-tab (its type row).
    Discover,
}

// ponytail: the frame thread's own session memory (every screen runs on it); a restart forgets
// the choice, which is what "for the session" means. A per-profile saved choice would be a store.
thread_local! {
    static CHOSEN: RefCell<[String; 3]> = const { RefCell::new([String::new(), String::new(), String::new()]) };
}

/// The type `page` was last narrowed to this session ([`ALL`] when none).
pub fn remembered(page: Page) -> String {
    CHOSEN.with(|c| c.borrow()[page as usize].clone())
}

/// Keep `kind` as `page`'s choice for the session (what a chip's OK does; Discover opened on its
/// Live TV sub-tab sets it too).
pub fn remember(page: Page, kind: &str) {
    CHOSEN.with(|c| c.borrow_mut()[page as usize] = kind.to_string());
}

/// A type's chip label: *All*, a known type's localised word, or the type title-cased.
pub fn label(kind: &str) -> String {
    if kind == ALL {
        return msg::app_filter_all().to_string();
    }
    if let Some(l) = kind_label(kind) {
        return l.to_string();
    }
    kind.split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The chips for the types present: *All*, Movies, Series, then the rest alphabetically by label;
/// each type once. Empty when fewer than two types are present (nothing to choose).
pub fn chips<'a>(kinds: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let rank = |k: &str| match k {
        "movie" => 0,
        "series" => 1,
        _ => 2,
    };
    let mut v: Vec<(u8, String, String)> =
        kinds.into_iter().filter(|k| !k.is_empty()).map(|k| (rank(k), label(k).to_lowercase(), k.to_string())).collect();
    v.sort();
    v.dedup();
    if v.len() < 2 {
        return Vec::new();
    }
    std::iter::once(ALL.to_string()).chain(v.into_iter().map(|(_, _, k)| k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

    #[test]
    fn the_chips_are_all_then_movies_series_then_the_rest_by_label_each_once() {
        let kinds = ["tv", "series", "anime", "movie", "series", "channel", "movie", "other_stuff", ""];
        let got = chips(kinds);
        assert_eq!(got, ["", "movie", "series", "anime", "channel", "other_stuff", "tv"].map(String::from));
        assert_eq!(chips(["movie", "movie"]), Vec::<String>::new(), "one type: nothing to choose");
        assert_eq!(chips(["series", "tv"]), ["", "series", "tv"].map(String::from), "no movies: series still leads");
    }

    #[test]
    fn known_types_are_localised_and_unknown_ones_title_cased() {
        let _g = language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        assert_eq!(label("movie"), "Movies");
        assert_eq!(label("series"), "Series");
        assert_eq!(label("anime"), "Anime");
        assert_eq!(label("channel"), "Channels");
        assert_eq!(label("tv"), "TV");
        assert_eq!(label(ALL), "All");
        assert_eq!(label("podcasts"), "Podcasts");
        assert_eq!(label("music_video"), "Music Video");
    }

    /// Every type's dropdown label fits its button uncapped, in every shipped language.
    #[test]
    fn the_type_labels_fit_in_every_shipped_language() {
        let m = sn_base::fontcov::advances::ShippedMeasure;
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            for kind in ["movie", "series", "anime", "channel", "tv", ALL] {
                let w = crate::dropdown::face_w(&label(kind), &m);
                assert!(w < crate::dropdown::BUTTON_W_CAP, "{}: {:?} is {w}px", language.tag(), label(kind));
            }
        }
    }
}
