//! **The Episodes panel** — the HUD's Episodes pill (an episode of a series or an anime): the
//! show's episodes in one panel over the video, a season dropdown above them (`crate::dropdown`,
//! the Detail page's own control) and the season's episode rows under it (`crate::episodes::row`,
//! the Detail page's own rows: still, title, address and date, overview, and the library's watched
//! marks — *Watched*, the resume bar on the current episode's still).
//!
//! It opens on the season of the episode that plays, focused on it and marked *Now playing*. OK
//! on another episode goes back to the player as `AppMsg::Chose(ChoiceTarget::PlayerEpisode, i)`
//! (`i` its index in the meta's videos) and closes; the player plays it the way Up Next plays the
//! next one (the same release straight away, else that episode's Streams page). OK on the one
//! playing just closes. BACK closes, and playback goes on underneath. The rows are read off the
//! detail store, so the meta landing after the panel opened fills it in.

use std::borrow::Cow;
use std::convert::Infallible;

use sn_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputKind, Key, LogicalState, Machine, NavOp,
};
use sn_platform::i18n::msg;
use sn_stremio::types::Meta;
use sn_ui::consts::{MARGIN_X, SCR_W};
use sn_ui::form::{Form, FormId, FormSection, FormTable, RowKey, RowKind};
use sn_ui::screen::{
    Activate, At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Focusable, GroupKind, GroupSpec, Hover, Placed, RenderStrategy,
    Scrim, Screen, ScreenEvent, Seat, Step, Stop,
};
use sn_ui::{theme, Rect};

use crate::dropdown::{self, Dropdown};
use crate::episodes::{of_season, season_label, seasons};
use crate::registry::{word, AppMsg, Bundle, ChoiceTarget, MenuArg, MenuKind, PlayerPanel, Views};
use crate::stores::detail::{MetaKey, MetaState};

const PANEL_RAD: f32 = theme::radius::PANEL;
/// The panel's keep-out from the top of the screen.
const EDGE: f32 = theme::space::XL;
const PAD: f32 = theme::space::MD;
/// The season dropdown's element; the rows' keys start at [`ROW_BAND`].
const SEASON_ELEM: u32 = 1;
const ROW_BAND: u32 = 0x100;
const SEASON_GROUP: GroupId = GroupId(0);
const ROWS_GROUP: GroupId = GroupId(1);

/// An episode row's identity: its index in the meta's videos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ep(pub usize);

impl FormId for Ep {
    fn key(&self) -> RowKey {
        RowKey(ROW_BAND + (self.0 as u32).min(0x00FF_FFFF))
    }
}

/// `season`'s rows, the episode `playing` marked *Now playing*, the rest with their watched marks
/// (`crate::episodes::row_marked`).
pub fn form(meta: &Meta, season: u32, playing: &str, today: &str, watched: &crate::episodes::Watched<'_>) -> Form<Ep, Ep, Infallible> {
    let mut s = FormSection::new("");
    for (i, v) in of_season(meta, season) {
        let mut row = crate::episodes::row_marked(v, season, today, watched.of_video(v));
        if v.id == playing {
            row.ticon = None;
            row = row.value(msg::app_player_now_playing()).dim(false);
        }
        s = s.item(Ep(i), RowKind::Button, Ep(i), row);
    }
    Form::new().section(s)
}

/// The season the episode `video_id` is in, else the first.
fn season_of(meta: &Meta, video_id: &str) -> Option<u32> {
    meta.videos.iter().find(|v| v.id == video_id).map(|v| v.season.unwrap_or(0)).or_else(|| seasons(meta).first().copied())
}

pub struct EpisodesPanel {
    entry: EntryId,
    arg: MenuArg,
    key: MetaKey,
    playing: String,
    dropdown: Dropdown,
    /// The season listed; `None` until the meta is in.
    season: Option<u32>,
    table: FormTable<Ep, Ep, Infallible>,
    today: String,
}

