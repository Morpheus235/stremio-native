//! **Catalog cards** — the one `CardSource` over a run of catalog items that Search's rows, the
//! full catalog grid and Live TV share (Home keeps its own interning, because its rows land out of
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
        thumb: item.poster.as_deref().unwrap_or(""),
        ..Default::default()
    }
}

/// What a Detail page opens for a catalog item.
pub fn item_ref(item: &MetaPreview) -> ItemRef {
    ItemRef { kind: item.kind.clone(), id: item.id.clone(), name: item.name.clone() }
}

pub struct ItemCards<'a> {
    pub items: &'a [MetaPreview],
    pub base: u32,
    /// The catalog goes on past what is loaded.
    pub more: bool,
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
        Art::Poster(self.items.get(i).map(tile_facts))
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
