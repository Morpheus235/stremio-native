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
//! **Hotkeys** ([`hotkey`], [`route_hotkey`]): the colour keys and the channel rocker switch tabs
//! from anywhere except while text is being entered, a menu is open, or the player is up. The
//! codes and their provenance are `docs/remote-keys.md`.

use sn_machine::machine::{Cx, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, MachineId, NavOp};
use sn_ui::containers::tabs::STRIP;
use sn_ui::dispatch::{Dispatcher, STRIP_BASE};
use sn_ui::screen::{Dir, FocusTarget, Link};

use crate::registry::{reseat, AppArg, Bundle};

/// A top-bar destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Home,
    LiveTv,
    Library,
    Addons,
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

/// The destinations, in bar order. Search is last: the strip draws its last pill as the
/// magnifier.
pub const TABS: [TabEntry; 6] = [
    TabEntry { tab: Tab::Home, id: 0, label: sn_platform::i18n::msg::app_tab_home },
    TabEntry { tab: Tab::LiveTv, id: 1, label: sn_platform::i18n::msg::app_tab_live_tv },
    TabEntry { tab: Tab::Library, id: 2, label: sn_platform::i18n::msg::app_tab_library },
    TabEntry { tab: Tab::Addons, id: 3, label: sn_platform::i18n::msg::app_tab_addons },
    TabEntry { tab: Tab::Settings, id: 4, label: sn_platform::i18n::msg::app_tab_settings },
    TabEntry { tab: Tab::Search, id: 5, label: sn_platform::i18n::msg::app_tab_search },
];

impl Tab {
    fn entry(self) -> &'static TabEntry {
        TABS.iter().find(|e| e.tab == self).expect("every tab has an entry")
    }

    /// The bar position.
    pub fn index(self) -> usize {
        TABS.iter().position(|e| e.tab == self).expect("every tab has an entry")
    }

    pub fn at(index: usize) -> Option<Tab> {
        TABS.get(index).map(|e| e.tab)
    }

    /// The page this destination opens.
    pub fn arg(self) -> AppArg {
        match self {
            Tab::Home => AppArg::Home,
            Tab::LiveTv => AppArg::LiveTv,
            Tab::Library => AppArg::Library,
            Tab::Addons => AppArg::Addons,
            Tab::Settings => AppArg::Settings,
            Tab::Search => AppArg::Search,
        }
    }

    /// The destination a page IS, if it is one.
    pub fn of_arg(arg: &AppArg) -> Option<Tab> {
        Some(match arg {
            AppArg::Home => Tab::Home,
            AppArg::LiveTv => Tab::LiveTv,
            AppArg::Library => Tab::Library,
            AppArg::Addons => Tab::Addons,
            AppArg::Settings => Tab::Settings,
            AppArg::Search => Tab::Search,
            _ => return None,
        })
    }

    /// Its pill's focus element.
    pub fn elem(self) -> u32 {
        STRIP_BASE + self.entry().id
    }

    /// The destination whose pill `elem` is.
    pub fn of_elem(elem: u32) -> Option<Tab> {
        let id = elem.checked_sub(STRIP_BASE)?;
        TABS.iter().find(|e| e.id == id).map(|e| e.tab)
    }

    pub fn label(self) -> &'static str {
        (self.entry().label)()
    }

    /// The neighbour `step` pills along, wrapping at the ends.
    pub fn step(self, step: i32) -> Tab {
        let n = TABS.len() as i32;
        let i = (self.index() as i32 + step).rem_euclid(n);
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
/// pill enters the page (`first`, when it has something to focus). `false` when `elem` is not a
/// pill.
pub fn on_pill(elem: u32, here: Tab, first: Option<GroupId>, fx: &mut Effects<'_, Bundle>) -> bool {
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
/// (measured on the dev set by PlxNative, 2026-08-26 — `docs/remote-keys.md`).
pub const WCODE_RED: u32 = 486;
pub const WCODE_GREEN: u32 = 487;
pub const WCODE_YELLOW: u32 = 488;
pub const WCODE_BLUE: u32 = 489;

/// The remote key `(sym, wcode)` names, if it is a hotkey. Matched in `wcode` alone, the field
/// the television fills for every remote button above 300.
pub fn hotkey(_sym: u32, wcode: u32) -> Option<Hotkey> {
    use sn_ui::consts::{WCODE_CH_DOWN_KEY, WCODE_CH_UP_KEY};
    Some(match wcode {
        WCODE_RED => Hotkey::PrevTab,
        WCODE_GREEN => Hotkey::NextTab,
        WCODE_YELLOW => Hotkey::Go(Tab::Settings),
        WCODE_BLUE => Hotkey::Go(Tab::Search),
        WCODE_CH_UP_KEY => Hotkey::PrevTab,
        WCODE_CH_DOWN_KEY => Hotkey::NextTab,
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
        for (i, e) in TABS.iter().enumerate() {
            assert_eq!(Tab::of_arg(&e.tab.arg()), Some(e.tab));
            assert_eq!(Tab::of_elem(e.tab.elem()), Some(e.tab));
            assert_eq!(e.tab.index(), i);
            assert!(!e.tab.label().is_empty());
        }
        assert_eq!(TABS.last().map(|e| e.tab), Some(Tab::Search), "the strip draws its last pill as the magnifier");
        let ids: std::collections::BTreeSet<u32> = TABS.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), TABS.len(), "strip ids are unique");
    }

    #[test]
    fn the_hotkeys_walk_the_bar_and_name_two_destinations() {
        assert_eq!(hotkey(0, WCODE_BLUE), Some(Hotkey::Go(Tab::Search)));
        assert_eq!(hotkey(0, WCODE_YELLOW), Some(Hotkey::Go(Tab::Settings)));
        assert_eq!(hotkey(0, 72), None, "PAUSE is no hotkey");
        assert_eq!(hotkey_target(Tab::Home, Hotkey::NextTab), Tab::LiveTv);
        assert_eq!(hotkey_target(Tab::Home, Hotkey::PrevTab), Tab::Search, "the walk wraps");
        assert_eq!(hotkey_target(Tab::Search, Hotkey::NextTab), Tab::Home);
        assert_eq!(hotkey(0, sn_ui::consts::WCODE_CH_DOWN_KEY), Some(Hotkey::NextTab));
        assert_eq!(hotkey(0, sn_ui::consts::WCODE_CH_UP_KEY), Some(Hotkey::PrevTab));
    }
}
