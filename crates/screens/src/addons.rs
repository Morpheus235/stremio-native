//! **Addons** — the addons the app uses ([`AddonsStore::in_use`](crate::stores::addons::AddonsStore::in_use))
//! and the account they come from; in its second mode, **one addon**: what it serves, its
//! catalogs, and whether a search asks it.
//!
//! Both are route pages over a `TableView` (`TableScreen`), built by pure functions of the stores
//! ([`build_list`], [`build_addon`]) so the host suite grades them:
//!
//! - **the list** opens with the account: who is signed in (signing out is the top bar's account
//!   menu's, which asks first), or *Sign in*; a failed
//!   refresh of the collection says so with *Try again*. Then one row per addon in the
//!   collection's order: its name, version and what it serves (resources and types), and
//!   whether it is configurable. OK opens the addon.
//! - **an addon** lists its facts, the *Include in search* switch (shared with the search
//!   fan-out through `sn_stremio::search::AddonFilter`, persisted by the addons store) when it has
//!   a searchable catalog, and its catalogs: whether each is a Home row or search only.
//!
//! Installing and removing addons is not offered on the television: Stremio's own apps do that
//! (the copy says so).
//!
//! The list is a page pushed from Settings' *Addons* row or the account menu's (BACK returns to
//! where it was pushed from); it wears the top bar, on the account pill (`tabs::Tab::of_arg`). An
//! addon is a page pushed over it (crumb *Addons*).

use std::borrow::Cow;

use sn_machine::machine::{
    Canon, Cx, Effects, EntryId, Fx, GroupId, Handled, LogicalState, Machine,
    NavOp,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::manifest::Addon;
use sn_ui::form::{RowKey, RowKeys};
use sn_ui::route_screen::RouteLayout;
use sn_ui::screen::{DrawFrame, FocusSource, FocusTarget, HitSource, Link, Part, RenderStrategy, Screen, ScreenEvent};
use sn_ui::table::{Row, Section, TableView};
use sn_ui::table_screen::{Header, TableScreen};
use sn_ui::{theme, Rect};

use crate::registry::{kind_label, word, AddonsCmd, AppArg, AppFx, Bundle, FirstSeat, Views};
use crate::stores::account::SessionState;
use crate::stores::addons::{InUse, Refresh};
use crate::stores::StoreId;

const GROUP: GroupId = GroupId(0);

pub const SIGN_IN: u32 = 1;
pub const RETRY: u32 = 3;
pub const SEARCH: u32 = 4;
/// Addon rows: this plus the addon's position.
pub const ADDON: u32 = 0x1000;
/// Catalog rows (inert, but focusable so a long list can be read): this plus the position.
pub const CATALOG: u32 = 0x2000;
/// The most addon or catalog rows a page lists (the key ranges above).
pub const MAX_ROWS: usize = 0x0FFF;
const NOTE: u32 = 0x8000;

/// The page's two modes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    List,
    /// One addon, by manifest id.
    Addon(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    SignIn,
    Retry,
    Open(String),
    ToggleSearch(String),
}

#[derive(Default)]
pub struct Built {
    pub sections: Vec<Section>,
    pub keys: Vec<RowKey>,
    pub actions: Vec<Option<Action>>,
}

impl Built {
    fn push(&mut self, s: &mut Section, row: Row, key: u32, action: Option<Action>) {
        s.rows.push(row);
        self.keys.push(RowKey(key));
        self.actions.push(action);
    }
    fn note(&mut self, s: &mut Section, text: impl Into<String>) {
        let key = NOTE + self.keys.len() as u32;
        self.push(s, Row::note(text), key, None);
    }
    pub fn index_of(&self, key: RowKey) -> Option<usize> {
        self.keys.iter().position(|k| *k == key)
    }
    pub fn action_of(&self, key: u32) -> Option<&Action> {
        self.actions.get(self.index_of(RowKey(key))?)?.as_ref()
    }
}

impl RowKeys for Built {
    fn key_at(&self, index: usize) -> Option<RowKey> {
        self.keys.get(index).copied()
    }
    fn index_of_key(&self, key: RowKey) -> Option<usize> {
        Built::index_of(self, key)
    }
    fn reseat(&self) -> Option<RowKey> {
        None
    }
}

/// The addons the list shows: the account's collection, or the defaults as far as their
/// manifests have loaded.
pub fn listed<'a>(views: &Views<'a>) -> &'a [Addon] {
    match views.addons.in_use() {
        InUse::Installed(a) => a,
        InUse::Defaults(_) => views.addons(),
    }
}

