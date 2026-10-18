//! The top bar, the hotkeys, Search and the item menu (Discover's are `discover_tests`) through the real dispatcher
//! (`test_support::TestApp`): where each key goes, what each page asks of its store, and what
//! the remote may not do while text is being entered, a menu is open or the player is up.

use sn_machine::machine::{Key, MachineId, NavOp};
use sn_stremio::search::RowState;
use sn_stremio::types::{CatalogPage, MetaPreview};

use crate::registry::{AppArg, AppFx, GridCmd, MenuKind, SearchCmd};
use crate::stores::catalog::{CatalogStore, Job, Landing};
use crate::stores::search::{SearchLanding, DEBOUNCE_MS};
use crate::stores::StoreId;
use crate::tabs::{HotkeyRoute, Tab, WCODE_BLUE, WCODE_GREEN, WCODE_RED, WCODE_YELLOW};
use crate::test_support::TestApp;

const CINEMETA: &str = r#"{
    "id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta",
    "resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"],
    "catalogs":[
        {"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]},
        {"type":"series","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]}
    ]}"#;
const LIVE: &str = r#"{
    "id":"org.example.live","version":"1.0.0","name":"Live",
    "resources":["catalog","meta","stream"],"types":["tv"],
    "catalogs":[
        {"type":"tv","id":"channels","name":"Channels","extra":[{"name":"genre","isRequired":true,"options":["News & Talk","Sport","Kids"]},{"name":"skip"},{"name":"search"}]}
    ]}"#;

fn page(prefix: &str, n: usize) -> CatalogPage {
    CatalogPage {
        metas: (0..n)
            .map(|i| MetaPreview { id: format!("{prefix}{i}"), kind: "movie".into(), name: format!("Title {i}"), ..Default::default() })
            .collect(),
    }
}

/// Cinemeta (and, when `live`, the live addon) landed, every row's first page landed.
fn store(live: bool) -> CatalogStore {
    let urls: Vec<&str> = if live {
        vec!["https://a.example/cinemeta/manifest.json", "https://a.example/live/manifest.json"]
    } else {
        vec!["https://a.example/cinemeta/manifest.json"]
    };
    let mut s = CatalogStore::new(urls);
    for job in s.take_jobs() {
        if let Job::Manifest { source, .. } = job {
            let body = if source == 0 { CINEMETA } else { LIVE };
            s.land(Landing::Manifest { source, result: Ok(body.as_bytes().to_vec()) });
        }
    }
    s.set_view(0..4);
    for _ in 0..4 {
        for job in s.take_jobs() {
            if let Job::Page { row, .. } = job {
                s.land(Landing::Page { row, result: Ok(page(&format!("tt{}-{}-", row.source, row.catalog), 20)) });
            }
        }
    }
    s
}

fn boot(live: bool) -> TestApp {
    let s = store(live);
    let gen = s.gen();
    let mut app = TestApp::boot(s);
    app.notify(StoreId::Catalog, gen);
    app.frames(4);
    app
}

fn focus_elem(app: &TestApp) -> Option<u32> {
    app.d.focus().map(|k| k.elem)
}

/// Settings, as a pick from the account pill's menu asks for it (no colour key names it).
fn to_settings(app: &mut TestApp) {
    app.d.request(MachineId::Nav, NavOp::SelectTab(AppArg::Settings));
    app.frames(40);
}

fn on_pill(app: &TestApp, tab: Tab) -> bool {
    focus_elem(app) == Some(tab.elem())
}

#[test]
fn up_from_the_first_row_reaches_the_bar_and_left_right_walk_the_tabs() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    assert!(focus_elem(&app).is_some_and(|e| !crate::tabs::on_strip(e)), "Home seats its first row");
    app.press(Key::Up);
    let all_chip = focus_elem(&app);
    assert!(all_chip.is_some_and(|e| !crate::tabs::on_strip(e)), "UP from the first row reaches the type chips: {:?}", app.d.focus());
    app.press(Key::Up);
    assert!(on_pill(&app, Tab::Home), "UP from the chips reaches the bar, on the page's own pill: {:?}", app.d.focus());
    app.press(Key::Right);
    assert!(on_pill(&app, Tab::Discover), "RIGHT walks to the next pill");
    app.press(Key::Right);
    app.press(Key::Left);
    assert!(on_pill(&app, Tab::Discover), "LEFT walks back");
    app.press(Key::Down);
    assert_eq!(focus_elem(&app), all_chip, "DOWN from Home's pill enters the page at its chips");
    assert_eq!(app.top_name(), "home", "walking the pills selects nothing");
}

