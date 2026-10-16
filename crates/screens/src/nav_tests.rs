//! The top bar, the hotkeys, Search, Live TV and the item menu through the real dispatcher
//! (`test_support::TestApp`): where each key goes, what each page asks of its store, and what
//! the remote may not do while text is being entered, a menu is open or the player is up.

use sn_machine::machine::{Key, MachineId, NavOp};
use sn_stremio::search::RowState;
use sn_stremio::types::{CatalogPage, MetaPreview};

use crate::registry::{AppArg, AppFx, GridCmd, MenuKind, SearchCmd};
use crate::stores::catalog::{CatalogStore, Job, Landing};
use crate::stores::grid::GridLanding;
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

fn on_pill(app: &TestApp, tab: Tab) -> bool {
    focus_elem(app) == Some(tab.elem())
}

#[test]
fn up_from_the_first_row_reaches_the_bar_and_left_right_walk_the_tabs() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    assert!(focus_elem(&app).is_some_and(|e| !crate::tabs::on_strip(e)), "Home seats its first row");
    app.press(Key::Up);
    assert!(on_pill(&app, Tab::Home), "UP from the first row reaches the bar, on the page's own pill: {:?}", app.d.focus());
    app.press(Key::Right);
    assert!(on_pill(&app, Tab::LiveTv), "RIGHT walks to the next pill");
    app.press(Key::Right);
    app.press(Key::Left);
    assert!(on_pill(&app, Tab::LiveTv), "LEFT walks back");
    app.press(Key::Down);
    assert!(focus_elem(&app).is_some_and(|e| !crate::tabs::on_strip(e)), "DOWN from Home's pill enters the page");
    assert_eq!(app.top_name(), "home", "walking the pills selects nothing");
}

#[test]
fn ok_on_a_pill_opens_its_tab_and_back_returns_through_the_bar_to_home() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.press(Key::Up);
    for _ in 0..Tab::Settings.index() {
        app.press(Key::Right);
    }
    assert!(on_pill(&app, Tab::Settings));
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "settings", "OK on a pill selects its destination");
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
    assert_eq!(app.press_raw(0, WCODE_YELLOW), HotkeyRoute::Went(Tab::Settings));
    assert_eq!(app.top_name(), "settings", "YELLOW is Settings");
    app.press_raw(0, WCODE_GREEN);
    assert_eq!(app.top_name(), "search", "GREEN is the next pill (Settings → Search)");
    app.press_raw(0, WCODE_GREEN);
    assert_eq!(app.top_name(), "home", "and the walk wraps");
    app.press_raw(0, WCODE_RED);
    assert_eq!(app.top_name(), "search", "RED is the previous pill");
    app.press_raw(0, sn_ui::consts::WCODE_CH_UP_KEY);
    assert_eq!(app.top_name(), "settings", "CH+ is the previous pill too");
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
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::Detail(crate::registry::ItemRef { kind: "movie".into(), id: "tt1".into(), name: "X".into() })));
    app.frames(40);
    assert_eq!(app.top_name(), "detail");
    app.press_raw(0, WCODE_YELLOW);
    assert_eq!(app.top_name(), "settings");
}

#[test]
fn no_hotkey_while_text_is_entered_a_menu_is_open_or_the_player_is_up() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    // the player
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::Player("T".into())));
    app.frames(40);
    assert_eq!(app.top_name(), "player");
    assert_eq!(app.press_raw(0, WCODE_BLUE), HotkeyRoute::Player);
    assert_eq!(app.top_name(), "player", "the player keeps its keys");
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home");
    // a menu
    app.frames(10);
    app.hold_ok();
    assert_eq!(app.top_name(), "menu", "a held OK opens the card's menu");
    assert_eq!(app.press_raw(0, WCODE_YELLOW), HotkeyRoute::Menu);
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
    assert_eq!(app.press_raw(0, WCODE_YELLOW), HotkeyRoute::TextEntry);
    assert_eq!(app.top_name(), "search", "a colour key while typing is the keyboard's");
}

