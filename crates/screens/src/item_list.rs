//! **List mode** — the table Search's results, the Library, a catalog grid, Home and the Continue
//! Watching page switch to from posters, and the *View* chip that switches them (each page's
//! choice is a setting, `SettingsStore::list`, `SettingsCmd::List`). Not a screen: the screens
//! compose it.
//!
//! A list is ONE `Custom` section per row in the page's `cards::Stack` (one focus element and one
//! group each), so the Stack's reveal scrolls row by row, UP/DOWN walk the rows by geometry, the
//! pointer hits through the stops the Stack registers for each row on screen, and a row off the
//! screen draws nothing — its thumbnail is never asked for, the poster grid's laziness. A row's
//! element IS the element its card has in poster mode, so OK, the hold and every item lookup are
//! the card's, and focus survives a switch between the two.
//!
//! A row is the catalog row the design system's table draws at the catalog measure
//! (`sn_ui::table::ROW_H_ART`, the accent pill and its ink when focused): a small poster, the
//! title, a sub-line (year · type · source) and the IMDb rating trailing. `TableView` itself owns
//! a selection and a scroll of its own and has no art slot, so it cannot be one of a Stack's
//! sections; the row takes its measure, its pill and its ink instead.

use std::ffi::CString;

use sn_machine::machine::{GroupId, Measure};
use sn_platform::i18n::msg;
use sn_stremio::library::LibraryItem;
use sn_stremio::types::MetaPreview;
use sn_ui::consts::{MARGIN_X, SCR_H, SCR_W};
use sn_ui::label::{HAlign, Label, VAlign};
use sn_ui::screen::{AxisMask, DrawFrame, EdgeRule, ElemKind, GroupKind, GroupSpec, Seat};
use sn_ui::tile::TileFacts;
use sn_ui::value_chip::ValueChip;
use sn_ui::widgets::Art;
use sn_ui::{theme, Env, Painter, Rect, View};

use crate::registry::Bundle;

/// A row's band in the flow (the table's catalog measure).
pub const ROW_H: f32 = sn_ui::table::ROW_H_ART;
/// The air between two rows' pills.
const ROW_GAP: f32 = 6.0;
const PAD: f32 = 20.0;
const THUMB_W: f32 = 54.0;
const THUMB_H: f32 = 81.0;
const THUMB_RAD: f32 = 6.0;
const TITLE_SIZE: i32 = theme::size::BODY;
const META_SIZE: i32 = theme::size::CAPTION;
const VALUE_SIZE: i32 = theme::size::LABEL;
/// Rows of look-ahead a paged list asks for its next page within (about a screenful).
pub const AHEAD: usize = 10;

/// What one row shows. Title, sub-line and value are server text (they may elide).
pub struct ListRow<'a> {
    pub art: TileFacts<'a>,
    pub title: &'a str,
    pub meta: String,
    pub value: Option<String>,
}

/// The sub-line's runs that are present, `·`-joined.
pub fn meta_line(parts: &[&str]) -> String {
    parts.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" \u{b7} ")
}

/// A type's word in a row: the type chips' label (localised for the known types).
fn kind_word(kind: &str) -> String {
    if kind.is_empty() {
        String::new()
    } else {
        crate::type_filter::label(kind)
    }
}

impl<'a> ListRow<'a> {
    /// A catalog item, with the catalog it came from where the page knows it.
    pub fn of_meta(item: &'a MetaPreview, source: Option<&str>) -> Self {
        let year = item.release_info.as_deref().unwrap_or("");
        ListRow {
            art: crate::cards::tile_facts(item),
            title: &item.name,
            meta: meta_line(&[year, &kind_word(&item.kind), source.unwrap_or("")]),
            value: item.imdb_rating.as_deref().map(str::trim).filter(|r| !r.is_empty()).map(|r| format!("IMDb {r}")),
        }
    }

    /// A live channel's on-air line (`cards::on_air`) in place of the year · type · source line.
    pub fn on_air(mut self, now: Option<String>) -> Self {
        if let Some(now) = now {
            self.meta = now;
        }
        self
    }

