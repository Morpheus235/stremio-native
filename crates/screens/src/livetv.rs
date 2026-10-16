//! **Live TV** — the installed addons' `tv` and `channel` catalogs as a channel grid, a top-bar
//! destination.
//!
//! Under the bar stand two chips (PlxNative's Library toolbar, `ValueChip`): the CATALOG, when
//! more than one addon catalog offers channels, and the GENRE, when the chosen catalog declares a
//! `genre` extra with options. Each opens a popover choice (`crate::menu`); the pick comes back as
//! `AppMsg::Chose`. A catalog that REQUIRES a genre starts on its first option (the addon cannot
//! answer without one — `Catalog::default_genre`); one that does not starts on "All". The grid
//! below is the chosen catalog under the chosen genre, paged by the grid store as it reaches its
//! end; OK on a channel opens its Detail page (its streams), which plays through the ordinary
//! player (HLS included).
//!
//! An install with no channel catalog says so, rather than standing empty.

use std::borrow::Cow;
use std::ffi::CString;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, Fx, GroupId, Handled, InputEvent, InputKind, Key, Leave, LogicalState, Machine,
    NavOp,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::manifest::{Addon, Catalog};
use sn_ui::cards::{CardEvent, GridSpec, Kind, SectionSpec, Stack, StackEvent, StackPage};
use sn_ui::consts::{MARGIN_X, SCR_H, SCR_W};
use sn_ui::screen::{
    At, AxisMask, Dir, DrawFrame, EdgeRule, ElemKind, Focusable, GroupKind, GroupSpec, Link, RenderStrategy, Screen,
    ScreenEvent, Seat,
};
use sn_ui::value_chip::ValueChip;
use sn_ui::widgets::{StatusKind, StatusOverlay};
use sn_ui::{theme, Env, Rect, View};

use crate::cards::{item_ref, ItemCards};
use crate::registry::{word, AppArg, AppFx, AppMsg, Bundle, ChoiceTarget, GridCmd, MenuArg, MenuKind};
use crate::stores::grid::{GridKey, GridState};
use crate::stores::StoreId;
use crate::tabs::Tab;

const CHIP_CATALOG: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x10;
const CHIP_GENRE: u32 = sn_ui::geom::NAMESPACE_FLOOR + 0x11;
const RETRY_ELEM: u32 = sn_ui::geom::NAMESPACE_FLOOR + 1;
const CARD_BASE: u32 = 0x1000;
/// Elements per grid view: a new view takes the next band, so a card key never names two items.
const VIEW_BAND: u32 = 0x0010_0000;
const CONTROLS_GROUP: GroupId = GroupId(1);
const ITEMS_GROUP: GroupId = GroupId(0);
const STATUS_GROUP: GroupId = GroupId(2);
const CHIP_H: f32 = sn_ui::widgets::TAB_PILL_H;
const CHIP_GAP: f32 = theme::space::SM;
/// The controls band: from the page's top, under the bar, to the grid.
const CHIPS_Y: f32 = sn_ui::widgets::TOP_BAR_BOTTOM + theme::space::MD;
const CONTROLS_H: f32 = CHIPS_Y + CHIP_H + theme::space::LG;

/// One addon catalog that offers channels.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveCatalog {
    pub addon_id: String,
    pub addon_name: String,
    pub catalog: Catalog,
}

impl LiveCatalog {
    /// The catalog chip's value, and the choice menu's row. Server text.
    pub fn label(&self) -> String {
        format!("{} \u{b7} {}", self.addon_name, self.catalog.display_name())
    }

    /// The genres the catalog offers (empty: no genre chip).
    pub fn genres(&self) -> Vec<String> {
        self.catalog.extra("genre").map(|g| g.options).unwrap_or_default()
    }

    /// Must a genre be named? Then there is no "All".
    pub fn genre_required(&self) -> bool {
        self.catalog.extra("genre").is_some_and(|g| g.is_required)
    }