#[test]
fn ok_on_a_pill_opens_its_tab_and_back_returns_through_the_bar_to_home() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.press(Key::Up); // the type chips
    app.press(Key::Up);
    app.press(Key::Right);
    assert!(on_pill(&app, Tab::Discover));
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "discover", "OK on a pill selects its destination");
    assert_eq!(app.d.nav.tabs.stack.entries.len(), 2, "a tab covers Home, the one root");
    up_to_the_bar(&mut app);
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home", "BACK on the bar of another tab returns to Home");
    // Settings is behind the account pill now: LEFT of Home, OK, the menu's first row
    up_to_the_bar(&mut app);
    app.press(Key::Left);
    assert!(on_pill(&app, Tab::Settings), "LEFT from Home reaches the account pill: {:?}", app.d.focus());
    app.press(Key::Ok);
    app.frames(3);
    app.press(Key::Ok);
    app.frames(3);
    assert_eq!(app.top_name(), "settings", "the account menu's Settings opens the page");
    assert_eq!(app.d.nav.tabs.stack.entries.len(), 2, "a tab covers Home, the one root");
    // into the page and back out to the bar
    if crate::tabs::on_strip(focus_elem(&app).unwrap_or(0)) {
        app.press(Key::Down);
    }
    assert!(focus_elem(&app).is_some_and(|e| !crate::tabs::on_strip(e)), "the page's content holds focus: {:?}", app.d.focus());
    app.press(Key::Back);
    assert_eq!(app.top_name(), "settings", "BACK on the content stays on the page…");
    assert!(on_pill(&app, Tab::Settings), "…and returns focus to its pill: {:?}", app.d.focus());
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home", "BACK on the bar of another tab returns to Home");
    assert_eq!(app.rig.back_at_root, 0);
}

#[test]
fn back_on_homes_bar_is_the_applications_root_back() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.press(Key::Back);
    assert!(on_pill(&app, Tab::Home), "BACK on Home's rows goes to the bar first");
    assert_eq!(app.rig.back_at_root, 0);
    app.press(Key::Back);
    assert_eq!(app.rig.back_at_root, 1, "BACK on the bar at Home hands the screen to the television's Home");
    assert_eq!(app.top_name(), "home", "and the app keeps running on Home");
}

#[test]
fn the_colour_keys_switch_tabs_from_anywhere() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    assert_eq!(app.press_raw(0, WCODE_BLUE), HotkeyRoute::Went(Tab::Search));
    assert_eq!(app.top_name(), "search", "BLUE is Search");
    assert_eq!(app.press_raw(0, WCODE_YELLOW), HotkeyRoute::NotHotkey, "YELLOW is no tab shortcut (a row's See all)");
    assert_eq!(app.top_name(), "search", "and Search has no card row to open");
    to_settings(&mut app);
    app.press_raw(0, WCODE_GREEN);
    assert_eq!(app.top_name(), "home", "GREEN is the next pill (Settings, on the account pill left of Home → Home)");
    app.press_raw(0, WCODE_RED);
    assert_eq!(app.top_name(), "search", "RED is the previous pill, and the walk wraps");
    app.press_raw(0, sn_ui::consts::WCODE_CH_UP_KEY);
    assert_eq!(app.top_name(), "library", "CH+ is the previous pill too (Settings is no pill)");
    app.press_raw(0, sn_ui::consts::WCODE_CH_DOWN_KEY);
    assert_eq!(app.top_name(), "search", "CH− the next");
    assert_eq!(app.press_raw(0, 72), HotkeyRoute::NotHotkey, "an ordinary key goes on to the page");
}

#[test]
fn a_hotkey_from_a_pushed_page_leaves_it_and_its_own_tab_returns_to_the_tab() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail");
    app.press_raw(0, WCODE_BLUE);
    assert_eq!(app.top_name(), "search", "from a Detail page over Home, BLUE still goes to Search");
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::Settings));
    app.frames(40);
    // a Detail page pushed over Search: the current tab is still Search
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::Detail(crate::registry::ItemRef { kind: "movie".into(), id: "tt1".into(), name: "X".into(), from: String::new() })));
    app.frames(40);
    assert_eq!(app.top_name(), "detail");
    to_settings(&mut app);
    assert_eq!(app.top_name(), "settings");
}

