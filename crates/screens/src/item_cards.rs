//! **Library items as cards** — the `sn_ui::cards::CardSource` Home's Continue Watching row, the
//! Continue Watching page and the Library page's type shelves draw, what a poster tile reads of a
//! library item, and where a Continue Watching press goes ([`resume_arg`]).
//!
//! Not a screen: the two screens share it here because neither may name the other. A card is a
//! poster with its resume bar while the item is in progress (`LibraryItem::progress`), titled
//! with the item's name and, for a series in progress, the episode it stopped on.

use std::collections::HashMap;

use sn_stremio::library::LibraryItem;
use sn_ui::cards::{CardSource, TileLabel};
use sn_ui::tile::{Resume, TileFacts, TileKind};
use sn_ui::widgets::Art;

use sn_machine::machine::Cx;

use crate::registry::{AppArg, Bundle, ItemRef, VideoRef};
use crate::stores::account::SessionState;

/// The Continue Watching items: signed in, in progress, most recent first (Home's shelf and the
/// Continue Watching page it opens).
pub fn continue_items<'a>(cx: &Cx<'a, Bundle>) -> Vec<&'a LibraryItem> {
    match cx.views.account.session() {
        SessionState::SignedIn(_) => cx.views.library.continue_watching(),
        _ => Vec::new(),
    }
}

/// Where a Continue Watching press goes: the resume target's Streams page, with the offset (for a
/// series the episode it stopped on), or the Resume page for an item with no recorded video.
pub fn resume_arg(item: &LibraryItem, from: &str) -> AppArg {
    match item.resume_target() {
        Some(t) => AppArg::Streams(VideoRef {
            kind: t.kind,
            video_id: t.video_id,
            show: item.name.clone(),
            title: caption(item),
            resume_ms: t.offset_ms,
            item_id: item.id.clone(),
            from: from.to_string(),
        }),
        None => AppArg::Resume(ItemRef { kind: item.kind.clone(), id: item.id.clone(), name: item.name.clone(), from: from.to_string() }),
    }
}

/// A held OK on a Continue Watching card or row: the item's menu.
pub fn item_menu(item: &LibraryItem, from: &str) -> crate::registry::MenuKind {
    crate::registry::MenuKind::Item {
        item: ItemRef { kind: item.kind.clone(), id: item.id.clone(), name: item.name.clone(), from: from.to_string() },
        poster: item.poster.clone(),
    }
}

/// What a poster tile reads of a library item: its poster and, in progress, its resume bar.
pub fn tile_facts(item: &LibraryItem) -> TileFacts<'_> {
    let resume = item.is_continue_watching().then(|| Resume {
        frac: item.progress(),
        left_ms: i64::try_from(item.state.duration.saturating_sub(item.state.time_offset)).unwrap_or(i64::MAX),
    });
    TileFacts {
        owner: std::ptr::from_ref(item) as usize,
        src: 0,
        kind: match item.kind.as_str() {
            "movie" => TileKind::Movie,
            "series" => TileKind::Show,
            _ => TileKind::Other,
        },
        title: &item.name,
        thumb: crate::cards::art_url(item.poster.as_deref()).unwrap_or(""),
        resume,
        watched: item.state.times_watched > 0 && !item.is_continue_watching(),
        ..Default::default()
    }
}

/// Library items interned to card elements by item id, from `base` up: an element never
/// changes item, and two screens' (or one screen's two) internings never share an element.
pub struct Interned {
    by_id: HashMap<String, u32>,
    next: u32,
}

