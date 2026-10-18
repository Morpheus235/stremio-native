//! **The top bar's destinations** — the ONE registry the shared bar, the remote's hotkeys and
//! BACK read ([`TABS`]), and the rules every tab page shares.
//!
//! A destination is a [`TabEntry`]: which page it opens, its stable strip control id and its
//! label. The bar draws them in this order; the Search pill is the shared strip's magnifier,
//! which the renderer always draws LAST (`sn_ui::widgets::StripRender`), so Search is the last
//! entry. Adding a destination (the sign-in work's account page, say) is one line here, its
//! [`AppArg`] variant and its mount arm.
//!
//! **Navigation** (the PlxNative model, `sn_ui::containers::tabs`): Home is the root of the one
//! shared stack and every other tab is a `NavOp::SelectTab` that covers it, so
//!
//! - UP from a page's first row reaches the bar ([`links`]); LEFT/RIGHT walk the pills; OK on a
//!   pill selects it ([`on_pill`]); DOWN or OK on the selected pill enters the page;
//! - BACK on a page's content returns focus to the bar ([`on_back`]); BACK on the bar is the
//!   container's: on a tab other than Home it pops to Home, on Home it is the application's root
//!   BACK, which hands the screen to the television's Home and keeps the app running (PlxNative's
//!   exit behaviour; the remote's EXIT key is what ends the process).
//!
//! **Hotkeys** ([`hotkey`], [`route_hotkey`]): RED, GREEN, BLUE and the channel rocker switch tabs
//! from anywhere except while text is being entered, a menu is open, or the player is up. YELLOW is
//! no hotkey: it goes on to the page, where a row of cards takes it as *See all* (Home) and every
//! other page ignores it. The codes and their provenance are `docs/remote-keys.md`.

use sn_machine::machine::{Cx, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, MachineId, NavOp};
use sn_ui::containers::tabs::STRIP;
use sn_ui::dispatch::{Dispatcher, STRIP_BASE};
use sn_ui::screen::{Dir, FocusTarget, Link};

use crate::registry::{reseat, AppArg, Bundle, MenuArg, MenuKind};

/// A top-bar destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Home,
    /// Stremio's Discover: a sub-tab per type (Live TV among them), a catalog, a genre.
    Discover,
    Library,
    Settings,
    Search,
}

/// One registry entry: the destination, its stable strip id (`STRIP_BASE + id`, never its
/// position, so removing a destination renumbers no other control) and its label.
pub struct TabEntry {
    pub tab: Tab,
    pub id: u32,
    pub label: fn() -> &'static str,
}

/// The pills, in bar order. Search is last: the strip draws its last pill as the magnifier.
/// Addons is not one (id 3 stays retired, so no other pill is renumbered), and Settings is not
/// one any more: both live behind the account pill at the bar's left end ([`ACCOUNT_ELEM`]), which
/// took Settings' id 4. Discover took Live TV's pill (id 1): Live TV is its sub-tab now.
pub const TABS: [TabEntry; 4] = [
    TabEntry { tab: Tab::Home, id: 0, label: sn_platform::i18n::msg::app_tab_home },
    TabEntry { tab: Tab::Discover, id: 1, label: sn_platform::i18n::msg::app_tab_discover },
    TabEntry { tab: Tab::Library, id: 2, label: sn_platform::i18n::msg::app_tab_library },
    TabEntry { tab: Tab::Search, id: 5, label: sn_platform::i18n::msg::app_tab_search },
];

/// **The account pill** — the avatar at the bar's left end, before Home. OK opens the account menu
/// ([`open_account_menu`]: Settings, Addons, the account, Sign out or Sign in). It is
/// [`Tab::Settings`]'s stop: the Settings page and the Addons list are "on" it, so it wears the
/// current-page plate there and BACK on their content returns to it. No colour key names it.
pub const ACCOUNT_ELEM: u32 = STRIP_BASE + 4;