    /// The genre chip's options, in order: "All" first unless a genre is required.
    pub fn genre_options(&self) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = if self.genre_required() { Vec::new() } else { vec![None] };
        out.extend(self.genres().into_iter().map(Some));
        out
    }

    /// The grid this catalog shows under `genre` (a required genre defaults to the first option).
    pub fn key(&self, genre: Option<&str>) -> GridKey {
        let genre = match genre {
            Some(g) => Some(g.to_string()),
            None => self.catalog.default_genre(),
        };
        GridKey { addon_id: self.addon_id.clone(), kind: self.catalog.kind.clone(), catalog_id: self.catalog.id.clone(), genre, search: None }
    }
}

/// Every browsable `tv` / `channel` catalog of `addons`, in install then manifest order.
pub fn live_catalogs(addons: &[Addon]) -> Vec<LiveCatalog> {
    addons
        .iter()
        .flat_map(|a| {
            let name = if a.manifest.name.is_empty() { a.manifest.id.clone() } else { a.manifest.name.clone() };
            a.manifest
                .catalogs
                .iter()
                .filter(|c| matches!(c.kind.as_str(), "tv" | "channel") && c.is_browsable())
                .map(move |c| LiveCatalog { addon_id: a.manifest.id.clone(), addon_name: name.clone(), catalog: c.clone() })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    Controls,
    Status,
    /// The grid of one view (its band ordinal).
    Items(u32),
}

/// The page's content: the catalogs as they stand, the selection, and the view the grid shows.
struct Page {
    catalogs: Vec<LiveCatalog>,
    selected: usize,
    genre: Option<String>,
    key: Option<GridKey>,
    view: u32,
}

impl Page {
    fn current(&self) -> Option<&LiveCatalog> {
        self.catalogs.get(self.selected)
    }

    /// Which chips stand, in order.
    fn chips(&self) -> Vec<u32> {
        let mut out = Vec::new();
        if self.catalogs.len() > 1 {
            out.push(CHIP_CATALOG);
        }
        if self.current().is_some_and(|c| !c.genres().is_empty()) {
            out.push(CHIP_GENRE);
        }
        out
    }

    fn genre_label(&self) -> String {
        match (&self.genre, self.current()) {
            (Some(g), _) => g.clone(),
            (None, Some(c)) if c.genre_required() => c.genres().into_iter().next().unwrap_or_default(),
            _ => msg::app_live_all_genres().to_string(),
        }
    }

    /// `(name, value)` of a chip.
    fn chip_text(&self, chip: u32) -> (CString, CString) {
        let (name, value) = if chip == CHIP_CATALOG {
            (msg::app_live_catalog_chip(), self.current().map(LiveCatalog::label).unwrap_or_default())
        } else {
            (msg::app_live_genre_chip(), self.genre_label())
        };
        // `ValueChip`'s value carries its own separator
        (CString::new(name).unwrap_or_default(), CString::new(format!(" \u{b7} {}", value.replace('\0', ""))).unwrap_or_default())
    }

    fn chip_rect(&self, n: usize, measure: &dyn sn_machine::machine::Measure, section_y: f32) -> Rect {
        let mut x = MARGIN_X;
        for (i, chip) in self.chips().into_iter().enumerate() {
            let (name, value) = self.chip_text(chip);
            let w = ValueChip::width(measure, &name, &value, None);
            if i == n {
                return Rect::new(x, section_y + CHIPS_Y, w, CHIP_H);
            }
            x += w + CHIP_GAP;
        }
        Rect::new(x, section_y + CHIPS_Y, 0.0, CHIP_H)
    }

    fn state(&self, cx: &Cx<'_, Bundle>) -> Option<GridState> {
        if self.catalogs.is_empty() {
            return None;
        }
        Some(self.key.as_ref().and_then(|k| cx.views.grid.state(k)).unwrap_or(GridState::Loading))
    }

    fn readout(&self, cx: &Cx<'_, Bundle>) -> Option<StatusOverlay<'static>> {
        let frame = Rect::new(0.0, CONTROLS_H, SCR_W, SCR_H - CONTROLS_H);
        if self.catalogs.is_empty() {
            return Some(
                StatusOverlay::new(Rect::new(0.0, sn_ui::widgets::TOP_BAR_BOTTOM, SCR_W, SCR_H - sn_ui::widgets::TOP_BAR_BOTTOM), msg::app_live_none_c(), StatusKind::Empty)
                    .reason(msg::app_live_none_reason_c()),
            );
        }
        crate::grid::readout(&self.state(cx)?, frame)
    }

    fn items<'a>(&self, cx: &Cx<'a, Bundle>) -> &'a [sn_stremio::types::MetaPreview] {
        self.key.as_ref().map_or(&[], |k| cx.views.grid.items(k))
    }

    fn base(&self) -> u32 {
        CARD_BASE + (self.view % 64) * VIEW_BAND
    }
}

impl StackPage<Bundle> for Page {
    type Key = Sec;
    type Cards<'a> = ItemCards<'a>;

    fn revision(&self, cx: &Cx<'_, Bundle>) -> u64 {
        (u64::from(cx.views.grid.gen()) << 32) ^ (u64::from(self.view) << 16) ^ (self.catalogs.len() as u64)
    }
    fn sections(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<SectionSpec<Sec>>) {
        let focusable = !self.chips().is_empty();
        out.push(SectionSpec::new(Sec::Controls, Kind::Custom { height: CONTROLS_H, focusable }, CONTROLS_GROUP).ranked(1));
        if let Some(o) = self.readout(cx) {
            let failed = matches!(self.state(cx), Some(GridState::Failed(_)));
            let rect = if failed { o.action_frame_measured(cx.measure).unwrap_or(Rect::FULL) } else { Rect::FULL };
            out.push(SectionSpec::new(Sec::Status, Kind::Overlay { rect, focusable: failed }, STATUS_GROUP).ranked(2));
        }
        if !self.catalogs.is_empty() {
            out.push(SectionSpec::new(Sec::Items(self.view), Kind::Grid { spec: GridSpec::new(0.0, CHIPS_Y) }, ITEMS_GROUP));
        }
    }
    fn fallback(&self, _cx: &Cx<'_, Bundle>, out: &mut Vec<Sec>) {
        out.extend([Sec::Items(self.view), Sec::Controls, Sec::Status]);
    }
    fn cards<'a>(&'a self, cx: &'a Cx<'_, Bundle>, k: Sec) -> Option<ItemCards<'a>> {
        match k {
            Sec::Items(v) if v == self.view => Some(ItemCards {
                items: self.items(cx),
                base: self.base(),
                more: matches!(self.state(cx), Some(GridState::Ready { more: true })),
            }),
            _ => None,
        }
    }
    fn elem_of(&self, k: Sec) -> Option<u32> {
        match k {
            Sec::Status => Some(RETRY_ELEM),
            Sec::Controls => self.chips().first().copied(),
            Sec::Items(_) => None,
        }
    }
    fn plain_len(&self, k: Sec) -> usize {
        match k {
            Sec::Controls => self.chips().len(),
            Sec::Status => 1,
            Sec::Items(_) => 0,
        }
    }
    fn plain_elem(&self, k: Sec, n: usize) -> Option<u32> {
        match k {
            Sec::Controls => self.chips().get(n).copied(),
            Sec::Status => (n == 0).then_some(RETRY_ELEM),
            Sec::Items(_) => None,
        }
    }
    fn plain_step(&self, k: Sec, at: usize, dir: Dir) -> Option<usize> {
        match (k, dir) {
            (Sec::Controls, Dir::Left) => at.checked_sub(1),
            (Sec::Controls, Dir::Right) => (at + 1 < self.chips().len()).then_some(at + 1),
            _ => None,
        }
    }
    fn element_rect(&self, cx: &Cx<'_, Bundle>, k: Sec, n: usize, section: Rect) -> Rect {
        match k {
            Sec::Controls => self.chip_rect(n, cx.measure, section.y),
            _ => section,
        }
    }
    fn plain_group(&self, _cx: &Cx<'_, Bundle>, k: Sec, id: GroupId, extent: Rect) -> GroupSpec {
        let (kind, len) = match k {
            Sec::Controls => (GroupKind::Row { wrap: false }, self.chips().len()),
            _ => (GroupKind::Free, 1),
        };
        GroupSpec {
            id,
            kind,
            seat: Seat::First,
            reachable: AxisMask::BOTH,
            edge: [EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop],
            extent,
            len,
            elem: ElemKind::Control,
        }
    }
    fn custom_draw(&self, k: Sec, f: &mut DrawFrame<'_, '_, Bundle>, r: Rect, _scroll: f32) {
        let p = f.painter.alpha(f.page_alpha);
        match k {
            Sec::Controls => {
                for (n, chip) in self.chips().into_iter().enumerate() {
                    let (name, value) = self.chip_text(chip);
                    let rect = self.chip_rect(n, f.measure, r.y);
                    let focused = f.focus.current.is_some_and(|key| key.elem == chip);
                    ValueChip::new(&name, &value, None, rect).focused(focused).draw(&Env::inert(), p);
                }
            }
            Sec::Status => {
                if let Some(o) = self.readout(f.cx) {
                    let focused = f.focus.current.is_some_and(|key| key.elem == RETRY_ELEM);
                    o.phase(f.cx.tick.ms).focused(focused).draw_measured(&Env::inert(), p, f.measure);
                }
            }
            Sec::Items(_) => {}
        }
    }
}

