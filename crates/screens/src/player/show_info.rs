//! **The show info** — the `…` menu's *Show info* row: what plays, as its catalog tells it, in one
//! panel over the video. The show's name, the episode's address and title, the episode's overview
//! (else the show's description), a facts line (year · runtime · IMDb rating), the Genres, Cast
//! and Director chips (the detail page's own, `crate::detail::meta_links` laid by
//! `crate::detail::place_chips`, at most [`LINK_LINES`] lines each ending in `+N`; DISPLAY ONLY —
//! nothing here takes focus or leaves the player), the episode's still (else the poster) decoded small through the image
//! cache, and last one quiet technical line (`tracks::tech_line`); the stream's full technical
//! detail is Stats for nerds'.
//!
//! The panel reads the show's meta live off the detail store (the player holds it open), so it
//! fills in when the meta lands after it opened; until then it shows the player's own title. It
//! takes no focus: OK or BACK closes it, and playback goes on underneath.

use std::borrow::Cow;

use sn_machine::machine::{Canon, Cx, Edge, Effects, EntryId, FocusKey, GroupId, Handled, InputKind, Key, LogicalState, Machine, Measure, NavOp};
use sn_machine::machine::Fx;
use sn_stremio::types::Meta;
use sn_ui::consts::{MARGIN_X, SCR_W};
use sn_ui::screen::{At, Dir, DrawFrame, Focusable, GroupSpec, Placed, RenderStrategy, Scrim, Screen, ScreenEvent, Step};
use sn_ui::text_view::TextView;
use sn_ui::widgets::Art;
use sn_ui::{theme, Rect};

use crate::detail::{label_view, meta_links, place_chips, GroupPlace, LinkGroup, LINK_LINES};
use crate::registry::{word, Bundle, MenuArg, MenuKind, PlayerPanel, Views};
use crate::stores::detail::MetaState;

const PANEL_RAD: f32 = theme::radius::PANEL;
/// The panel's keep-out from the top of the screen.
const EDGE: f32 = theme::space::XL;
const PAD: f32 = theme::space::LG;
/// The art column: an episode's 16:9 still, or the 2:3 poster in the same column's width.
const ART_W: f32 = 320.0;
const STILL_H: f32 = ART_W * 9.0 / 16.0;
const POSTER_W: f32 = 200.0;
const POSTER_H: f32 = POSTER_W * 1.5;
const OVERVIEW_LINES: usize = 7;

/// What the panel says, built PURE from the meta (the host suite grades it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShowInfo {
    /// The show's (or the movie's) name.
    pub title: String,
    /// An episode's `S1 E5 · Title`; empty for a movie.
    pub episode: String,
    /// The episode's overview, else the show's description.
    pub overview: String,
    /// Year · runtime · IMDb rating, each when known.
    pub facts: String,
    /// The Genres, Cast and Director chips, each group when the meta names any (their targets
    /// are the detail page's; the panel never follows them).
    pub links: Vec<LinkGroup>,
    /// The picture: the episode's still (`true`), else the poster (`false`).
    pub art: Option<(String, bool)>,
}

/// What `meta` says about the video `video_id`; `title` stands in until the meta is in.
pub fn show_info(meta: Option<&Meta>, video_id: &str, title: &str) -> ShowInfo {
    let Some(meta) = meta else { return ShowInfo { title: title.to_string(), ..ShowInfo::default() } };
    let text = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let p = &meta.preview;
    let video = meta.videos.iter().find(|v| v.id == video_id && v.id != p.id);
    let episode = video.map_or_else(String::new, |v| {
        let address = crate::episodes::address(v.season.unwrap_or(0), v);
        match text(Some(&v.title)) {
            Some(t) => format!("{address} \u{b7} {t}"),
            None => address,
        }
    });
    let year = video
        .and_then(|v| v.released.as_deref())
        .and_then(|d| d.get(..4))
        .map(str::to_string)
        .or_else(|| text(p.release_info.as_deref()));
    // the genres are chips of their own, as on the detail page
    let facts = [year, text(meta.runtime.as_deref()), text(p.imdb_rating.as_deref()).map(|r| format!("IMDb {r}"))]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    let overview = video.and_then(|v| text(v.overview.as_deref())).or_else(|| text(p.description.as_deref())).unwrap_or_default();
    let art = video
        .and_then(|v| text(v.thumbnail.as_deref()))
        .map(|u| (u, true))
        .or_else(|| text(p.poster.as_deref()).map(|u| (u, false)));
    let links = meta_links(&p.kind, meta);
    ShowInfo { title: text(Some(&p.name)).unwrap_or_else(|| title.to_string()), episode, overview, facts, links, art }
}