impl Tab {
    fn entry(self) -> Option<&'static TabEntry> {
        TABS.iter().find(|e| e.tab == self)
    }

    /// The pill's bar position; `None` for Settings, whose stop is the account pill.
    pub fn index(self) -> Option<usize> {
        TABS.iter().position(|e| e.tab == self)
    }

    pub fn at(index: usize) -> Option<Tab> {
        TABS.get(index).map(|e| e.tab)
    }

    /// The page this destination opens.
    pub fn arg(self) -> AppArg {
        match self {
            Tab::Home => AppArg::Home,
            Tab::Discover => AppArg::Discover,
            Tab::Library => AppArg::Library,
            Tab::Settings => AppArg::Settings,
            Tab::Search => AppArg::Search,
        }
    }

    /// The destination a page IS, if it is one.
    pub fn of_arg(arg: &AppArg) -> Option<Tab> {
        Some(match arg {
            AppArg::Home => Tab::Home,
            // Live TV is Discover opened on its Live TV sub-tab
            AppArg::Discover | AppArg::LiveTv | AppArg::DiscoverAt(_) => Tab::Discover,
            AppArg::Library => Tab::Library,
            // the Addons list is the account pill's other page (it opens from the account menu)
            AppArg::Settings | AppArg::Addons => Tab::Settings,
            AppArg::Search | AppArg::SearchFor(_) => Tab::Search,
            _ => return None,
        })
    }

    /// Its pill's focus element (Settings': the account pill).
    pub fn elem(self) -> u32 {
        self.entry().map_or(ACCOUNT_ELEM, |e| STRIP_BASE + e.id)
    }

    /// The destination whose pill `elem` is (the account pill's is Settings).
    pub fn of_elem(elem: u32) -> Option<Tab> {
        if elem == ACCOUNT_ELEM {
            return Some(Tab::Settings);
        }
        let id = elem.checked_sub(STRIP_BASE)?;
        TABS.iter().find(|e| e.id == id).map(|e| e.tab)
    }

    pub fn label(self) -> &'static str {
        self.entry().map_or_else(sn_platform::i18n::msg::app_tab_settings, |e| (e.label)())
    }

    /// The neighbour `step` pills along, wrapping at the ends. From Settings (the account pill,
    /// left of Home) the walk starts at the bar's ends: forward is Home, back is Search.
    pub fn step(self, step: i32) -> Tab {
        let n = TABS.len() as i32;
        let i = match self.index() {
            Some(i) => (i as i32 + step).rem_euclid(n),
            None if step > 0 => (step - 1).rem_euclid(n),
            None => (n + step).rem_euclid(n),
        };
        TABS[i as usize].tab
    }
}

/// Is `elem` one of the bar's controls?
pub fn on_strip(elem: u32) -> bool {
    elem >= STRIP_BASE
}

/// The links every tab page declares: DOWN from the bar enters the page's `first` group, UP from
/// it reaches the bar.
pub fn links(first: GroupId, out: &mut Vec<Link>) {
    out.push(Link { from: STRIP, dir: Dir::Down, to: first });
    out.push(Link { from: first, dir: Dir::Up, to: STRIP });
}

/// **BACK on a tab page.** On the page's content it is the page's: focus returns to the page's
/// own pill and the press is spent. On the bar it is the container's (`Handled::No`): a pop to
/// Home, or at Home the application's root BACK.
pub fn on_back(entry: EntryId, tab: Tab, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
    match cx.focus.current.filter(|k| k.entry == entry) {
        Some(k) if on_strip(k.elem) => Handled::No,
        _ => {
            reseat(FocusTarget::Elem(FocusKey { entry, elem: tab.elem() }), fx);
            fx.invalidate(sn_machine::present::Provenance::Input);
            Handled::Yes
        }
    }
}