pub struct LiveTvScreen {
    entry: EntryId,
    page: Page,
    stack: Stack<Sec>,
    /// The catalog store generation the catalogs were read at.
    read: Option<u32>,
    /// The group DOWN from the bar enters, as of the last tick.
    first: GroupId,
    /// The page mounts before its catalogs are read: the first control or card takes the seat,
    /// and with neither the remote rests on the page's pill.
    seat: crate::registry::FirstSeat,
    /// The viewer has moved focus themselves.
    chose: bool,
}

impl LiveTvScreen {
    pub fn new(entry: EntryId) -> Self {
        LiveTvScreen {
            entry,
            page: Page { catalogs: Vec::new(), selected: 0, genre: None, key: None, view: 0 },
            stack: Stack::new(entry).hold_hint(sn_ui::hold_hint::Kind::Library),
            read: None,
            first: CONTROLS_GROUP,
            seat: crate::registry::FirstSeat::new(),
            chose: false,
        }
    }

    /// The grid the page shows, as the tests read it.
    pub fn key(&self) -> Option<&GridKey> {
        self.page.key.as_ref()
    }

    pub fn catalogs(&self) -> &[LiveCatalog] {
        &self.page.catalogs
    }

    fn view(&self) -> impl Focusable<Bundle> + '_ {
        self.stack.view(&self.page)
    }

    /// Re-read the catalogs when the install moved; keep the selection by identity.
    fn sync(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        if self.read == Some(cx.views.catalog.gen()) {
            return;
        }
        self.read = Some(cx.views.catalog.gen());
        let was = self.page.current().map(|c| (c.addon_id.clone(), c.catalog.id.clone(), c.catalog.kind.clone()));
        self.page.catalogs = live_catalogs(cx.views.addons());
        self.page.selected = was
            .and_then(|(a, id, k)| self.page.catalogs.iter().position(|c| c.addon_id == a && c.catalog.id == id && c.catalog.kind == k))
            .unwrap_or(0);
        self.show(fx);
    }

    /// Point the grid at the selection: close the old view, open the new one.
    fn show(&mut self, fx: &mut Effects<'_, Bundle>) {
        let key = self.page.current().map(|c| c.key(self.page.genre.as_deref()));
        if key == self.page.key {
            return;
        }
        if let Some(old) = self.page.key.take() {
            fx.push(Fx::App(AppFx::Grid(GridCmd::Close(old))));
        }
        if let Some(k) = &key {
            fx.push(Fx::App(AppFx::Grid(GridCmd::Open(k.clone()))));
        }
        self.page.key = key;
        self.page.view = self.page.view.wrapping_add(1);
        fx.invalidate(Provenance::Landing(fx.from()));
    }

    /// The menu a chip opens.
    fn choice(&self, chip: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let Some(n) = self.page.chips().iter().position(|c| *c == chip) else { return };
        let (title, options, current, target) = if chip == CHIP_CATALOG {
            (msg::app_live_catalog_chip(), self.page.catalogs.iter().map(LiveCatalog::label).collect(), self.page.selected, ChoiceTarget::LiveCatalog)
        } else {
            let Some(c) = self.page.current() else { return };
            let options = c.genre_options();
            let current = options.iter().position(|o| *o == self.page.genre).unwrap_or(0);
            (
                msg::app_live_genre_chip(),
                options.into_iter().map(|o| o.unwrap_or_else(|| msg::app_live_all_genres().to_string())).collect(),
                current,
                ChoiceTarget::LiveGenre,
            )
        };
        // the controls are the page's first section: their top is the page's scroll, upward
        let section_y = -self.stack.scroll();
        let anchor = self.page.chip_rect(n, cx.measure, section_y);
        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
            host: self.entry,
            reply: fx.from(),
            focus: cx.focus.current,
            anchor: MenuArg::anchor_of(anchor),
            kind: MenuKind::Choice { title: title.to_string(), options, current, target },
        }))));
    }

    /// A pick came back from a chip's menu.
    pub fn chose(&mut self, target: ChoiceTarget, i: usize, fx: &mut Effects<'_, Bundle>) {
        match target {
            ChoiceTarget::LiveCatalog if i < self.page.catalogs.len() => {
                if i != self.page.selected {
                    self.page.selected = i;
                    self.page.genre = None;
                }
            }
            ChoiceTarget::LiveGenre => {
                if let Some(g) = self.page.current().and_then(|c| c.genre_options().get(i).cloned()) {
                    self.page.genre = g;
                }
            }
            ChoiceTarget::LiveCatalog => {}
        }
        self.show(fx);
    }

    fn open_card(&self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> bool {
        let items = ItemCards { items: self.page.items(cx), base: self.page.base(), more: false };
        match items.item(elem) {
            Some(item) => {
                fx.push(Fx::Nav(NavOp::Push(AppArg::Detail(item_ref(item)))));
                true
            }
            None => false,
        }
    }

    fn press(&mut self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        if crate::tabs::on_pill(elem, Tab::LiveTv, Some(self.first), fx) {
            return;
        }
        match elem {
            CHIP_CATALOG | CHIP_GENRE => self.choice(elem, cx, fx),
            RETRY_ELEM => {
                if let Some(k) = &self.page.key {
                    fx.push(Fx::App(AppFx::Grid(GridCmd::Retry(k.clone()))));
                }
            }
            _ => {
                self.open_card(elem, cx, fx);
            }
        }
    }
}