/// One text block of the panel and where it goes.
struct Block<'a> {
    view: TextView<'a>,
    rect: Rect,
}

/// The panel's layout: its rect, the art's rect, the text blocks and the technical line's rect
/// with the hairline over it. Pure over `measure`.
struct Layout<'a> {
    panel: Rect,
    art: Option<Rect>,
    blocks: Vec<Block<'a>>,
    /// Each shown link group: its label and its chips.
    links: Vec<(Block<'a>, GroupPlace)>,
    tech: Option<Block<'a>>,
    rule_y: f32,
}

fn layout<'a>(info: &'a ShowInfo, tech: &'a str, measure: &'a dyn Measure) -> Layout<'a> {
    let w = theme::layout::PLAYER_INFO_W;
    let x = SCR_W - MARGIN_X - w;
    let (art_w, art_h) = match &info.art {
        Some((_, true)) => (ART_W, STILL_H),
        Some((_, false)) => (POSTER_W, POSTER_H),
        None => (0.0, 0.0),
    };
    let tx = if art_w > 0.0 { x + PAD + art_w + PAD } else { x + PAD };
    let tw = x + w - PAD - tx;
    let view = |t: &'a str, sz: i32, ink: [f32; 4], lines: usize| {
        TextView::new(t, sz, ink).with_measure(measure).leading(sz as f32 + theme::space::XS).max_lines(lines)
    };
    let mut specs: Vec<(TextView<'a>, f32)> = vec![(view(&info.title, theme::size::HEADLINE, theme::TEXT_PRIMARY, 2), theme::space::XS)];
    if !info.episode.is_empty() {
        specs.push((view(&info.episode, theme::size::BODY, theme::TEXT_PRIMARY, 2), theme::space::SM));
    }
    if !info.facts.is_empty() {
        specs.push((view(&info.facts, theme::size::CAPTION, theme::TEXT_SECONDARY, 1), theme::space::MD));
    }
    let tech = (!tech.is_empty()).then(|| view(tech, theme::size::CAPTION, theme::TEXT_SECONDARY, 1));
    let tech_h = tech.as_ref().map_or(0.0, |t| t.measure_h(w - 2.0 * PAD) + theme::space::MD * 2.0);
    let max_h = theme::layout::PLAYER_MENU_BOTTOM - EDGE;
    // the overview takes what the other lines and the chips leave (at most OVERVIEW_LINES), so
    // the chip groups always show
    let names: Vec<Vec<&str>> = info.links.iter().map(|g| g.chips.iter().map(|c| c.name.as_str()).collect()).collect();
    let group_h = |i: usize, g: &LinkGroup| {
        label_view(g.label(), measure).measure_h(tw) + theme::space::XS + place_chips(i, &names[i], 0.0, 0.0, tw, LINK_LINES, measure).1
    };
    let links_h: f32 = info.links.iter().enumerate().map(|(i, g)| group_h(i, g) + theme::space::SM).sum();
    if !info.overview.is_empty() {
        let fixed: f32 = specs.iter().map(|(v, after)| v.measure_h(tw) + after).sum();
        let lead = theme::size::LABEL as f32 + theme::space::XS;
        let room = max_h - 2.0 * PAD - tech_h - fixed - links_h - theme::space::MD;
        let lines = ((room / lead).floor().max(0.0) as usize).min(OVERVIEW_LINES);
        if lines > 0 {
            specs.push((view(&info.overview, theme::size::LABEL, theme::TEXT_READING, lines), theme::space::MD));
        }
    }
    let mut y = 0.0;
    let mut text_h: f32 = 0.0;
    let mut blocks = Vec::new();
    for (v, after) in specs {
        let h = v.measure_h(tw);
        blocks.push(Block { view: v, rect: Rect::new(tx, y, tw, h) });
        text_h = y + h;
        y += h + after;
    }
    let mut links = Vec::new();
    for (i, g) in info.links.iter().enumerate() {
        let label = label_view(g.label(), measure);
        let lh = label.measure_h(tw);
        let (place, ch) = place_chips(i, &names[i], tx, y + lh + theme::space::XS, tw, LINK_LINES, measure);
        links.push((Block { view: label, rect: Rect::new(tx, y, tw, lh) }, place));
        text_h = y + lh + theme::space::XS + ch;
        y = text_h + theme::space::SM;
    }
    let body_h = text_h.max(art_h);
    let h = (PAD + body_h + tech_h + PAD).min(max_h);
    let top = theme::layout::PLAYER_MENU_BOTTOM - h;
    // the text column runs from the panel's top; a block that would run into the tech line is cut
    let limit = top + h - PAD - tech_h;
    let blocks = blocks
        .into_iter()
        .map(|b| Block { rect: Rect::new(b.rect.x, b.rect.y + top + PAD, b.rect.w, b.rect.h), ..b })
        .filter(|b| b.rect.y + b.rect.h <= limit + 0.5)
        .collect();
    let dy = top + PAD;
    let shift = |r: Rect| Rect::new(r.x, r.y + dy, r.w, r.h);
    let links = links
        .into_iter()
        .map(|(b, g)| {
            let place = GroupPlace { chips: g.chips.iter().map(|r| shift(*r)).collect(), more: g.more.map(|(r, n)| (shift(r), n)), ..g };
            (Block { rect: shift(b.rect), ..b }, place)
        })
        // a group whose chips would run into the tech line is cut, with its label
        .filter(|(_, g)| g.chips.iter().chain(g.more.iter().map(|m| &m.0)).all(|r| r.y + r.h <= limit + 0.5))
        .collect();
    let rule_y = top + h - PAD - tech_h + theme::space::MD;
    let tech = tech.map(|v| {
        let th = v.measure_h(w - 2.0 * PAD);
        Block { view: v, rect: Rect::new(x + PAD, rule_y + theme::space::MD, w - 2.0 * PAD, th) }
    });
    let art = (art_w > 0.0).then(|| Rect::new(x + PAD, top + PAD, art_w, art_h));
    Layout { panel: Rect::new(x, top, w, h), art, blocks, links, tech, rule_y }
}