impl EpisodesPanel {
    pub fn new(entry: EntryId, arg: MenuArg, views: &Views<'_>) -> Self {
        let (key, playing) = match &arg.kind {
            MenuKind::Player(PlayerPanel::Episodes { meta, video_id }) => (meta.clone(), video_id.clone()),
            _ => (MetaKey { kind: String::new(), id: String::new() }, String::new()),
        };
        let mut table = FormTable::new(ROW_BAND + 0x0100_0000);
        table.table.compact = true;
        let mut panel = EpisodesPanel {
            entry,
            arg,
            key,
            playing,
            dropdown: Dropdown::row(entry, SEASON_ELEM),
            season: None,
            table,
            today: crate::episodes::today(),
        };
        panel.sync(views);
        panel
    }

    fn meta<'a>(&self, views: &Views<'a>) -> Option<&'a Meta> {
        match views.detail.meta(&self.key) {
            Some(MetaState::Ready(m)) => Some(m),
            _ => None,
        }
    }

    /// The season listed (the tests read it).
    pub fn season(&self) -> Option<u32> {
        self.season
    }

    /// The rows' table (the tests read it).
    pub fn table(&self) -> &sn_ui::table::TableView {
        &self.table.table
    }

    /// The meta landed: the seasons and the playing episode's season, focused on it.
    fn sync(&mut self, views: &Views<'_>) {
        if self.season.is_some() {
            return;
        }
        let Some(meta) = self.meta(views) else { return };
        let Some(season) = season_of(meta, &self.playing) else { return };
        self.dropdown.sync_named(seasons(meta).into_iter().map(|s| (s.to_string(), season_label(s))).collect());
        self.list(meta, season, views);
    }

    /// List `season`, opening on the playing episode when it is there, else the first.
    fn list(&mut self, meta: &Meta, season: u32, views: &Views<'_>) {
        self.season = Some(season);
        self.dropdown.set_chosen(&season.to_string());
        let playing = meta.videos.iter().position(|v| v.id == self.playing).map(Ep);
        let watched = crate::episodes::Watched::of(views.library.items(), &self.key.id, meta);
        self.table.open(form(meta, season, &self.playing, &self.today, &watched), playing.as_ref());
    }

    fn has_dropdown(&self) -> bool {
        self.dropdown.len() > 1
    }

    /// The panel, the dropdown's rect and the rows' frame: right-aligned above the control row like
    /// every player menu, from the top keep-out down.
    fn frames(&self, measure: &dyn sn_machine::machine::Measure) -> (Rect, Rect, Rect) {
        let w = theme::layout::PLAYER_EPISODES_W;
        let bottom = theme::layout::PLAYER_MENU_BOTTOM;
        let panel = Rect::new(SCR_W - MARGIN_X - w, EDGE, w, bottom - EDGE);
        let dd = Rect::new(panel.x + PAD, panel.y + PAD, self.dropdown.width(measure).min(w - 2.0 * PAD), dropdown::H);
        let top = if self.has_dropdown() { dd.y + dd.h + PAD } else { panel.y + PAD };
        (panel, dd, Rect::new(panel.x, top, w, panel.y + panel.h - PAD - top))
    }

    fn focus_key(&self, elem: u32) -> FocusKey<u32> {
        FocusKey { entry: self.entry, elem }
    }

    fn row_seat(&self) -> Option<u32> {
        self.table.selected_key().or_else(|| self.table.opening_key()).map(|k| k.0)
    }

    fn activate(&mut self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        if elem == SEASON_ELEM {
            let (_, dd, _) = self.frames(cx.measure);
            let kind = self.dropdown.menu(msg::app_detail_seasons(), ChoiceTarget::PlayerEpisodeSeason);
            dropdown::open(self.entry, cx, fx, dd, kind);
            return;
        }
        let Some(Ep(i)) = self.table.index_of_key(RowKey(elem)).and_then(|n| self.table.id_at(n)).copied() else { return };
        let playing = self.meta(&cx.views).and_then(|m| m.videos.get(i)).is_some_and(|v| v.id == self.playing);
        if !playing {
            fx.push(Fx::Deliver(self.arg.reply, Delivery::Screen(ScreenEvent::App(AppMsg::Chose(ChoiceTarget::PlayerEpisode, i)))));
        }
        fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
    }

    fn rows(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.table.table.n_rows() as usize).filter_map(|i| self.table.key_at(i).map(|k| k.0))
    }
}