#[test]
fn no_hotkey_while_text_is_entered_a_menu_is_open_or_the_player_is_up() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    // the player
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::Player(crate::registry::PlayerArg { title: "T".into(), item: None, addon: String::new(), resumed_ms: 0 })));
    app.frames(40);
    assert_eq!(app.top_name(), "player");
    assert_eq!(app.press_raw(0, WCODE_BLUE), HotkeyRoute::Player);
    app.frames(10);
    assert_eq!(app.top_name(), "menu", "the player keeps its keys: BLUE is its show info");
    app.press(Key::Back);
    app.frames(10);
    assert_eq!(app.top_name(), "player");
    // nothing playing, so no picture for the transport to be put away over: BACK leaves
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home");
    // a menu
    app.frames(10);
    app.hold_ok();
    assert_eq!(app.top_name(), "menu", "a held OK opens the card's menu");
    assert_eq!(app.press_raw(0, WCODE_BLUE), HotkeyRoute::Menu);
    assert_eq!(app.top_name(), "menu");
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home", "BACK dismisses the menu");
    // text entry
    app.press_raw(0, WCODE_BLUE);
    assert_eq!(app.top_name(), "search");
    assert!(focus_elem(&app).is_some(), "Search seats its field");
    app.press(Key::Ok);
    assert_eq!(app.rig.keyboard.last(), Some(&true), "OK on the field raises the television keyboard");
    assert!(app.d.input.keyboard);
    assert_eq!(app.press_raw(0, WCODE_RED), HotkeyRoute::TextEntry);
    assert_eq!(app.top_name(), "search", "a colour key while typing is the keyboard's");
}

#[test]
fn hold_ok_opens_the_item_menu_and_signed_out_its_library_row_signs_in() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.hold_ok();
    assert_eq!(app.top_name(), "menu");
    let Some(AppArg::Menu(m)) = app.d.nav.modals.surfaces.last().map(|s| s.entry.arg.clone()) else {
        panic!("the menu is a surface over Home")
    };
    let MenuKind::Item { item, .. } = &m.kind else { panic!("an item menu") };
    assert_eq!(item.id, "tt0-0-0");
    let menu = app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::menu::MenuScreen>()).expect("the menu screen");
    let rows: Vec<(String, bool)> = menu.table().sections[0].rows.iter().map(|r| (r.label.clone(), r.dim)).collect();
    assert_eq!(
        rows,
        vec![(sn_platform::i18n::msg::app_menu_open().into(), false), (sn_platform::i18n::msg::app_menu_sign_in_to_add().into(), false)],
        "Open, and signed out the library row signs in"
    );
    app.press(Key::Down);
    app.press(Key::Ok);
    app.frames(5);
    assert_eq!(app.top_arg(), Some(&AppArg::SignIn), "it opens the sign-in");
    app.press(Key::Back);
    app.frames(5);
    app.hold_ok();
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail", "Open opens the item's page");
    assert_eq!(app.top_arg(), Some(&AppArg::Detail(item.clone())));
}