/// A resource's app name ("Streams"), or the addon's own word for one the app has none for.
fn resource_label(name: &str) -> Cow<'_, str> {
    Cow::Borrowed(match name {
        "catalog" => msg::app_addons_res_catalog(),
        "meta" => msg::app_addons_res_meta(),
        "stream" => msg::app_addons_res_stream(),
        "subtitles" => msg::app_addons_res_subtitles(),
        "addon_catalog" => msg::app_addons_res_addon_catalog(),
        other => return Cow::Owned(other.to_string()),
    })
}

fn kind_text(kind: &str) -> String {
    kind_label(kind).map_or_else(|| kind.to_string(), str::to_string)
}

/// "Catalogs, Streams · Movies, Series".
pub fn serves_line(addon: &Addon) -> String {
    let mut resources: Vec<String> = Vec::new();
    for r in &addon.manifest.resources {
        let l = resource_label(r.name()).into_owned();
        if !resources.contains(&l) {
            resources.push(l);
        }
    }
    let types: Vec<String> = addon.manifest.types.iter().map(|t| kind_text(t)).collect();
    match (resources.is_empty(), types.is_empty()) {
        (false, false) => format!("{} \u{b7} {}", resources.join(", "), types.join(", ")),
        (false, true) => resources.join(", "),
        (true, false) => types.join(", "),
        (true, true) => String::new(),
    }
}

fn addon_name(addon: &Addon) -> &str {
    if addon.manifest.name.trim().is_empty() {
        &addon.manifest.id
    } else {
        &addon.manifest.name
    }
}

/// The list page.
pub fn build_list(views: &Views<'_>) -> Built {
    let mut out = Built::default();
    let mut account = Section::new(msg::app_addons_account());
    match views.account.session() {
        SessionState::Restoring => out.note(&mut account, msg::app_addons_restoring()),
        SessionState::SignedOut { .. } => {
            out.push(&mut account, Row::new(msg::app_addons_sign_in()).detail(msg::app_addons_sign_in_detail()).chevron(true), SIGN_IN, Some(Action::SignIn));
        }
        SessionState::SignedIn(a) => {
            let who = if a.email.is_empty() { a.user_id.clone() } else { a.email.clone() };
            out.push(&mut account, Row::new(msg::app_addons_signed_in()).value(who).server_value(), NOTE - 1, None);
            match views.addons.refresh() {
                Refresh::Idle => {}
                Refresh::Loading => out.note(&mut account, msg::app_addons_refreshing()),
                Refresh::Failed(why) => {
                    out.note(&mut account, msg::app_addons_refresh_failed(why));
                    out.push(&mut account, Row::new(msg::app_home_retry()), RETRY, Some(Action::Retry));
                }
            }
            if views.account.save_failed() {
                out.note(&mut account, msg::app_addons_not_kept());
            }
            // signing out is the top bar's account menu's (it asks first): no second path here
        }
    }
    out.sections.push(account);
    let mut list = Section::new(msg::app_addons_installed());
    let addons = listed(views);
    if addons.is_empty() {
        out.note(&mut list, msg::app_home_loading_addons());
    }
    for (i, addon) in addons.iter().take(MAX_ROWS).enumerate() {
        let version = if addon.manifest.version.is_empty() { String::new() } else { format!("v{} \u{b7} ", addon.manifest.version) };
        let mut row = Row::new(addon_name(addon)).server_label().detail(format!("{version}{}", serves_line(addon))).server_detail().chevron(true);
        if addon.manifest.behavior_hints.configurable {
            row = row.value(msg::app_addons_configurable());
        }
        out.push(&mut list, row, ADDON + i as u32, Some(Action::Open(addon.manifest.id.clone())));
    }
    out.sections.push(list);
    out
}