impl Interned {
    pub fn with_base(base: u32) -> Self {
        Interned { by_id: HashMap::new(), next: base }
    }
    pub fn elem(&mut self, id: &str) -> u32 {
        if let Some(e) = self.by_id.get(id) {
            return *e;
        }
        let e = self.next;
        self.next += 1;
        self.by_id.insert(id.to_string(), e);
        e
    }
    pub fn get(&self, id: &str) -> Option<u32> {
        self.by_id.get(id).copied()
    }
    pub fn contains(&self, elem: u32) -> bool {
        self.by_id.values().any(|e| *e == elem)
    }
    pub fn len(&self) -> usize {
        self.by_id.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// A shelf of library items.
pub struct ItemCards<'a> {
    pub items: Vec<&'a LibraryItem>,
    pub cards: &'a Interned,
}

impl CardSource<Bundle> for ItemCards<'_> {
    fn len(&self) -> usize {
        self.items.iter().take_while(|i| self.cards.get(&i.id).is_some()).count()
    }
    fn elem(&self, i: usize) -> u32 {
        self.cards.get(&self.items[i].id).unwrap_or(0)
    }
    fn index_of(&self, e: &u32) -> Option<usize> {
        self.items.iter().position(|i| self.cards.get(&i.id) == Some(*e)).filter(|i| *i < self.len())
    }
    fn art(&self, i: usize) -> Art<'_> {
        Art::Poster(self.items.get(i).map(|item| tile_facts(item)))
    }
    fn label(&self, i: usize) -> TileLabel {
        match self.items.get(i) {
            Some(item) => TileLabel::titled(&item.name, &caption(item)),
            None => TileLabel::default(),
        }
    }
    fn progress(&self, i: usize) -> Option<f32> {
        self.items.get(i).filter(|i| i.is_continue_watching()).map(|i| i.progress())
    }
    fn more(&self) -> bool {
        false
    }
}

/// A card's sub-line: a series' episode address while one is in progress; nothing otherwise.
/// The video id is the item id plus `:{season}:{episode}` (`tt0903747:2:5` → "S2, E5") or, from
/// anime addons, plus `:{episode}` alone (`kitsu:8671:12` → "E12") — so what follows the ITEM id is
/// read, never the last two segments (that read `kitsu:8671:12` as season 8671).
pub fn caption(item: &LibraryItem) -> String {
    if !matches!(item.kind.as_str(), "series" | "anime") {
        return String::new();
    }
    let Some(rest) = item.state.video_id.as_deref().and_then(|v| v.strip_prefix(item.id.as_str())?.strip_prefix(':')) else {
        return String::new();
    };
    let nums: Option<Vec<i64>> = rest.split(':').map(|n| n.parse().ok()).collect();
    match nums.as_deref() {
        Some(&[season, episode]) => sn_ui::fmt::episode_ordinal(season, episode),
        Some(&[episode]) => sn_ui::fmt::episode_address(0, episode),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_card_names_its_episode_and_carries_its_progress() {
        let item: LibraryItem = serde_json::from_str(
            r#"{"_id":"tt1","type":"series","name":"Show","poster":"https://img.example/p.jpg",
                "state":{"timeOffset":600000,"duration":2400000,"video_id":"tt1:2:5"}}"#,
        )
        .unwrap();
        assert_eq!(caption(&item), sn_ui::fmt::episode_ordinal(2, 5));
        let facts = tile_facts(&item);
        assert_eq!(facts.thumb, "https://img.example/p.jpg");
        let r = facts.resume.expect("in progress");
        assert!((r.frac - 0.25).abs() < 1e-6);
        assert_eq!(r.left_ms, 1_800_000);
        let mut cards = Interned::with_base(0x10);
        assert_eq!(cards.elem("tt1"), 0x10);
        assert_eq!(cards.elem("tt1"), 0x10, "an item keeps its element");
        let shelf = ItemCards { items: vec![&item], cards: &cards };
        assert_eq!(CardSource::<Bundle>::progress(&shelf, 0), Some(0.25));
    }

    /// Real ids: Cinemeta's `{imdb}:{season}:{episode}` and the Kitsu addon's `kitsu:{id}:{episode}`
    /// (no season; its meta lists every episode as season 1).
    #[test]
    fn captions_read_the_episode_after_the_item_id_in_english() {
        let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        let item = |id: &str, video: &str| -> LibraryItem {
            serde_json::from_str(&format!(r#"{{"_id":"{id}","type":"series","state":{{"timeOffset":5,"video_id":"{video}"}}}}"#)).unwrap()
        };
        assert_eq!(caption(&item("tt0903747", "tt0903747:2:5")), "S2, E5");
        assert_eq!(caption(&item("kitsu:8671", "kitsu:8671:12")), "E12");
    }
}