impl LogicalState for LiveTvScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.page.selected as u32).u32(self.page.view);
        c.option(self.page.genre.as_deref(), |c, g| {
            c.str(g);
        });
        self.stack.write(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("livetv catalogs={} selected={}", self.page.catalogs.len(), self.page.selected));
    }
}

impl Machine<Bundle> for LiveTvScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        if matches!(ev, ScreenEvent::Mount | ScreenEvent::Tick(_) | ScreenEvent::StoreChanged(..)) {
            self.sync(cx, fx);
        }
        let got = self.stack.on(&self.page, ev, cx, fx);
        match ev {
            ScreenEvent::Tick(_) => {
                if let Some(StackEvent::Card(Sec::Items(_), CardEvent::Want(range))) = &got {
                    if range.end > self.page.items(cx).len() {
                        if let Some(k) = &self.page.key {
                            fx.push(Fx::App(AppFx::Grid(GridCmd::More(k.clone()))));
                        }
                    }
                }
                self.first = if self.page.chips().is_empty() { ITEMS_GROUP } else { CONTROLS_GROUP };
                let target = if !self.page.chips().is_empty() {
                    sn_ui::screen::FocusTarget::FirstInGroup(CONTROLS_GROUP)
                } else if !self.page.items(cx).is_empty() {
                    sn_ui::screen::FocusTarget::FirstInGroup(ITEMS_GROUP)
                } else {
                    sn_ui::screen::FocusTarget::Elem(sn_machine::machine::FocusKey { entry: self.entry, elem: Tab::LiveTv.elem() })
                };
                let has_content = !matches!(target, sn_ui::screen::FocusTarget::Elem(_));
                let seated = crate::tabs::rests(self.entry, Tab::LiveTv, cx, self.chose, has_content);
                self.seat.tick(seated, Some(target), fx);
                if self.page.state(cx) == Some(GridState::Loading) {
                    fx.note(sn_machine::present::PresentEvent::Motion);
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Grid.ord() || *ord == StoreId::Catalog.ord() => {
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::Chose(target, i)) => {
                self.chose(*target, *i, fx);
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                match got {
                    Some(StackEvent::Card(_, CardEvent::Activate(elem))) => {
                        self.open_card(elem, cx, fx);
                    }
                    _ => {
                        if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry) {
                            self.press(k.elem, cx, fx);
                        }
                    }
                }
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.press(*elem, cx, fx);
                Handled::Yes
            }
            ScreenEvent::PressHold(_) => {
                if let Some(StackEvent::Card(_, CardEvent::Hold(elem))) = got {
                    let items = ItemCards { items: self.page.items(cx), base: self.page.base(), more: false };
                    if let Some(item) = items.item(elem) {
                        let anchor = self.stack.focused_card(&self.page, cx, At::Drawn).map_or_else(crate::menu::fallback_anchor, |(_, at)| at.rect);
                        fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
                            host: self.entry,
                            reply: fx.from(),
                            focus: cx.focus.current,
                            anchor: MenuArg::anchor_of(anchor),
                            kind: crate::cards::item_menu(item),
                        }))));
                    }
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => {
                self.chose = true;
                crate::tabs::on_back(self.entry, Tab::LiveTv, cx, fx)
            }
            ScreenEvent::WillLeave(Leave::ForGood) | ScreenEvent::Unmount => {
                if let Some(k) = self.page.key.take() {
                    fx.push(Fx::App(AppFx::Grid(GridCmd::Close(k))));
                }
                Handled::Yes
            }
            ScreenEvent::FocusMoved { from, by, .. } => {
                self.chose |= crate::tabs::deliberate(self.entry, *from, *by);
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Enter(_) | ScreenEvent::Uncover => {
                fx.invalidate(Provenance::Nav);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

sn_ui::focusable_via_view!(LiveTvScreen, Bundle, view);

impl Screen<Bundle> for LiveTvScreen {
    fn name(&self) -> &'static str {
        word::LIVE_TV
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
    }
    fn links(&self, out: &mut Vec<Link>) {
        crate::tabs::links(self.first, out);
        if self.first == CONTROLS_GROUP {
            out.push(Link { from: CONTROLS_GROUP, dir: Dir::Down, to: ITEMS_GROUP });
            out.push(Link { from: ITEMS_GROUP, dir: Dir::Up, to: CONTROLS_GROUP });
        }
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        sn_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        self.stack.view(&self.page).paint(f);
        self.stack.draw_hold_hint(f.painter.alpha(f.page_alpha), f.measure);
    }
    fn redraw_focused(&self, f: &mut DrawFrame<'_, '_, Bundle>, focus: Option<sn_machine::machine::FocusKey<u32>>) {
        self.stack.view(&self.page).redraw_focused(f, focus);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}
