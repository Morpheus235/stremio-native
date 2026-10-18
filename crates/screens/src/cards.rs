//! **Catalog cards** — the one `CardSource` over a run of catalog items that Search's rows, the
//! full catalog grid and Discover share (Home keeps its own interning, because its rows land out of
//! order across addons). Not a screen: a composition every card page reads.
//!
//! An element is `base + index`. A catalog view only ever appends (`Pager` keeps what loaded), so
//! an index names the same item for as long as the view lives; a page that swaps the view (a new
//! query, another genre) swaps the section key and its `base` with it.

use sn_stremio::types::MetaPreview;
use sn_ui::cards::{CardSource, TileLabel};
use sn_ui::tile::{TileFacts, TileKind};
use sn_ui::widgets::Art;

use crate::registry::{Bundle, ItemRef};

/// `url` if it is one an image can be fetched from: an `http(s)` address, trimmed. An addon's
/// blank, `"N/A"` or relative "poster" is no poster, so the tile draws its title fallback at once
/// instead of asking for it.
pub fn art_url(url: Option<&str>) -> Option<&str> {
    let u = url?.trim();
    let lower = u.get(..8).unwrap_or(u).to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://")).then_some(u)
}

/// The picture a catalog item's poster tile shows: its poster, else its background (cover-cropped
/// to the poster's shape, the same key a detail page's backdrop is fetched under at another
/// size). No usable picture is `""`: the title fallback. A logo is not used, since cover-cropped it
/// would cut its wordmark.
pub fn poster_of(item: &MetaPreview) -> &str {
    art_url(item.poster.as_deref()).or_else(|| art_url(item.background.as_deref())).unwrap_or("")
}

/// What a poster tile reads of a catalog item.
pub fn tile_facts(item: &MetaPreview) -> TileFacts<'_> {
    TileFacts {
        owner: std::ptr::from_ref(item) as usize,
        src: 0,
        kind: match item.kind.as_str() {
            "movie" => TileKind::Movie,
            "series" => TileKind::Show,
            _ => TileKind::Other,
        },
        title: &item.name,
        thumb: poster_of(item),
        ..Default::default()
    }
}

/// What a Detail page opens for a catalog item, opened from the page named `from` (its crumb).
pub fn item_ref(item: &MetaPreview, from: &str) -> ItemRef {
    ItemRef { kind: item.kind.clone(), id: item.id.clone(), name: item.name.clone(), from: from.to_string() }
}

/// A card's item menu content: the item, and its poster for a new library entry; its *Open*
/// names `from` as the Detail page's crumb.
pub fn item_menu(item: &MetaPreview, from: &str) -> crate::registry::MenuKind {
    crate::registry::MenuKind::Item { item: item_ref(item, from), poster: item.poster.clone() }
}

pub struct ItemCards<'a> {
    pub items: &'a [MetaPreview],
    pub base: u32,
    /// The catalog goes on past what is loaded.
    pub more: bool,
    /// The library whose watched items wear the tick (stremio-web's Discover: an item the account
    /// holds with `timesWatched > 0`); `None`: no ticks.
    pub watched: Option<&'a crate::stores::library::LibraryStore>,
}

impl ItemCards<'_> {
    /// The item an element names.
    pub fn item(&self, elem: u32) -> Option<&MetaPreview> {
        self.items.get(elem.checked_sub(self.base)? as usize)
    }
}

impl CardSource<Bundle> for ItemCards<'_> {
    fn len(&self) -> usize {
        self.items.len()
    }
    fn elem(&self, i: usize) -> u32 {
        self.base + i as u32
    }
    fn index_of(&self, e: &u32) -> Option<usize> {
        let i = e.checked_sub(self.base)? as usize;
        (i < self.items.len()).then_some(i)
    }
    fn art(&self, i: usize) -> Art<'_> {
        // ponytail: a linear find per drawn card; index the library by id if it grows past a few hundred
        let watched = |item: &MetaPreview| self.watched.and_then(|l| l.item(&item.id)).is_some_and(|i| i.watched());
        Art::Poster(self.items.get(i).map(|item| TileFacts { watched: watched(item), ..tile_facts(item) }))
    }
    fn label(&self, i: usize) -> TileLabel {
        match self.items.get(i) {
            Some(item) => TileLabel::titled(&item.name, item.release_info.as_deref().unwrap_or("")),
            None => TileLabel::default(),
        }
    }
    fn more(&self) -> bool {
        self.more
    }
    fn holdable(&self, _i: usize) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(json: &str) -> MetaPreview {
        serde_json::from_str(json).unwrap()
    }

    /// A poster tile shows the poster, else the background; an unusable "poster" (blank, "N/A",
    /// relative) is none, so the tile is its title fallback rather than a fetch that cannot work.
    #[test]
    fn a_tile_reads_the_poster_else_the_background_else_nothing() {
        let both = meta(r#"{"id":"tt1","type":"movie","name":"A","poster":" https://img.example/p.jpg ","background":"https://img.example/b.jpg"}"#);
        assert_eq!(tile_facts(&both).thumb, "https://img.example/p.jpg");
        let bg = meta(r#"{"id":"tt2","type":"movie","name":"B","poster":"N/A","background":"https://img.example/b.jpg","logo":"https://img.example/l.png"}"#);
        assert_eq!(tile_facts(&bg).thumb, "https://img.example/b.jpg");
        let none = meta(r#"{"id":"tt3","type":"movie","name":"C","poster":"/poster.jpg","logo":"https://img.example/l.png"}"#);
        assert_eq!(tile_facts(&none).thumb, "", "no picture: the title fallback");
        assert_eq!(tile_facts(&none).title, "C");
        assert_eq!(art_url(Some("HTTPS://img.example/x.jpg")), Some("HTTPS://img.example/x.jpg"));
        assert_eq!(art_url(Some("")), None);
        assert_eq!(art_url(None), None);
    }
}