pub struct ShowInfoPanel {
    entry: EntryId,
    arg: MenuArg,
}

impl ShowInfoPanel {
    pub fn new(entry: EntryId, arg: MenuArg) -> Self {
        ShowInfoPanel { entry, arg }
    }

    /// What the panel says now, off the store.
    pub fn info(&self, views: &Views<'_>) -> (ShowInfo, String) {
        let MenuKind::Player(PlayerPanel::ShowInfo { meta, video_id, title, tech }) = &self.arg.kind else {
            return (ShowInfo::default(), String::new());
        };
        let m = match meta.as_ref().and_then(|k| views.detail.meta(k)) {
            Some(MetaState::Ready(m)) => Some(&**m),
            _ => None,
        };
        (show_info(m, video_id, title), tech.clone())
    }
}

impl Machine<Bundle> for ShowInfoPanel {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, _cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::StoreChanged(..) => {
                fx.invalidate(sn_machine::present::Provenance::Landing(fx.from()));
                Handled::No
            }
            ScreenEvent::Input(input) => match input.kind {
                InputKind::Key { key: Key::Back | Key::Ok, edge: Edge::Down, .. } => {
                    fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                    Handled::Yes
                }
                InputKind::Click { .. } => {
                    fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                    Handled::Yes
                }
                // the key that opened it closes it: BLUE, or YELLOW (a film's)
                InputKind::Key { wcode: crate::tabs::WCODE_BLUE | crate::tabs::WCODE_YELLOW, edge, .. } => {
                    if edge == Edge::Down {
                        fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                    }
                    Handled::Yes
                }
                _ => Handled::No,
            },
            _ => Handled::No,
        }
    }
}