impl Machine<Bundle> for EpisodesPanel {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            // focus opens on the episode playing, not on the dropdown above it
            ScreenEvent::Mount => {
                if let Some(k) = self.row_seat() {
                    crate::registry::reseat(sn_ui::screen::FocusTarget::Elem(self.focus_key(k)), fx);
                }
            }
            ScreenEvent::Tick(t) => {
                let had = self.season.is_some();
                self.sync(&cx.views);
                if !had && self.season.is_some() {
                    if let Some(k) = self.row_seat() {
                        crate::registry::reseat(sn_ui::screen::FocusTarget::Elem(self.focus_key(k)), fx);
                    }
                    fx.invalidate(sn_machine::present::Provenance::Landing(fx.from()));
                }
                let focus = cx.focus.current.filter(|k| k.entry == self.entry).map(|k| k.elem);
                self.table.table.list_focused = focus.is_some_and(|e| e != SEASON_ELEM);
                if let Some(i) = focus.and_then(|e| self.table.index_of_key(RowKey(e))) {
                    self.table.table.sel = i as i32;
                }
                let (_, _, rows) = self.frames(cx.measure);
                self.table.table.update(t.dt(), rows.h);
            }
            ScreenEvent::FocusMoved { to, .. } => {
                if let Some(i) = self.table.index_of_key(RowKey(to.elem)) {
                    self.table.table.sel = i as i32;
                }
                self.table.table.list_focused = to.elem != SEASON_ELEM;
                fx.invalidate(sn_machine::present::Provenance::Input);
            }
            ScreenEvent::App(AppMsg::Chose(ChoiceTarget::PlayerEpisodeSeason, n)) => {
                if self.dropdown.choose(*n) {
                    let season = self.dropdown.kind().and_then(|k| k.parse::<u32>().ok());
                    if let (Some(meta), Some(season)) = (self.meta(&cx.views), season) {
                        self.list(meta, season, &cx.views);
                        if let Some(k) = self.row_seat() {
                            crate::registry::reseat(sn_ui::screen::FocusTarget::Elem(self.focus_key(k)), fx);
                        }
                    }
                }
                fx.invalidate(sn_machine::present::Provenance::Input);
            }
            ScreenEvent::Activate(elem) => self.activate(*elem, cx, fx),
            ScreenEvent::PressCommit(_) => {
                if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry) {
                    self.activate(k.elem, cx, fx);
                }
            }
            ScreenEvent::StoreChanged(..) => {
                fx.invalidate(sn_machine::present::Provenance::Landing(fx.from()));
                return Handled::No;
            }
            ScreenEvent::Input(input) => {
                if let InputKind::Key { key: Key::Back, edge: Edge::Down, .. } = input.kind {
                    fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                    return Handled::Yes;
                }
                // a toggle on the key that opened it, as RED and GREEN close their menus
                if let InputKind::Key { wcode: crate::tabs::WCODE_YELLOW, edge, .. } = input.kind {
                    if edge == Edge::Down {
                        fx.push(Fx::Nav(NavOp::Dismiss(self.entry)));
                    }
                    return Handled::Yes;
                }
                return Handled::No;
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }
}