#[test]
fn signed_in_add_to_library_asks_the_library_store_and_then_waits_for_it() {
    use crate::registry::{AppFx, LibraryCmd};
    use crate::stores::account::{Account, AccountLanding, AccountStore, Secret, Stored};
    use crate::stores::library::{LibraryLanding, LibraryStore};
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    let mut account = AccountStore::new();
    let _ = account.land(AccountLanding::Restored(Stored::Session(Account { user_id: "u1".into(), email: "v@example.com".into(), auth_key: Secret::new("k") })), 0);
    let mut library = LibraryStore::new();
    let _ = library.sign_in("u1", &Secret::new("k"));
    let _ = library.land(LibraryLanding::Fetched { user_id: "u1".into(), result: Ok(Vec::new()) });
    app.rig.account = account;
    app.rig.library = library;
    app.notify(StoreId::Account, 7);
    app.notify(StoreId::Library, 7);
    app.frames(3);
    let menu_rows = |app: &TestApp| -> Vec<(String, String, bool)> {
        let menu = app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::menu::MenuScreen>()).expect("the menu screen");
        menu.table().sections[0].rows.iter().map(|r| (r.label.clone(), r.detail.clone(), r.dim)).collect()
    };
    app.hold_ok();
    assert_eq!(app.top_name(), "menu");
    assert_eq!(menu_rows(&app)[1], (sn_platform::i18n::msg::app_menu_add_to_library().into(), String::new(), false), "offered signed in");
    app.take_effects();
    app.press(Key::Down);
    app.press(Key::Ok);
    let adds: Vec<sn_stremio::library::NewEntry> = app
        .take_effects()
        .into_iter()
        .filter_map(|fx| match fx {
            AppFx::Library(LibraryCmd::Add(e)) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(adds.len(), 1, "one write asked for");
    assert_eq!((adds[0].id.as_str(), adds[0].kind.as_str()), ("tt0-0-0", "movie"));
    assert_ne!(app.top_name(), "menu", "the menu closes");
    // the rig does not run the store; the application's bridge does — stand in for it
    let _ = app.rig.library.add(adds[0].clone());
    app.notify(StoreId::Library, 8);
    app.frames(2);
    app.hold_ok();
    let row = menu_rows(&app)[1].clone();
    assert_eq!((row.1.as_str(), row.2), (sn_platform::i18n::msg::app_menu_adding(), true), "the write is on its way");
}

#[test]
fn search_types_debounced_and_each_row_reports_its_own_state() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(true);
    app.press_raw(0, WCODE_BLUE);
    assert_eq!(app.top_name(), "search");
    app.press(Key::Ok);
    app.type_text("ba");
    app.type_text("t");
    assert_eq!(app.rig.search.typed(), "bat");
    assert!(app.rig.search_jobs.is_empty(), "nothing is asked while typing");
    app.frames((DEBOUNCE_MS / 16) as usize + 2);
    let jobs = std::mem::take(&mut app.rig.search_jobs);
    let urls: Vec<&str> = jobs.iter().map(|j| j.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://a.example/cinemeta/catalog/movie/top/search=bat.json",
            "https://a.example/cinemeta/catalog/series/top/search=bat.json",
            "https://a.example/live/catalog/tv/channels/genre=News%20%26%20Talk&search=bat.json",
        ],
        "one request per searchable catalog of every addon, the live tv catalog included"
    );
    let qgen = jobs[0].qgen;
    app.rig.search.land(SearchLanding { qgen, row: 0, result: Ok(page("s", 30)) });
    app.rig.search.land(SearchLanding { qgen, row: 1, result: Ok(page("s", 0)) });
    app.rig.search.land(SearchLanding { qgen, row: 2, result: Err(sn_stremio::FetchError::Status(503)) });
    app.frames(4);
    assert_eq!(app.rig.search.row_state(0), Some(RowState::Results { count: 30, more: true }));
    assert_eq!(app.rig.search.row_state(1), Some(RowState::Empty));
    assert_eq!(app.rig.search.row_state(2), Some(RowState::Failed(sn_stremio::FetchError::Status(503))));
    // OK commits the query to the recents
    app.press(Key::Ok);
    assert!(!app.d.input.keyboard, "OK puts the keyboard away");
    assert_eq!(app.rig.search.recents(), ["bat"]);
}

#[test]
fn a_search_rows_more_opens_its_whole_catalog_seeded_with_the_row() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.press_raw(0, WCODE_BLUE);
    let addons = app.rig.catalog.addons().to_vec();
    let jobs = app.rig.search.submit("bat", &addons, &sn_stremio::search::AddonFilter::default());
    app.rig.search.land(SearchLanding { qgen: jobs[0].qgen, row: 0, result: Ok(page("s", 30)) });
    app.frames(4);
    app.take_effects();
    // DOWN from the field: past the filter chip to the first row's MORE heading
    app.press(Key::Down);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "catalog", "MORE opens the catalog's grid: {:?}", app.d.focus());
    let Some(AppArg::Catalog(r)) = app.top_arg().cloned() else { panic!("a catalog page") };
    assert_eq!(r.key.search.as_deref(), Some("bat"));
    assert!(app.take_effects().iter().any(|fx| matches!(fx, AppFx::Grid(GridCmd::Open(k)) if *k == r.key)));
    assert_eq!(app.rig.grid.items(&r.key).len(), 30, "the grid opens on the cards the row holds");
    assert_eq!(app.rig.grid.next_url(&r.key).as_deref(), Some("https://a.example/cinemeta/catalog/movie/top/search=bat&skip=30.json"));
    app.press(Key::Back);
    assert_eq!(app.top_name(), "search");
    assert!(app.take_effects().iter().any(|fx| matches!(fx, AppFx::Grid(GridCmd::Close(_)))), "leaving the grid lets its view go");
}