impl Focusable<Bundle> for ShowInfoPanel {
    fn groups(&self, _: &Cx<'_, Bundle>, _: &mut Vec<GroupSpec>) {}
    fn group_of(&self, _: &u32, _: &Cx<'_, Bundle>) -> Option<GroupId> {
        None
    }
    fn neighbour(&self, _: FocusKey<u32>, _: Dir, _: &Cx<'_, Bundle>) -> Step<u32> {
        Step::Edge
    }
    fn place(&self, _: &u32, _: &Cx<'_, Bundle>, _: At) -> Option<Placed> {
        None
    }
    fn reconcile(&self, want: FocusKey<u32>, _: &Cx<'_, Bundle>) -> FocusKey<u32> {
        want
    }
    fn seat(&self, _: GroupId, _: Placed, _: &Cx<'_, Bundle>) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem: 0 }
    }
}

impl Screen<Bundle> for ShowInfoPanel {
    fn name(&self) -> &'static str {
        word::MENU
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
    }
    fn opener(&self) -> Option<(EntryId, Option<FocusKey<u32>>)> {
        Some((self.arg.host, self.arg.focus))
    }
    fn scrim(&self) -> Scrim {
        Scrim::dim(theme::underlay::DIM_COMPACT)
    }
    fn prepare(&mut self, _: &mut sn_ui::frame::Budget, _: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        let p = f.painter.alpha(f.page_alpha);
        let (info, tech) = self.info(&f.cx.views);
        let l = layout(&info, &tech, f.measure);
        sn_ui::widgets::panel_ground(p, l.panel, PANEL_RAD, f.underlay);
        if let (Some(r), Some((url, _))) = (l.art, &info.art) {
            let res = (r.w as std::os::raw::c_int, r.h as std::os::raw::c_int);
            sn_ui::widgets::card(p, r, Art::Thumb { sid: 0, key: url, res }, theme::CARD_RING_RAD, false, 1.0, 1.0);
        }
        for b in &l.blocks {
            b.view.draw(p, b.rect);
        }
        // the detail page's chips, never focused: the panel takes no focus
        let style = || sn_ui::widgets::BadgeStyle::Link { focused: false };
        for (label, place) in &l.links {
            label.view.draw(p, label.rect);
            let group = &info.links[place.group];
            for (c, r) in group.chips.iter().zip(&place.chips) {
                sn_ui::widgets::badge(p, r.x, r.cy(), &c.name, None, style(), f.measure);
            }
            if let Some((r, n)) = place.more {
                sn_ui::widgets::badge(p, r.x, r.cy(), &format!("+{n}"), None, style(), f.measure);
            }
        }
        if let Some(t) = &l.tech {
            p.rect(Rect::new(l.panel.x + PAD, l.rule_y, l.panel.w - 2.0 * PAD, 1.0), 0.0, theme::HAIRLINE, theme::HAIRLINE, 0.0);
            t.view.draw(p, t.rect);
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

impl LogicalState for ShowInfoPanel {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0);
    }
    fn probe(&self, out: &mut String) {
        out.push_str("show info");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_platform::i18n::msg;

    fn meta() -> Meta {
        serde_json::from_str(
            r#"{"id":"tt9","type":"series","name":"Show","releaseInfo":"2008–2013","runtime":"47 min","imdbRating":"9.5",
                "genres":["Drama","Crime"],"description":"A teacher turns.","poster":"https://img.example/p.jpg",
                "cast":["A","B","C","D","E","F"],"director":["G"],
                "videos":[{"id":"tt9:1:3","season":1,"episode":3,"title":"Three","released":"2008-02-10T00:00:00Z",
                           "overview":"The third.","thumbnail":"https://img.example/s.jpg"},
                          {"id":"tt9:1:4","season":1,"episode":4,"title":"Four"}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn an_episode_reads_its_own_overview_still_and_year() {
        let _g = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        let i = show_info(Some(&meta()), "tt9:1:3", "Player title");
        assert_eq!(i.title, "Show");
        assert_eq!(i.episode, format!("{} \u{b7} Three", sn_ui::fmt::episode_ordinal(1, 3)));
        assert_eq!(i.overview, "The third.");
        assert_eq!(i.facts, "2008 \u{b7} 47 min \u{b7} IMDb 9.5", "the genres are chips");
        let groups: Vec<(&str, Vec<&str>)> = i.links.iter().map(|g| (g.label(), g.chips.iter().map(|c| c.name.as_str()).collect())).collect();
        assert_eq!(
            groups,
            [
                (msg::app_detail_genres_label(), vec!["Drama", "Crime"]),
                (msg::app_detail_cast_label(), vec!["A", "B", "C", "D", "E", "F"]),
                (msg::app_detail_director_label(), vec!["G"]),
            ]
        );
        assert_eq!(i.art, Some(("https://img.example/s.jpg".into(), true)));
    }

    #[test]
    fn an_episode_with_no_overview_or_still_falls_back_to_the_show() {
        let i = show_info(Some(&meta()), "tt9:1:4", "");
        assert_eq!(i.overview, "A teacher turns.", "the show's description");
        assert_eq!(i.art, Some(("https://img.example/p.jpg".into(), false)), "the poster");
        assert!(i.facts.starts_with("2008–2013"), "the show's years: {}", i.facts);
        let movie = show_info(Some(&meta()), "tt9", "");
        assert!(movie.episode.is_empty(), "the work itself has no address");
        let none = show_info(None, "tt9:1:3", "Player title");
        assert_eq!((none.title.as_str(), none.overview.as_str()), ("Player title", ""), "until the meta lands");
    }

    /// The panel stays above the control row and under the top edge, with every block inside it,
    /// whatever the overview's length.
    #[test]
    fn the_panel_holds_its_blocks_above_the_control_row() {
        let m = sn_base::fontcov::advances::ShippedMeasure;
        let mut i = show_info(Some(&meta()), "tt9:1:3", "");
        i.overview = "word ".repeat(400);
        let l = layout(&i, "4K \u{b7} DV P8 \u{b7} AIOStreams", &m);
        assert!(l.panel.y >= EDGE - 0.5 && l.panel.y + l.panel.h <= theme::layout::PLAYER_MENU_BOTTOM + 0.5);
        assert!(l.panel.x >= MARGIN_X && l.panel.x + l.panel.w <= SCR_W - MARGIN_X + 0.5);
        for b in l.blocks.iter().chain(l.tech.iter()) {
            assert!(b.rect.y >= l.panel.y && b.rect.y + b.rect.h <= l.panel.y + l.panel.h + 0.5, "{:?} inside {:?}", b.rect, l.panel);
        }
        assert!(l.tech.is_some() && l.blocks.len() >= 4);
    }

    /// **The chips fit the panel in every shipped language**: a crowded cast folds into two lines
    /// ending in `+N`, every chip and label inside the text column, above the tech line, nothing
    /// over the art, with the longest overview the panel shows.
    #[test]
    fn the_chips_fit_the_panel_in_every_shipped_language() {
        use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
        let m = sn_base::fontcov::advances::ShippedMeasure;
        let mut meta = meta();
        meta.cast = (1..=14).map(|n| format!("Firstname Lastname-Longername {n}")).collect();
        meta.preview.genres = ["Drama", "Crime", "Thriller", "Science Fiction", "Mystery", "Documentary"].map(String::from).to_vec();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let tag = language.tag();
            let mut i = show_info(Some(&meta), "tt9:1:3", "");
            i.overview = "word ".repeat(400);
            i.title = "The Remarkably Long Title of a Series".into();
            let l = layout(&i, "4K \u{b7} DV P8 \u{b7} AIOStreams", &m);
            assert_eq!(l.links.len(), 3, "{tag}: genres, cast, director all show");
            assert_eq!(l.blocks.len(), 4, "{tag}: title, episode, facts and the overview, which gives up lines first");
            let art = l.art.unwrap();
            let col = l.blocks[0].rect;
            for (label, g) in &l.links {
                let rects: Vec<&Rect> = g.chips.iter().chain(g.more.iter().map(|m| &m.0)).collect();
                let lines: std::collections::BTreeSet<u32> = rects.iter().map(|r| r.y as u32).collect();
                assert!(lines.len() <= LINK_LINES, "{tag}: {} chip lines", lines.len());
                assert!(label.view.line_count(label.rect.w) == 1 && !label.view.truncates(label.rect.w), "{tag}: the label fits");
                for r in std::iter::once(&label.rect).chain(rects) {
                    assert!(r.x >= col.x - 0.5 && r.x + r.w <= col.x + col.w + 0.5, "{tag}: {r:?} inside the text column {col:?}");
                    assert!(r.x >= art.x + art.w, "{tag}: clear of the art");
                    assert!(r.y >= l.panel.y && r.y + r.h <= l.rule_y + 0.5, "{tag}: {r:?} above the tech line");
                }
            }
            let (_, cast) = &l.links[1];
            let (_, hidden) = cast.more.expect("fourteen long names fold into +N");
            assert_eq!(cast.chips.len() + hidden, 14, "{tag}: +N counts the rest");
        }
    }
}