#[test]
fn hold_ok_opens_the_item_menu_and_add_to_library_waits_for_sign_in() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.hold_ok();
    assert_eq!(app.top_name(), "menu");
    let Some(AppArg::Menu(m)) = app.d.nav.modals.surfaces.last().map(|s| s.entry.arg.clone()) else {
        panic!("the menu is a surface over Home")
    };
    let MenuKind::Item(item) = &m.kind else { panic!("an item menu") };
    assert_eq!(item.id, "tt0-0-0");
    let menu = app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::menu::MenuScreen>()).expect("the menu screen");
    let rows: Vec<(String, bool)> = menu.table().sections[0].rows.iter().map(|r| (r.label.clone(), r.dim)).collect();
    assert_eq!(
        rows,
        vec![(sn_platform::i18n::msg::app_menu_open().into(), false), (sn_platform::i18n::msg::app_menu_add_to_library().into(), true)],
        "Open, and Add to library disabled while signed out"
    );
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "menu", "the disabled row does nothing");
    app.press(Key::Up);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail", "Open opens the item's page");
    assert_eq!(app.top_arg(), Some(&AppArg::Detail(item.clone())));
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
    let jobs = app.rig.search.submit("bat", &addons);
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

#[test]
fn the_addon_filter_is_a_chip_and_a_setting() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(true);
    app.press_raw(0, WCODE_BLUE);
    app.press(Key::Down);
    let settings = app.rig.search.settings_gen();
    app.press(Key::Ok);
    assert!(app.take_effects().iter().any(|fx| matches!(fx, AppFx::Search(SearchCmd::ToggleAddon(id)) if id == "com.linvo.cinemeta")));
    assert!(!app.rig.search.filter().allows("com.linvo.cinemeta"), "the first chip switched Cinemeta off");
    assert_ne!(app.rig.search.settings_gen(), settings, "and the filter is persisted with the settings");
    let addons = app.rig.catalog.addons().to_vec();
    let jobs = app.rig.search.submit("news", &addons);
    assert_eq!(jobs.len(), 1, "only the live addon is asked");
}

#[test]
fn live_tv_lists_the_channel_catalogs_and_the_genre_chip_picks_the_url() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(true);
    app.press_raw(0, WCODE_GREEN);
    assert_eq!(app.top_name(), "livetv");
    let open: Vec<_> = app
        .take_effects()
        .into_iter()
        .filter_map(|fx| match fx {
            AppFx::Grid(GridCmd::Open(k)) => Some(k),
            _ => None,
        })
        .collect();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].genre.as_deref(), Some("News & Talk"), "a required genre starts on its first option");
    assert_eq!(app.rig.grid.next_url(&open[0]).as_deref(), Some("https://a.example/live/catalog/tv/channels/genre=News%20%26%20Talk.json"));
    app.rig.grid.land(GridLanding { key: open[0].clone(), result: Ok(page("ch", 12)) });
    app.frames(4);
    // the genre chip: seated first (the only control), OK opens its choice menu
    if crate::tabs::on_strip(focus_elem(&app).unwrap_or(0)) {
        app.press(Key::Down);
    }
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "menu", "the genre chip opens a choice: {:?}", app.d.focus());
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "livetv", "a pick closes the menu");
    let effects = app.take_effects();
    assert!(effects.iter().any(|fx| matches!(fx, AppFx::Grid(GridCmd::Close(k)) if *k == open[0])), "the old genre's view is let go");
    let sport = effects
        .iter()
        .find_map(|fx| match fx {
            AppFx::Grid(GridCmd::Open(k)) => Some(k.clone()),
            _ => None,
        })
        .expect("the picked genre is opened");
    assert_eq!(app.rig.grid.next_url(&sport).as_deref(), Some("https://a.example/live/catalog/tv/channels/genre=Sport.json"));
    // a channel opens its streams page
    app.rig.grid.land(GridLanding { key: sport, result: Ok(page("ch", 5)) });
    app.frames(4);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail", "OK on a channel opens its page: {:?}", app.d.focus());
}

#[test]
fn an_install_without_channels_says_so_on_live_tv() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.press_raw(0, WCODE_GREEN);
    assert_eq!(app.top_name(), "livetv");
    let live = app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::livetv::LiveTvScreen>()).unwrap();
    assert!(live.catalogs().is_empty());
    assert!(live.key().is_none(), "nothing is asked");
    assert!(on_pill(&app, Tab::LiveTv), "the remote rests on the bar: {:?}", app.d.focus());
}

#[test]
fn a_subtitle_picker_sets_the_setting_and_returns() {
    let _g = sn_base::testlock::serial();
    let mut app = boot(false);
    app.press_raw(0, WCODE_YELLOW);
    assert_eq!(app.top_name(), "settings");
    if crate::tabs::on_strip(focus_elem(&app).unwrap_or(0)) {
        app.press(Key::Down);
    }
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "settings-page", "the subtitle size row opens its picker");
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "settings", "a pick returns to the root");
    use crate::stores::settings::Pref;
    assert_eq!(app.rig.settings.get(Pref::SubtitleSize), 2, "Medium → Large");
}