/// Search's addon filter is one dropdown whose popover toggles each addon in place (it stays
/// open), with Select all / Select none first; the switch is a setting.
#[test]
fn the_addon_filter_is_a_dropdown_and_a_setting() {
    use crate::registry::ChoiceTarget;
    let _g = sn_base::testlock::serial();
    let _l = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot(true);
    app.press_raw(0, WCODE_BLUE);
    app.press(Key::Down);
    assert!(app.drawn_text().iter().any(|t| t == "All addons"), "the button says what is searched");
    let filter_gen = app.rig.addons.gen();
    app.press(Key::Ok);
    let values = |app: &TestApp| match crate::discover_tests::menu_of(app) {
        MenuKind::List { rows, target: ChoiceTarget::SearchAddons, .. } => rows.into_iter().map(|r| (r.label, r.value)).collect::<Vec<_>>(),
        other => panic!("the addon filter: {other:?}"),
    };
    let rows = values(&app);
    assert_eq!((rows[0].0.as_str(), rows[1].0.as_str()), ("Select all", "Select none"));
    assert!(rows[2..].iter().all(|(_, v)| v == "On"), "{rows:?}");
    let (anchor, panel) = crate::discover_tests::menu_panel(&app);
    assert!(panel.is_some_and(|p| p.y >= anchor.y + anchor.h), "under its button");
    app.press(Key::Down);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert!(app.take_effects().iter().any(|fx| matches!(fx, AppFx::Search(SearchCmd::ToggleAddon(id)) if id == "com.linvo.cinemeta")));
    assert!(!app.rig.addons.filter().allows("com.linvo.cinemeta"), "the first addon's row switched Cinemeta off");
    assert_ne!(app.rig.addons.gen(), filter_gen, "and the switch is the addons store's (persisted with the account's files)");
    app.frames(2);
    assert_eq!(app.top_name(), "menu", "the popover stays open");
    assert_eq!(values(&app)[2].1, "Off", "and shows the switch");
    // Select none, then Select all
    app.press(Key::Up);
    app.press(Key::Ok);
    app.frames(2);
    assert!(values(&app)[2..].iter().all(|(_, v)| v == "Off"));
    app.press(Key::Up);
    app.press(Key::Ok);
    app.frames(2);
    assert!(values(&app)[2..].iter().all(|(_, v)| v == "On"));
    // Cinemeta off again, BACK closes and the button says so
    app.press(Key::Down);
    app.press(Key::Down);
    app.press(Key::Ok);
    app.press(Key::Back);
    assert_eq!(app.top_name(), "search");
    let addons = app.rig.catalog.addons().to_vec();
    let jobs = app.rig.search.submit("news", &addons, app.rig.addons.filter());
    assert_eq!(jobs.len(), 1, "only the live addon is asked");
}

#[test]
fn a_subtitle_picker_sets_the_setting_and_returns() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    to_settings(&mut app);
    assert_eq!(app.top_name(), "settings");
    // the Account section comes first: walk down to the subtitle size row
    let size = sn_ui::form::FormId::key(&crate::settings::RootRow::SubtitleSize).0;
    for _ in 0..12 {
        if focus_elem(&app) == Some(size) {
            break;
        }
        app.press(Key::Down);
    }
    assert_eq!(focus_elem(&app), Some(size));
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "settings-page", "the subtitle size row opens its picker");
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "settings", "a pick returns to the root");
    use crate::stores::settings::Pref;
    assert_eq!(app.rig.settings.get(Pref::SubtitleSize), 2, "Medium → Large");
}