    /// A library item: its type and, for a series in progress, the episode it stopped on; in
    /// progress, the time left trails ("42 min left") and the thumbnail wears the resume bar.
    pub fn of_library(item: &'a LibraryItem) -> Self {
        let art = crate::item_cards::tile_facts(item);
        ListRow {
            art,
            title: &item.name,
            meta: meta_line(&[&kind_word(&item.kind), &crate::item_cards::caption(item)]),
            value: art.resume.filter(|_| item.state.duration > 0).map(|r| sn_ui::fmt::time_left(r.left_ms)),
        }
    }
}

/// The pill of a row whose section is `section` (screen space): the safe width, the gap shared
/// above and below. Also its focus rect and hit target.
pub fn face_rect(section: Rect) -> Rect {
    Rect::new(MARGIN_X, section.y + ROW_GAP * 0.5, SCR_W - 2.0 * MARGIN_X, ROW_H - ROW_GAP)
}

/// A row's focus group: one element, UP and DOWN to the neighbouring rows (or whatever is above
/// and below) by geometry, LEFT and RIGHT nowhere. A `Card` element, so a held OK arms the hold
/// exactly as on the card it stands for (the dispatcher arms a hold only on a card).
pub fn group(id: GroupId, extent: Rect) -> GroupSpec {
    GroupSpec {
        id,
        kind: GroupKind::Free,
        seat: Seat::First,
        reachable: AxisMask::BOTH,
        edge: [EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop],
        extent,
        len: 1,
        elem: ElemKind::Card,
    }
}

/// Whether a paged list of `len` rows should ask for its next page: focus is within [`AHEAD`]
/// rows of the end, or (nothing in the list focused) the whole list is shorter than that.
pub fn wants_more(focused: Option<usize>, len: usize) -> bool {
    focused.map_or(len < AHEAD, |i| i + AHEAD >= len)
}

/// Draw the row in `section` (screen space). A row off the screen draws, and asks for, nothing.
pub fn draw(f: &mut DrawFrame<'_, '_, Bundle>, section: Rect, row: &ListRow<'_>, focused: bool) {
    if section.y >= SCR_H || section.y + section.h <= 0.0 {
        return;
    }
    let p = f.painter.alpha(f.page_alpha);
    let face = face_rect(section);
    if focused {
        p.rrect(face, sn_ui::table::PILL_RAD, sn_ui::table::PILL_RAD, theme::FOCUS_FILL);
    }
    let (ink, sub) = if focused {
        (sn_ui::ACCENT_INK, theme::INK_ON_ACCENT_SECONDARY)
    } else {
        (theme::TEXT_PRIMARY, theme::TEXT_SECONDARY)
    };
    let thumb = Rect::new(face.x + PAD, face.y + (face.h - THUMB_H) * 0.5, THUMB_W, THUMB_H);
    sn_ui::widgets::card(p, thumb, Art::Poster(Some(row.art)), THUMB_RAD, false, 1.0, 0.0);
    if let Some(r) = row.art.resume {
        sn_ui::cards::resume_bar(p, thumb, r.frac, THUMB_RAD);
    }
    let x = thumb.x + THUMB_W + PAD;
    let mut right = face.x + face.w - PAD;
    if let Some(value) = &row.value {
        let w = f.measure.width_str(value, VALUE_SIZE, true);
        let c = cstring(value);
        Label::new(c.as_ptr(), VALUE_SIZE, sub).bold().h(HAlign::Right).v(VAlign::Middle).draw(p, Rect::new(right - w, face.y, w, face.h));
        right -= w + PAD;
    }
    let w = (right - x).max(0.0);
    let half = face.h * 0.5;
    let title = sn_gfx::text::elide_by(row.title, w, false, |t| f.measure.width_str(t, TITLE_SIZE, true));
    let c = cstring(&title);
    Label::new(c.as_ptr(), TITLE_SIZE, ink).bold().v(VAlign::Middle).draw(p, Rect::new(x, face.y + 8.0, w, half - 8.0));
    if !row.meta.is_empty() {
        let meta = sn_gfx::text::elide_by(&row.meta, w, false, |t| f.measure.width_str(t, META_SIZE, false));
        let c = cstring(&meta);
        Label::new(c.as_ptr(), META_SIZE, sub).v(VAlign::Middle).draw(p, Rect::new(x, face.y + half, w, half - 8.0));
    }
}