/// **Does a tab page's focus rest where it should?** A seat counts when it is on the page's
/// content, or where the viewer put it (`chose`), or on the page's OWN pill while the page has no
/// content to seat (`has_content`). Anything else — no focus yet, the pill the remote arrived on
/// from another tab, the own pill while content stands waiting — is a seat nobody chose, which the
/// page's `FirstSeat` takes over.
pub fn rests(entry: EntryId, tab: Tab, cx: &Cx<'_, Bundle>, chose: bool, has_content: bool) -> bool {
    match cx.focus.current.filter(|k| k.entry == entry) {
        None => false,
        Some(_) if chose => true,
        Some(k) if !on_strip(k.elem) => true,
        Some(k) => k.elem == tab.elem() && !has_content,
    }
}

/// Was this focus move the viewer's own: a direction or the pointer, from somewhere on this page?
/// A tab's arrival seat is reported as a direction (`FocusTarget::FirstInGroupAnimated`) but
/// comes from no focus on the page, and is not.
pub fn deliberate(entry: EntryId, from: Option<FocusKey<u32>>, by: sn_machine::machine::By) -> bool {
    matches!(by, sn_machine::machine::By::Dir | sn_machine::machine::By::Pointer) && from.is_some_and(|f| f.entry == entry)
}

/// **A pill was pressed on a tab page.** Another destination is a `SelectTab`; the page's own
/// pill enters the page (`first`, when it has something to focus); the account pill opens the
/// account menu ([`open_account_menu`]), on every page. `false` when `elem` is not a pill.
pub fn on_pill(elem: u32, here: Tab, first: Option<GroupId>, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> bool {
    if elem == ACCOUNT_ELEM {
        open_account_menu(cx, fx);
        return true;
    }
    let Some(tab) = Tab::of_elem(elem) else { return false };
    if tab == here {
        if let Some(group) = first {
            reseat(FocusTarget::ContainerGroup(group), fx);
        }
    } else {
        fx.push(Fx::Nav(NavOp::SelectTab(tab.arg())));
    }
    true
}

/// **The account menu**, hung under the account pill of the page that pressed it (focus returns
/// to the pill when it closes). Its rows are `crate::menu`'s (`MenuKind::Account`).
pub fn open_account_menu(cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
    let host = match (cx.focus.current, cx.owner) {
        (Some(k), _) => k.entry,
        (None, sn_machine::machine::InputOwner::Entry(e)) => e,
        (None, _) => return,
    };
    open_account_menu_at(host, FocusKey { entry: host, elem: ACCOUNT_ELEM }, sn_ui::widgets::CHIP_FRAME, fx);
}

/// The account menu hung beside `anchor`, focus returning to `back` when it closes (Settings'
/// account row opens it in place).
pub fn open_account_menu_at(host: EntryId, back: FocusKey<u32>, anchor: sn_ui::Rect, fx: &mut Effects<'_, Bundle>) {
    fx.push(Fx::Nav(NavOp::Present(AppArg::Menu(MenuArg {
        host,
        reply: fx.from(),
        focus: Some(back),
        anchor: MenuArg::anchor_of(anchor),
        kind: MenuKind::Account,
    }))));
}

/// What a remote hotkey asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey {
    /// The pill to the left of the current one (wrapping).
    PrevTab,
    /// The pill to the right of the current one (wrapping).
    NextTab,
    /// A destination by name.
    Go(Tab),
}

/// The colour buttons, as the dev set's Magic Remote sends them: `wcode` 486-489 with `sym` 0
/// (measured on the dev set by PlxNative, 2026-08-26 — `docs/remote-keys.md`). YELLOW is not a
/// [`hotkey`]: a page reads it from its own input (Home's rows, *See all*).
pub const WCODE_RED: u32 = 486;
pub const WCODE_GREEN: u32 = 487;
pub const WCODE_YELLOW: u32 = 488;
pub const WCODE_BLUE: u32 = 489;
/// The channel rocker on the owner's C1 Magic Remote: CH ▲ 480, CH ▼ 481, `sym` 0 (codes from
/// the beta's unmapped-key log lines 2026-10-17; which is which confirmed by the owner 2026-10-18,
/// after the first reading had them backwards).
pub const WCODE_CH_UP: u32 = 480;
pub const WCODE_CH_DOWN: u32 = 481;