/// Hold OK on the focused card and check the opener lift: while the menu is up the card (its
/// caption, `title`) is drawn ABOVE the menu, by the lift, since the page pass leaves it out; after
/// BACK and the fade the page draws it again.
pub(crate) fn the_held_card_stays_drawn(app: &mut TestApp, title: &str) {
    let page = app.top_name();
    app.hold_ok();
    assert_eq!(app.top_name(), "menu", "a held OK on a {page} card opens its menu");
    let runs = app.drawn_text();
    let open = runs.iter().position(|r| r == sn_platform::i18n::msg::app_menu_open()).expect("the menu draws");
    assert!(runs[open..].iter().any(|r| r == title), "{page}: the held card {title:?} is drawn above its menu: {runs:?}");
    app.press(Key::Back);
    app.frames(40);
    assert_eq!(app.top_name(), page, "BACK closes the menu");
    assert_eq!(app.d.lifted_opener(), None);
    let runs = app.drawn_text();
    assert!(runs.iter().any(|r| r == title), "{page}: the page draws the card again once the menu is gone: {runs:?}");
}

/// How many times the popover over the page (open or still fading out) draws `label` itself.
fn in_popover(app: &TestApp, label: &str) -> usize {
    let Some(AppArg::Menu(m)) = app.d.nav.modals.surfaces.last().map(|s| &s.entry.arg) else { return 0 };
    let mut words: Vec<&str> = Vec::new();
    match &m.kind {
        MenuKind::Choice { title, options, .. } => words.extend(std::iter::once(title.as_str()).chain(options.iter().map(String::as_str))),
        MenuKind::List { title, rows, .. } => words.extend(std::iter::once(title.as_str()).chain(rows.iter().flat_map(|r| [r.label.as_str(), r.value.as_str()]))),
        MenuKind::Item { .. } | MenuKind::Subtitles | MenuKind::Account | MenuKind::Player(_) => {}
    }
    words.iter().filter(|w| **w == label).count()
}

/// The page draws its dropdown button `elem` saying `label` on this frame (beside whatever the
/// popover draws): its stop is on the hit map and its word among the runs.
fn button_drawn(app: &mut TestApp, elem: u32, label: &str) -> bool {
    let stop = app.d.last_stops().iter().any(|s| s.key.elem == elem);
    let runs = app.drawn_text();
    stop && runs.iter().filter(|r| *r == label).count() > in_popover(app, label)
}

/// **The dropdown button is drawn on every frame of its popover's life.** OK on the focused button
/// saying `before` opens the popover; row `n` is picked (a multi-select toggles it and BACK
/// closes); on every frame while the popover is up, while it fades and after, the page draws the
/// button — saying `after` once the pick applies. A button that left the page while its popover was
/// up, or that a pick took off the page, read on the set as a dropdown that vanished.
pub(crate) fn the_dropdown_button_stays_drawn(app: &mut TestApp, before: &str, n: usize, after: &str) {
    let page = app.top_name();
    let elem = app.d.focus().expect("focus on the button").elem;
    assert!(button_drawn(app, elem, before), "{page}: the button says {before:?}: {:?}", app.drawn_text());
    app.tap(Key::Ok);
    for i in 0..45 {
        assert!(button_drawn(app, elem, before), "{page}: frame {i} of the popover opening: {:?}", app.drawn_text());
        app.frame();
    }
    assert_eq!(app.top_name(), "menu", "{page}: OK opened the popover");
    let current = match app.d.nav.modals.surfaces.last().map(|s| &s.entry.arg) {
        Some(AppArg::Menu(m)) => match &m.kind {
            MenuKind::Choice { current, .. } => *current,
            _ => 0,
        },
        _ => 0,
    };
    for _ in 0..n.abs_diff(current) {
        app.press(if n > current { Key::Down } else { Key::Up });
    }
    let stays = match app.d.nav.modals.surfaces.last().map(|s| &s.entry.arg) {
        Some(AppArg::Menu(m)) => crate::dropdown::stays_open(&m.kind),
        _ => false,
    };
    app.tap(Key::Ok);
    if stays {
        // a multi-select toggles in place
        for i in 0..20 {
            assert!(button_drawn(app, elem, after), "{page}: frame {i} after the toggle: {:?}", app.drawn_text());
            app.frame();
        }
        app.tap(Key::Back);
    }
    for i in 0..90 {
        assert!(button_drawn(app, elem, after), "{page}: frame {i} after the pick: {:?}", app.drawn_text());
        app.frame();
    }
    assert_eq!(app.top_name(), page, "{page}: the popover closed");
    assert_eq!(app.d.lifted_opener(), None, "{page}: the lift stood down");
    assert_eq!(app.d.focus().map(|k| k.elem), Some(elem), "{page}: focus is back on the button");
}