/// A row shown as a list ends in *See all*, the poster shelf's See all tile as a list row: the
/// row's pill when focused, the words where the titles stand. Nothing off the screen.
pub fn draw_see_all(f: &mut DrawFrame<'_, '_, Bundle>, section: Rect, focused: bool) {
    if section.y >= SCR_H || section.y + section.h <= 0.0 {
        return;
    }
    let p = f.painter.alpha(f.page_alpha);
    let face = face_rect(section);
    if focused {
        p.rrect(face, sn_ui::table::PILL_RAD, sn_ui::table::PILL_RAD, sn_ui::ACCENT);
    }
    let ink = if focused { sn_ui::ACCENT_INK } else { theme::TEXT_SECONDARY };
    let x = face.x + PAD + THUMB_W + PAD;
    let text = msg::widgets_cards_see_all_c();
    Label::new(text.as_ptr(), TITLE_SIZE, ink).bold().v(VAlign::Middle).draw(p, Rect::new(x, face.y, face.x + face.w - PAD - x, face.h));
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// The *View* chip's two runs for the mode shown (`list`).
fn toggle_text(list: bool) -> (CString, CString) {
    let value = if list { msg::app_view_list() } else { msg::app_view_posters() };
    (cstring(msg::app_view_name()), cstring(&format!(" \u{b7} {value}")))
}

/// The *View* chip's width: the wider of its two values, so it does not resize as it flips.
pub fn toggle_width(measure: &dyn Measure) -> f32 {
    [false, true]
        .map(|list| {
            let (name, value) = toggle_text(list);
            ValueChip::width(measure, &name, &value, None)
        })
        .into_iter()
        .fold(0.0, f32::max)
}

/// The *View* chip in `rect`, naming the mode shown; OK flips it.
pub fn draw_toggle(p: Painter, rect: Rect, list: bool, focused: bool) {
    let (name, value) = toggle_text(list);
    let icon = if list { sn_ui::icons::Icon::Check } else { sn_ui::icons::Icon::Chevron };
    ValueChip::new(&name, &value, None, rect).icon(icon).focused(focused).draw(&Env::inert(), p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

    #[test]
    fn a_catalog_row_reads_year_type_source_and_rating() {
        let _g = language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        let item = MetaPreview {
            id: "tt1".into(),
            kind: "movie".into(),
            name: "Heat".into(),
            release_info: Some("1995".into()),
            imdb_rating: Some("8.3".into()),
            poster: Some("https://img.example/p.jpg".into()),
            ..Default::default()
        };
        let row = ListRow::of_meta(&item, Some("Cinemeta"));
        assert_eq!((row.title, row.meta.as_str(), row.value.as_deref()), ("Heat", "1995 \u{b7} Movies \u{b7} Cinemeta", Some("IMDb 8.3")));
        assert_eq!(row.art.thumb, "https://img.example/p.jpg");
        let bare = MetaPreview { id: "x".into(), name: "X".into(), imdb_rating: Some(" ".into()), ..Default::default() };
        let row = ListRow::of_meta(&bare, None);
        assert_eq!((row.meta.as_str(), row.value), ("", None), "absent facts leave no separators and no rating");
    }

    #[test]
    fn a_paged_list_asks_within_a_screenful_of_its_end() {
        assert!(!wants_more(Some(0), 100));
        assert!(wants_more(Some(90), 100));
        assert!(wants_more(None, 3), "a short list with nothing focused fills itself");
        assert!(!wants_more(None, 100));
    }

    /// The chip is app text: both its values fit a chip of a quarter of the safe width, unelided,
    /// in every shipped language.
    #[test]
    fn the_view_chip_fits_in_every_shipped_language() {
        let m = sn_base::fontcov::advances::ShippedMeasure;
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let w = toggle_width(&m);
            assert!(w <= (SCR_W - 2.0 * MARGIN_X) / 4.0, "{}: the view chip is {w}px", language.tag());
        }
    }
}
