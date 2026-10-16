//! **Library items as cards** — the `sn_ui::cards::CardSource` Home's Continue Watching row and
//! the Library page's type shelves both draw, and what a poster tile reads of a library item.
//!
//! Not a screen: the two screens share it here because neither may name the other. A card is a
//! poster with its resume bar while the item is in progress (`LibraryItem::progress`), titled
//! with the item's name and, for a series in progress, the episode it stopped on.

use std::collections::HashMap;

use sn_stremio::library::LibraryItem;
use sn_ui::cards::{CardSource, TileLabel};
use sn_ui::tile::{Resume, TileFacts, TileKind};
use sn_ui::widgets::Art;

use crate::registry::Bundle;

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
        thumb: item.poster.as_deref().unwrap_or(""),
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

/// A card's sub-line: a series' episode address (`tt0903747:2:5` → "S2 · E5") while one is in
/// progress; nothing otherwise.
pub fn caption(item: &LibraryItem) -> String {
    if item.kind != "series" {
        return String::new();
    }
    let Some(video) = item.state.video_id.as_deref() else { return String::new() };
    let mut parts = video.rsplit(':');
    match (parts.next().and_then(|e| e.parse::<i64>().ok()), parts.next().and_then(|s| s.parse::<i64>().ok())) {
        (Some(e), Some(s)) => sn_platform::i18n::msg::core_episode_ordinal(s, e),
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
        assert_eq!(caption(&item), sn_platform::i18n::msg::core_episode_ordinal(2, 5));
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
}