#[test]
fn a_held_card_stays_drawn_while_its_menu_is_up_on_home_search_and_the_catalog_grid() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.frames(20);
    the_held_card_stays_drawn(&mut app, "Title 0");
    // Search's poster shelves
    app.press_raw(0, WCODE_BLUE);
    let addons = app.rig.catalog.addons().to_vec();
    let jobs = app.rig.search.submit("bat", &addons, &sn_stremio::search::AddonFilter::default());
    app.rig.search.land(SearchLanding { qgen: jobs[0].qgen, row: 0, result: Ok(page("s", 30)) });
    app.frames(4);
    app.press(Key::Down);
    app.press(Key::Down);
    app.press(Key::Down);
    app.press(Key::Right);
    app.frames(20);
    the_held_card_stays_drawn(&mut app, "Title 2");
    // the row's whole catalog, a grid
    app.press(Key::Up);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "catalog", "MORE opens the catalog's grid: {:?}", app.d.focus());
    app.press(Key::Down);
    app.press(Key::Right);
    app.frames(20);
    the_held_card_stays_drawn(&mut app, "Title 7");
}

/// Search's Addons dropdown (a multi-select: OK toggles in place, BACK closes) stays on the page.
#[test]
fn the_dropdown_button_stays_drawn_while_open_and_after_a_pick() {
    let _g = sn_base::testlock::serial();
    let _l = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot(true);
    app.press_raw(0, WCODE_BLUE);
    app.press(Key::Down);
    // Cinemeta's row switched off leaves the one addon on, named
    the_dropdown_button_stays_drawn(&mut app, "All addons", 2, "Live");
    the_dropdown_button_stays_drawn(&mut app, "Live", 0, "All addons");
}

/// UP from `app`'s page content until focus reaches the bar (a page's rows sit under one another).
fn up_to_the_bar(app: &mut TestApp) {
    if focus_elem(app).is_some_and(crate::tabs::on_strip) {
        app.press(Key::Down);
    }
    assert!(focus_elem(app).is_some_and(|e| !crate::tabs::on_strip(e)), "{}: focus on the page's content: {:?}", app.top_name(), app.d.focus());
    for _ in 0..6 {
        if focus_elem(app).is_some_and(crate::tabs::on_strip) {
            return;
        }
        app.press(Key::Up);
    }
    panic!("{}: UP never reached the bar: {:?}", app.top_name(), app.d.focus());
}

/// **The bar marks the page shown, and focus entering it lands on that page's pill.** The selected
/// pill follows every way the page changes (a colour key, the channel rocker, OK on a pill); UP
/// from the content enters the bar on the CURRENT page's pill rather than the pill nearest the
/// control it left (Discover's type dropdown, at the left margin, sits under Home's); LEFT/RIGHT
/// in the bar move focus without changing the page, and OK then switches it.
#[test]
fn the_bar_marks_the_page_shown_and_focus_enters_it_on_that_pill() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    let selected = |app: &TestApp| Some(app.rig.chrome.selected());
    assert_eq!(selected(&app), Some(Tab::Home));
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    assert_eq!((app.top_name(), selected(&app)), ("discover", Some(Tab::Discover)), "a colour key moves the mark");
    up_to_the_bar(&mut app);
    assert!(on_pill(&app, Tab::Discover), "UP enters the bar on the page's own pill: {:?}", app.d.focus());
    app.press_raw(0, sn_ui::consts::WCODE_CH_DOWN_KEY);
    app.frames(3);
    assert_eq!((app.top_name(), selected(&app)), ("library", Some(Tab::Library)), "so does the channel rocker");
    to_settings(&mut app);
    app.frames(3);
    assert_eq!((app.top_name(), selected(&app)), ("settings", Some(Tab::Settings)), "Settings' mark is the account pill's");
    up_to_the_bar(&mut app);
    assert!(on_pill(&app, Tab::Settings), "UP enters the bar on the account pill: {:?}", app.d.focus());
    app.press(Key::Right);
    assert!(on_pill(&app, Tab::Home), "RIGHT moves focus…");
    assert_eq!((app.top_name(), selected(&app)), ("settings", Some(Tab::Settings)), "…and not the page or its mark");
    app.press(Key::Right);
    app.press(Key::Ok);
    app.frames(3);
    assert_eq!((app.top_name(), selected(&app)), ("discover", Some(Tab::Discover)), "OK switches the page, and the mark with it");
    up_to_the_bar(&mut app);
    assert!(on_pill(&app, Tab::Discover), "{:?}", app.d.focus());
    app.press(Key::Back);
    assert_eq!((app.top_name(), selected(&app)), ("home", Some(Tab::Home)), "BACK on the bar returns to Home, and the mark with it");
    up_to_the_bar(&mut app);
    assert!(on_pill(&app, Tab::Home), "{:?}", app.d.focus());
}