impl Focusable<Bundle> for EpisodesPanel {
    fn groups(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<GroupSpec>) {
        let (_, dd, rows) = self.frames(cx.measure);
        if self.has_dropdown() {
            out.push(dropdown::group(SEASON_GROUP, dd, 1));
        }
        if self.table.focusable_len() > 0 {
            out.push(GroupSpec {
                id: ROWS_GROUP,
                kind: GroupKind::Column,
                seat: Seat::Remembered,
                reachable: AxisMask::BOTH,
                edge: [EdgeRule::Stop; 4],
                extent: rows,
                len: self.table.focusable_len(),
                elem: ElemKind::Bare,
            });
        }
    }
    fn group_of(&self, elem: &u32, _: &Cx<'_, Bundle>) -> Option<GroupId> {
        if *elem == SEASON_ELEM {
            return self.has_dropdown().then_some(SEASON_GROUP);
        }
        self.table.index_of_key(RowKey(*elem)).map(|_| ROWS_GROUP)
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, _: &Cx<'_, Bundle>) -> Step<u32> {
        let to = if key.elem == SEASON_ELEM {
            match dir {
                Dir::Down => self.row_seat(),
                _ => None,
            }
        } else {
            match dir {
                Dir::Up => self.table.step_key(RowKey(key.elem), -1).map(|k| k.0).or(self.has_dropdown().then_some(SEASON_ELEM)),
                Dir::Down => self.table.step_key(RowKey(key.elem), 1).map(|k| k.0),
                _ => None,
            }
        };
        to.map_or(Step::Edge, |elem| Step::Move(self.focus_key(elem)))
    }
    fn place(&self, elem: &u32, cx: &Cx<'_, Bundle>, _: At) -> Option<Placed> {
        let (panel, dd, rows) = self.frames(cx.measure);
        if *elem == SEASON_ELEM {
            return self.has_dropdown().then_some(Placed { rect: dd, rest_rect: dd, clip: panel, index: Some(0) });
        }
        let index = self.table.index_of_key(RowKey(*elem))?;
        let rect = self.table.table.row_frame(rows, index as i32)?;
        Some(Placed { rect, rest_rect: rect, clip: rows, index: Some(index as u32) })
    }
    fn reconcile(&self, want: FocusKey<u32>, _: &Cx<'_, Bundle>) -> FocusKey<u32> {
        let there = if want.elem == SEASON_ELEM { self.has_dropdown() } else { self.table.index_of_key(RowKey(want.elem)).is_some() };
        if there {
            return want;
        }
        self.focus_key(self.row_seat().unwrap_or(SEASON_ELEM))
    }
    fn seat(&self, g: GroupId, _: Placed, _: &Cx<'_, Bundle>) -> FocusKey<u32> {
        match g {
            SEASON_GROUP => self.focus_key(SEASON_ELEM),
            _ => self.focus_key(self.row_seat().unwrap_or(SEASON_ELEM)),
        }
    }
}

impl Screen<Bundle> for EpisodesPanel {
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
        let (panel, dd, rows) = self.frames(f.measure);
        sn_ui::widgets::panel_ground(p, panel, PANEL_RAD, f.underlay);
        if self.has_dropdown() {
            self.dropdown.draw(f, dd);
        }
        self.table.table.draw(p, rows, f.measure);
        let mut elems: Vec<u32> = self.rows().collect();
        if self.has_dropdown() {
            elems.push(SEASON_ELEM);
        }
        for elem in elems {
            if let Some(placed) = self.place(&elem, f.cx, At::Drawn) {
                let key = self.focus_key(elem);
                f.stop(p, Stop { key, rect: placed.rect, rest_rect: placed.rest_rect, clip: placed.clip, hover: Hover::Focus, activate: Activate::Immediate });
            }
        }
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

impl LogicalState for EpisodesPanel {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.table.table.sel as u32).u32(self.season.unwrap_or(u32::MAX));
        self.table.table.write_motion(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("episodes season={:?} rows={}", self.season, self.table.table.n_rows()));
    }
}