/// The remote key `(sym, wcode)` names, if it is a hotkey. Matched in `wcode` alone, the field
/// the television fills for every remote button above 300.
pub fn hotkey(_sym: u32, wcode: u32) -> Option<Hotkey> {
    use sn_ui::consts::{WCODE_CH_DOWN_KEY, WCODE_CH_UP_KEY};
    Some(match wcode {
        WCODE_RED => Hotkey::PrevTab,
        WCODE_GREEN => Hotkey::NextTab,
        WCODE_BLUE => Hotkey::Go(Tab::Search),
        WCODE_CH_UP | WCODE_CH_UP_KEY => Hotkey::PrevTab,
        WCODE_CH_DOWN | WCODE_CH_DOWN_KEY => Hotkey::NextTab,
        _ => return None,
    })
}

/// Where a hotkey goes from `current`.
pub fn hotkey_target(current: Tab, key: Hotkey) -> Tab {
    match key {
        Hotkey::PrevTab => current.step(-1),
        Hotkey::NextTab => current.step(1),
        Hotkey::Go(tab) => tab,
    }
}

/// Why a hotkey was not taken, or that it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyRoute {
    /// Not a hotkey (or not a key press): the key goes on to the dispatcher.
    NotHotkey,
    /// The television keyboard is up: the key belongs to the text being entered.
    TextEntry,
    /// The player is up: its transport keys are its own.
    Player,
    /// A menu is open over the page: the menu owns the remote.
    Menu,
    /// Taken: the navigation op was requested.
    Went(Tab),
}

/// **Route one raw key press through the hotkeys**, before it reaches the dispatcher. Taken (and
/// so swallowed) only on its DOWN edge and only outside text entry, menus and the player; the
/// current tab is the topmost tab page on the stack (a Detail page pushed over Search is still
/// "on Search"). Going to the tab already under a pushed page returns to it (`PopTo`), it does
/// not open a fresh one.
pub fn route_hotkey(d: &mut Dispatcher<Bundle>, sym: u32, wcode: u32, edge: Edge) -> HotkeyRoute {
    let Some(key) = hotkey(sym, wcode) else { return HotkeyRoute::NotHotkey };
    if d.input.keyboard {
        return HotkeyRoute::TextEntry;
    }
    if matches!(d.top_arg(), Some(AppArg::Player(_))) {
        return HotkeyRoute::Player;
    }
    if !d.nav.modals.surfaces.is_empty() {
        return HotkeyRoute::Menu;
    }
    let (current, at) = current_tab(d).unwrap_or((Tab::Home, None));
    if edge != Edge::Down {
        // a hotkey's repeat and release are spent with its press, never delivered on their own
        return HotkeyRoute::Went(current);
    }
    let target = hotkey_target(current, key);
    let on_top = d.nav.tabs.stack.top().map(|e| e.id) == at;
    if target != current {
        d.request(MachineId::Nav, NavOp::SelectTab(target.arg()));
    } else if let (Some(id), false) = (at, on_top) {
        d.request(MachineId::Nav, NavOp::PopTo(id));
    }
    HotkeyRoute::Went(target)
}