/// One addon's page; `None` when it is not (any longer) among the addons in use.
pub fn build_addon(views: &Views<'_>, id: &str) -> (Built, Option<Addon>) {
    let mut out = Built::default();
    let Some(addon) = listed(views).iter().find(|a| a.manifest.id == id).cloned() else {
        let mut s = Section::new(msg::app_addons_about());
        out.note(&mut s, msg::app_addons_gone());
        out.sections.push(s);
        return (out, None);
    };
    let mut about = Section::new(msg::app_addons_about());
    if !addon.manifest.version.is_empty() {
        out.push(&mut about, Row::new(msg::app_addons_version()).value(addon.manifest.version.clone()).server_value(), NOTE - 2, None);
    }
    let serves = serves_line(&addon);
    if !serves.is_empty() {
        out.push(&mut about, Row::new(msg::app_addons_serves()).detail(serves).server_detail(), NOTE - 3, None);
    }
    let configurable = if addon.manifest.behavior_hints.configurable { msg::app_addons_yes() } else { msg::app_addons_no() };
    out.push(&mut about, Row::new(msg::app_addons_configurable()).value(configurable), NOTE - 4, None);
    out.sections.push(about);
    let mut search = Section::new(msg::app_addons_search());
    if addon.manifest.searchable_catalogs().next().is_some() {
        let on = views.addons.searched(&addon.manifest.id);
        out.push(&mut search, Row::new(msg::app_addons_search_toggle()).toggle(on), SEARCH, Some(Action::ToggleSearch(addon.manifest.id.clone())));
    } else {
        out.note(&mut search, msg::app_addons_no_search());
    }
    out.sections.push(search);
    let mut catalogs = Section::new(msg::app_addons_catalogs());
    if addon.manifest.catalogs.is_empty() {
        out.note(&mut catalogs, msg::app_addons_no_catalogs());
    }
    for (i, c) in addon.manifest.catalogs.iter().take(MAX_ROWS).enumerate() {
        let place = if c.is_browsable() { msg::app_addons_on_home() } else { msg::app_addons_search_only() };
        let row = Row::new(c.display_name()).server_label().detail(kind_text(&c.kind)).server_detail().value(place);
        out.push(&mut catalogs, row, CATALOG + i as u32, None);
    }
    out.sections.push(catalogs);
    (out, Some(addon))
}

pub struct AddonsScreen {
    entry: EntryId,
    mode: Mode,
    table: TableView,
    built: Built,
    built_gen: Option<(u32, u32, u32)>,
    sel: Option<RowKey>,
    title: String,
    copy: String,
    seat: FirstSeat,
}

impl AddonsScreen {
    pub fn list(entry: EntryId) -> Self {
        Self::new(entry, Mode::List)
    }

    pub fn addon(entry: EntryId, id: String) -> Self {
        Self::new(entry, Mode::Addon(id))
    }

    fn new(entry: EntryId, mode: Mode) -> Self {
        let mut table = TableView::new();
        table.list_focused = true;
        AddonsScreen {
            entry,
            mode,
            table,
            built: Built::default(),
            built_gen: None,
            sel: None,
            title: msg::app_addons_title().to_string(),
            copy: String::new(),
            seat: FirstSeat::new(),
        }
    }

    pub fn built(&self) -> &Built {
        &self.built
    }