/// The hold menu of a title in the library and in Continue Watching: *Remove from library* and
/// *Remove from Continue Watching*, each asking its own write; the store, optimistic, drops the
/// title from Continue Watching at once, so the next hold menu no longer offers it.
#[test]
fn the_item_menu_removes_from_the_library_and_from_continue_watching() {
    use crate::registry::LibraryCmd;
    use crate::stores::account::{Account, AccountLanding, AccountStore, Secret, Stored};
    use crate::stores::library::{LibraryLanding, LibraryStore};
    use sn_platform::i18n::msg;
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot(false);
    let mut account = AccountStore::new();
    let _ = account.land(AccountLanding::Restored(Stored::Session(Account { user_id: "u1".into(), email: "v@example.com".into(), auth_key: Secret::new("k") })), 0);
    let mut library = LibraryStore::new();
    let _ = library.sign_in("u1", &Secret::new("k"));
    let held = r#"[{"_id":"tt0-0-0","type":"movie","name":"Title 0","state":{"timeOffset":5000,"duration":9000,"lastWatched":"2026-10-01T00:00:00Z"}}]"#;
    let _ = library.land(LibraryLanding::Fetched { user_id: "u1".into(), result: Ok(serde_json::from_str(held).unwrap()) });
    app.rig.account = account;
    app.rig.library = library;
    app.notify(StoreId::Account, 7);
    app.notify(StoreId::Library, 7);
    app.frames(3);
    let menu_labels = |app: &TestApp| -> Vec<String> {
        let menu = app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::menu::MenuScreen>()).expect("the menu screen");
        menu.table().sections[0].rows.iter().map(|r| r.label.clone()).collect()
    };
    let library_cmds = |app: &mut TestApp| -> Vec<LibraryCmd> {
        app.take_effects()
            .into_iter()
            .filter_map(|fx| match fx {
                AppFx::Library(c) => Some(c),
                _ => None,
            })
            .collect()
    };
    app.hold_ok();
    assert_eq!(app.top_name(), "menu");
    let MenuKind::Item { item, .. } = crate::discover_tests::menu_of(&app) else { panic!("an item menu") };
    assert_eq!(item.id, "tt0-0-0");
    assert_eq!(
        menu_labels(&app),
        [msg::app_menu_open(), msg::app_menu_remove_from_library(), msg::app_menu_remove_from_continue(), sn_ui::widgets::mark_watched_verb()],
    );
    app.take_effects();
    app.press(Key::Down);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(library_cmds(&mut app), [LibraryCmd::Rewind("tt0-0-0".into())]);
    assert_ne!(app.top_name(), "menu", "the menu closes");
    // the rig does not run the store; the application's bridge does — stand in for it
    assert_eq!(app.rig.library.rewind("tt0-0-0").len(), 1);
    assert!(app.rig.library.continue_watching().is_empty(), "optimistic: off Continue Watching at once");
    app.notify(StoreId::Library, 8);
    app.frames(4);
    app.hold_ok();
    assert_eq!(app.top_name(), "menu");
    assert_eq!(menu_labels(&app), [msg::app_menu_open(), msg::app_menu_remove_from_library(), sn_ui::widgets::mark_watched_verb()], "not continued any more");
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(library_cmds(&mut app), [LibraryCmd::Remove("tt0-0-0".into())]);
}