/// The topmost tab page on the shared stack, and its entry.
pub fn current_tab(d: &Dispatcher<Bundle>) -> Option<(Tab, Option<EntryId>)> {
    d.nav.tabs.stack.entries.iter().rev().find_map(|e| Tab::of_arg(&e.arg).map(|t| (t, Some(e.id))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_destination_round_trips_through_its_arg_and_its_pill() {
        assert_eq!(Tab::of_arg(&AppArg::LiveTv), Some(Tab::Discover), "Live TV is Discover's sub-tab");
        for (i, e) in TABS.iter().enumerate() {
            assert_eq!(Tab::of_arg(&e.tab.arg()), Some(e.tab));
            assert_eq!(Tab::of_elem(e.tab.elem()), Some(e.tab));
            assert_eq!(e.tab.index(), Some(i));
            assert!(!e.tab.label().is_empty());
        }
        assert_eq!(TABS.last().map(|e| e.tab), Some(Tab::Search), "the strip draws its last pill as the magnifier");
        let ids: std::collections::BTreeSet<u32> = TABS.iter().map(|e| e.id).chain([ACCOUNT_ELEM - STRIP_BASE]).collect();
        assert_eq!(ids.len(), TABS.len() + 1, "strip ids are unique, the account pill's among them");
    }

    #[test]
    fn the_hotkeys_walk_the_bar_and_name_one_destination() {
        assert_eq!(hotkey(0, WCODE_BLUE), Some(Hotkey::Go(Tab::Search)));
        assert_eq!(hotkey(0, WCODE_YELLOW), None, "YELLOW is the page's (a row's See all), no tab");
        assert_eq!(hotkey(0, 72), None, "PAUSE is no hotkey");
        assert_eq!(hotkey_target(Tab::Home, Hotkey::NextTab), Tab::Discover);
        assert_eq!(hotkey_target(Tab::Discover, Hotkey::NextTab), Tab::Library);
        assert_eq!(hotkey_target(Tab::Library, Hotkey::PrevTab), Tab::Discover);
        assert_eq!(hotkey_target(Tab::Home, Hotkey::PrevTab), Tab::Search, "the walk wraps");
        assert_eq!(hotkey_target(Tab::Search, Hotkey::NextTab), Tab::Home);
        assert_eq!(hotkey(0, sn_ui::consts::WCODE_CH_DOWN_KEY), Some(Hotkey::NextTab));
        assert_eq!(hotkey(0, sn_ui::consts::WCODE_CH_UP_KEY), Some(Hotkey::PrevTab));
    }

    /// Settings and Addons live behind the account pill: neither is a pill, both pages are "on"
    /// the account pill, and the colour/channel walk goes straight from Library to Search.
    #[test]
    fn settings_and_addons_are_the_account_pills_and_the_walk_skips_them() {
        assert_eq!(TABS.iter().map(|e| e.tab).collect::<Vec<_>>(), [Tab::Home, Tab::Discover, Tab::Library, Tab::Search]);
        assert_eq!(Tab::of_arg(&AppArg::Settings), Some(Tab::Settings), "the Settings route still opens its page");
        assert_eq!(Tab::of_arg(&AppArg::Addons), Some(Tab::Settings));
        assert_eq!((Tab::Settings.index(), Tab::Settings.elem()), (None, ACCOUNT_ELEM));
        assert_eq!(Tab::of_elem(ACCOUNT_ELEM), Some(Tab::Settings));
        assert_eq!(hotkey_target(Tab::Library, Hotkey::NextTab), Tab::Search);
        assert_eq!(hotkey_target(Tab::Search, Hotkey::PrevTab), Tab::Library);
        assert_eq!(hotkey_target(Tab::Settings, Hotkey::NextTab), Tab::Home, "from the account pill, forward is Home");
        assert_eq!(hotkey_target(Tab::Settings, Hotkey::PrevTab), Tab::Search, "and back is the bar's other end");
        for code in [WCODE_RED, WCODE_GREEN, WCODE_BLUE, WCODE_CH_UP, WCODE_CH_DOWN] {
            assert!(!matches!(hotkey(0, code), Some(Hotkey::Go(Tab::Settings))), "no new key names the account pill");
        }
    }

    /// The channel rocker as the owner's C1 Magic Remote sends it, from the beta's
    /// `input: unmapped key` log lines: CH ▲ is wcode 480, CH ▼ is 481 (the owner found the
    /// first reading, 481 ▲, backwards in the player: CH + went back, CH − on).
    #[test]
    fn the_measured_channel_rocker_switches_tabs() {
        assert_eq!(hotkey(0, 480), Some(Hotkey::PrevTab));
        assert_eq!(hotkey(0, 481), Some(Hotkey::NextTab));
    }
}