    fn crumb(&self) -> Option<&'static str> {
        match self.mode {
            // a page of the top bar's (the account pill's): no crumb, the bar says where it is
            Mode::List => None,
            Mode::Addon(_) => Some(msg::app_addons_title()),
        }
    }

    /// The list wears the top bar (it is the account pill's page, `tabs::Tab::of_arg`); one addon
    /// is a route page over it.
    fn layout(&self) -> RouteLayout {
        match self.mode {
            Mode::List => RouteLayout::below_top_bar(),
            Mode::Addon(_) => RouteLayout::screen(),
        }
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(Header::new(self.layout(), self.crumb(), &self.title, &self.copy), &self.table, GROUP, self.entry)
            .keyed(&self.built)
    }

    fn activate(&mut self, elem: u32, fx: &mut Effects<'_, Bundle>) {
        match self.built.action_of(elem).cloned() {
            Some(Action::SignIn) => fx.push(Fx::Nav(NavOp::Push(AppArg::SignIn))),
            Some(Action::Retry) => fx.push(Fx::App(AppFx::Addons(AddonsCmd::Retry))),
            Some(Action::Open(id)) => fx.push(Fx::Nav(NavOp::Push(AppArg::Addon(id)))),
            Some(Action::ToggleSearch(id)) => fx.push(Fx::App(AppFx::Addons(AddonsCmd::ToggleSearch(id)))),
            None => {}
        }
        fx.invalidate(Provenance::Input);
    }

    fn sync(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let v = &cx.views;
        let gen = (v.account.gen(), v.addons.gen(), v.catalog.gen());
        if self.built_gen == Some(gen) {
            return;
        }
        self.built_gen = Some(gen);
        let built = match &self.mode {
            Mode::List => {
                self.copy = match v.account.session() {
                    SessionState::SignedIn(_) => msg::app_addons_copy_signed_in().to_string(),
                    _ => msg::app_addons_copy_signed_out().to_string(),
                };
                build_list(v)
            }
            Mode::Addon(id) => {
                let (built, addon) = build_addon(v, id);
                if let Some(a) = addon {
                    self.title = addon_name(&a).to_string();
                    self.copy = a.manifest.description.clone().unwrap_or_default();
                }
                built
            }
        };
        let sel = self.sel.and_then(|k| built.index_of(k)).map(|i| i as i32);
        let fresh = self.table.sections.is_empty();
        self.built = built;
        let sections = std::mem::take(&mut self.built.sections);
        self.table.set_sections_or_open(sections, sel, !fresh);
        fx.invalidate(Provenance::Landing(fx.from()));
    }
}

impl LogicalState for AddonsScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0);
        c.option(self.sel, |c, k| {
            c.u32(k.0);
        });
        self.table.write_motion(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("addons rows={} sel={:?}", self.table.n_rows(), self.sel));
    }
}

impl Machine<Bundle> for AddonsScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                self.sync(cx, fx);
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _)
                if [StoreId::Account.ord(), StoreId::Addons.ord(), StoreId::Catalog.ord()].contains(ord) =>
            {
                self.sync(cx, fx);
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                self.sync(cx, fx);
                let target = Some(FocusTarget::FirstInGroup(GROUP));
                let seated = cx.focus.current.is_some_and(|k| k.entry == self.entry);
                self.seat.tick(seated, target, fx);
                self.table.update(t.dt(), self.layout().sectioned_table().h);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                if let Some(i) = self.built.index_of(RowKey(to.elem)) {
                    self.table.sel = i as i32;
                    self.sel = Some(RowKey(to.elem));
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, crate::tabs::Tab::Settings, Some(GROUP), cx, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.activate(*elem, fx);
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

sn_ui::focusable_via_view!(AddonsScreen, Bundle, view);

impl Screen<Bundle> for AddonsScreen {
    fn name(&self) -> &'static str {
        match self.mode {
            Mode::List => word::ADDONS,
            Mode::Addon(_) => word::ADDON,
        }
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        AddonsScreen::crumb(self).map(Cow::Borrowed)
    }
    fn links(&self, out: &mut Vec<Link>) {
        if matches!(self.mode, Mode::List) {
            crate::tabs::links(GROUP, out);
        }
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        sn_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        let mut v = self.view();
        Part::<Bundle>::draw(&mut v, f, Rect::FULL);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}
