//! The account's screens through the real dispatcher (`test_support::TestApp`): Continue Watching
//! resuming the episode into its stream request, the Resume page, the Addons pages (model, fit,
//! the search switch) and the Library.

use sn_machine::machine::Key;
use sn_stremio::manifest::{Addon, Manifest};
use sn_stremio::streams::GroupState;
use sn_ui::form::RowKey;
use sn_ui::route_screen::RouteLayout;
use sn_ui::table::TableView;

use crate::addons::{build_addon, build_list, Action, SEARCH};
use crate::registry::{AddonsCmd, AppArg, AppFx, DetailCmd, ItemRef, VideoRef, Views};
use crate::stores::account::{Account, AccountLanding, AccountStore, Secret, Stored};
use crate::stores::addons::{AddonsLanding, AddonsStore};
use crate::stores::catalog::CatalogStore;
use crate::stores::detail::{DetailJob, DetailLanding, DetailStore, StreamKey};
use crate::stores::library::{LibraryLanding, LibraryStore};
use crate::stores::StoreId;
use crate::test_support::TestApp;

fn addon(json: &str, transport: &str) -> Addon {
    Addon::new(transport, serde_json::from_str::<Manifest>(json).unwrap())
}

fn cinemeta() -> Addon {
    addon(
        r#"{"id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta","resources":["catalog","meta"],
            "types":["movie","series"],"idPrefixes":["tt"],
            "catalogs":[{"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]},
                        {"type":"series","id":"search","name":"Search","extra":[{"name":"search","isRequired":true}]}]}"#,
        "https://v3-cinemeta.strem.io/manifest.json",
    )
}

fn streamer() -> Addon {
    addon(
        r#"{"id":"org.example.streams","version":"1.2.0","name":"Streamer","resources":["stream"],"types":["movie","series"],
            "idPrefixes":["tt"],"catalogs":[],"behaviorHints":{"configurable":true}}"#,
        "https://streams.example/cfg-SECRET/manifest.json",
    )
}

fn account() -> AccountStore {
    let mut s = AccountStore::new();
    s.land(
        AccountLanding::Restored(Stored::Session(Account { user_id: "u1".into(), email: "viewer@example.com".into(), auth_key: Secret::new("k") })),
        0,
    );
    s
}

fn addons() -> AddonsStore {
    let mut s = AddonsStore::new();
    s.sign_in("u1", &Secret::new("k"));
    s.land(AddonsLanding::Fetched { user_id: "u1".into(), result: Ok((vec![cinemeta(), streamer()], Vec::new())) });
    s
}

fn library(items: &str) -> LibraryStore {
    let mut s = LibraryStore::new();
    s.sign_in("u1", &Secret::new("k"));
    s.land(LibraryLanding::Fetched { user_id: "u1".into(), result: Ok(serde_json::from_str(items).unwrap()) });
    s
}

const IN_PROGRESS: &str = r#"[
    {"_id":"tt0903747","type":"series","name":"Breaking Bad","poster":"https://img.example/bb.jpg",
     "state":{"timeOffset":1260000,"duration":2820000,"video_id":"tt0903747:2:5","lastWatched":"2026-10-01T00:00:00Z"}},
    {"_id":"tt0111161","type":"movie","name":"The Shawshank Redemption","state":{"timeOffset":600000,"duration":8520000,
     "lastWatched":"2026-09-01T00:00:00Z"}}]"#;

/// A signed-in app on Home over the account's addons and library.
fn signed_in_home(items: &str) -> TestApp {
    let mut app = TestApp::boot(CatalogStore::from_addons(vec![cinemeta(), streamer()]));
    app.rig.account = account();
    app.rig.addons = addons();
    app.rig.library = library(items);
    for id in [StoreId::Account, StoreId::Addons, StoreId::Library, StoreId::Catalog] {
        app.notify(id, 1);
    }
    app.frames(5);
    app
}

#[test]
fn continue_watching_resumes_the_episode_it_stopped_on_from_the_offset() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    assert!(app.d.focus().is_some(), "Continue Watching takes the first seat");
    app.take_effects();
    app.press(Key::Ok);
    let Some(AppArg::Streams(video)) = app.top_arg().cloned() else { panic!("OK resumes: {:?}", app.top_arg()) };
    assert_eq!(
        video,
        VideoRef {
            kind: "series".into(),
            video_id: "tt0903747:2:5".into(),
            show: "Breaking Bad".into(),
            title: sn_ui::fmt::episode_ordinal(2, 5),
            resume_ms: 1_260_000,
            item_id: "tt0903747".into(),
            from: "Home".into(),
        },
        "the EPISODE, never the bare series id"
    );
    let key = StreamKey { kind: "series".into(), video_id: "tt0903747:2:5".into() };
    let effects = app.take_effects();
    assert!(effects.contains(&AppFx::Detail(DetailCmd::OpenStreams(key.clone()))), "{effects:?}");
    // the stream request `streams::plan` makes for it: the episode's own stream resource
    let addons = app.rig.catalog.addons().to_vec();
    let jobs = app.rig.detail.open_streams(&addons, &Default::default(), key.clone());
    let urls: Vec<String> = jobs
        .iter()
        .filter_map(|j| match j {
            DetailJob::Streams { group, .. } => Some(group.url.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(urls, [sn_stremio::url::resource_url(&streamer().base, "stream", "series", "tt0903747:2:5", &[])]);
    assert!(!urls[0].ends_with("/stream/series/tt0903747.json"));
    // a stream lands; OK plays it from where the viewer stopped
    for job in jobs {
        if let DetailJob::Streams { key, index, .. } = job {
            let s: sn_stremio::types::StreamsResponse = serde_json::from_str(r#"{"streams":[{"url":"https://cdn.example/e.mkv","name":"1080p"}]}"#).unwrap();
            app.rig.detail.land(DetailLanding::Streams { key, index, state: GroupState::Ready(s.streams) });
        }
    }
    let gen = app.rig.detail.gen();
    app.notify(StoreId::Detail, gen);
    app.frames(5);
    app.press(Key::Ok);
    let played = app.take_effects().into_iter().find_map(|fx| match fx {
        AppFx::Play(req) => Some(req),
        _ => None,
    });
    assert_eq!(played.map(|r| (r.url, r.start_ms)), Some(("https://cdn.example/e.mkv".to_string(), 1_260_000)));
}

/// YELLOW on a Continue Watching card opens the Continue Watching page (every item in progress,
/// from the library store: nothing is fetched), whose OK resumes exactly as the shelf's card does
/// and whose held OK opens the item menu; BACK returns to the card. The shelf's See all tile and
/// LEFT on its first card open the same page.
#[test]
fn yellow_on_continue_watching_opens_its_page_which_resumes_and_keeps_the_hold_menu() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    let card = app.d.focus().map(|k| k.elem);
    app.take_effects();
    assert_eq!(app.press_raw(0, crate::tabs::WCODE_YELLOW), crate::tabs::HotkeyRoute::NotHotkey);
    assert_eq!(app.top_arg(), Some(&AppArg::Continue));
    app.frames(5);
    assert!(app.take_effects().iter().all(|fx| !matches!(fx, AppFx::Catalog(_) | AppFx::Grid(_))), "nothing is fetched");
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::continue_page::CARD_BASE), "the page seats its first card");
    app.hold_ok();
    assert!(
        matches!(crate::discover_tests::menu_of(&app), crate::registry::MenuKind::Item { item, .. } if item.id == "tt0903747"),
        "a held OK opens the item's menu"
    );
    app.press(Key::Back);
    app.press(Key::Right);
    app.press(Key::Ok);
    assert!(
        matches!(app.top_arg(), Some(AppArg::Streams(v)) if v.item_id == "tt0111161" && v.resume_ms == 600_000),
        "OK resumes from the offset, as the shelf's card does: {:?}",
        app.top_arg()
    );
    app.press(Key::Back);
    assert_eq!(app.top_name(), "continue");
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home");
    assert_eq!(app.d.focus().map(|k| k.elem), card, "BACK returns to the card");
    // LEFT on the first card, and the tile after the last (two items: RIGHT twice)
    app.press(Key::Left);
    assert_eq!(app.top_arg(), Some(&AppArg::Continue), "LEFT on the first card");
    app.press(Key::Back);
    app.press(Key::Right);
    app.press(Key::Right);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::home::CW_MORE_ELEM), "the See all tile ends the shelf");
    app.press(Key::Ok);
    assert_eq!(app.top_arg(), Some(&AppArg::Continue), "and opens the page");
}

/// In list mode Continue Watching lists like the catalog rows, each row with its resume bar and
/// the time left; OK on a row resumes. The Continue Watching page lists too (the same choice), and
/// its heading's View chip switches both back to posters.
#[test]
fn continue_watching_in_list_mode_shows_the_time_left_and_resumes() {
    use crate::stores::settings::ViewPage;
    let _g = sn_base::testlock::serial();
    let _l = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = signed_in_home(IN_PROGRESS);
    app.rig.settings.set_list(ViewPage::Home, true);
    app.notify(StoreId::Settings, app.rig.settings.gen());
    app.frames(5);
    let text = app.drawn_text();
    assert!(text.iter().any(|t| t == "26 min left"), "Breaking Bad's row says what is left: {text:?}");
    assert!(text.iter().any(|t| t == "2 hr 12 min left"), "{text:?}");
    let first = app.d.focus().map(|k| k.elem);
    app.press(Key::Down);
    assert!(app.d.focus().map(|k| k.elem) != first, "DOWN walks the list");
    app.press(Key::Ok);
    assert!(matches!(app.top_arg(), Some(AppArg::Streams(v)) if v.item_id == "tt0111161"), "{:?}", app.top_arg());
    app.press(Key::Back);
    app.press(Key::Down);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::home::CW_MORE_ELEM), "the list ends in its See all row");
    app.press(Key::Ok);
    app.frames(5);
    assert_eq!(app.top_name(), "continue");
    assert!(app.drawn_text().iter().any(|t| t == "26 min left"), "the page lists too");
    app.press(Key::Up);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::continue_page::TOGGLE_ELEM), "UP from the first row reaches the View chip");
    app.press(Key::Ok);
    assert!(!app.rig.settings.list(ViewPage::Home), "back to posters, Home's choice");
}

#[test]
fn a_series_with_no_recorded_episode_resumes_on_its_first_episode_once_the_meta_lands() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(r#"[{"_id":"tt1","type":"series","name":"Show","state":{"timeOffset":5}}]"#);
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(app.top_arg(), Some(&AppArg::Resume(ItemRef { kind: "series".into(), id: "tt1".into(), name: "Show".into(), from: "Home".into() })));
    let effects = app.take_effects();
    let meta = crate::stores::detail::MetaKey { kind: "series".into(), id: "tt1".into() };
    assert!(effects.contains(&AppFx::Detail(DetailCmd::OpenMeta(meta.clone()))));
    assert!(!effects.iter().any(|fx| matches!(fx, AppFx::Detail(DetailCmd::OpenStreams(_)))), "no stream is asked for the bare series id");
    let addons = app.rig.catalog.addons().to_vec();
    app.rig.detail = DetailStore::new();
    app.rig.detail.open_meta(&addons, meta.clone());
    let m: sn_stremio::types::Meta = serde_json::from_str(
        r#"{"id":"tt1","type":"series","name":"Show","videos":[
            {"id":"tt1:0:1","season":0,"episode":1},{"id":"tt1:1:2","season":1,"episode":2},{"id":"tt1:1:1","season":1,"episode":1}]}"#,
    )
    .unwrap();
    app.rig.detail.land(DetailLanding::Meta { key: meta, result: Ok(m) });
    let gen = app.rig.detail.gen();
    app.notify(StoreId::Detail, gen);
    app.frames(3);
    let effects = app.take_effects();
    assert!(
        effects.contains(&AppFx::Detail(DetailCmd::OpenStreams(StreamKey { kind: "series".into(), video_id: "tt1:1:1".into() }))),
        "the first episode of the first season, not the special: {effects:?}"
    );
    assert_eq!(app.top_name(), "streams");
}

#[test]
fn signed_out_home_has_no_continue_watching() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    app.rig.account = AccountStore::signed_out(true);
    app.notify(StoreId::Account, 9);
    app.frames(3);
    app.take_effects();
    app.press(Key::Ok);
    assert!(!matches!(app.top_arg(), Some(AppArg::Streams(_))), "nothing to resume signed out");
}

fn views<'a>(catalog: &'a CatalogStore, detail: &'a DetailStore, account: &'a AccountStore, addons: &'a AddonsStore, library: &'a LibraryStore) -> Views<'a> {
    // the stores these read-outs never consult, idle (leaked: a test's views outlive no store)
    Views {
        catalog,
        detail,
        playback: Box::leak(Box::new(crate::registry::PlaybackView::IDLE)),
        account,
        addons,
        library,
        search: Box::leak(Box::default()),
        grid: Box::leak(Box::new(crate::stores::grid::GridStore::new())),
        settings: Box::leak(Box::default()),
        session: crate::registry::SessionView { signed_in: account.account().is_some() },
    }
}

#[test]
fn the_addons_page_lists_the_collection_with_what_each_addon_serves() {
    let catalog = CatalogStore::from_addons(vec![cinemeta(), streamer()]);
    let (detail, account, addons, library) = (DetailStore::new(), account(), addons(), LibraryStore::new());
    let v = views(&catalog, &detail, &account, &addons, &library);
    let b = build_list(&v);
    let rows: Vec<(String, Option<String>, String)> =
        b.sections[1].rows.iter().map(|r| (r.label.clone(), r.value.clone(), r.detail.clone())).collect();
    let msg = |s: &str| s.to_string();
    assert_eq!(rows[0].0, "Cinemeta");
    assert_eq!(rows[0].2, format!("v3.0.13 \u{b7} {}, {} \u{b7} {}, {}",
        msg(sn_platform::i18n::msg::app_addons_res_catalog()), msg(sn_platform::i18n::msg::app_addons_res_meta()),
        msg(sn_platform::i18n::msg::app_kind_movie()), msg(sn_platform::i18n::msg::app_kind_series())));
    assert_eq!(rows[1].0, "Streamer");
    assert_eq!(rows[1].1.as_deref(), Some(sn_platform::i18n::msg::app_addons_configurable()));
    assert!(b.sections[0].rows.iter().all(|r| r.label != sn_platform::i18n::msg::app_addons_sign_out()), "signing out is the account menu's");
    assert_eq!(b.sections[0].rows[0].value.as_deref(), Some("viewer@example.com"));
    // signed out: a sign-in row, and the defaults
    let out = AccountStore::signed_out(true);
    let none = AddonsStore::new();
    let b = build_list(&views(&catalog, &detail, &out, &none, &library));
    assert_eq!(b.action_of(crate::addons::SIGN_IN), Some(&Action::SignIn));
}

#[test]
fn one_addon_shows_its_catalogs_and_the_shared_search_switch() {
    let catalog = CatalogStore::from_addons(vec![cinemeta(), streamer()]);
    let (detail, account, mut addons, library) = (DetailStore::new(), account(), addons(), LibraryStore::new());
    let (b, a) = build_addon(&views(&catalog, &detail, &account, &addons, &library), "com.linvo.cinemeta");
    assert!(a.is_some());
    let search = b.index_of(RowKey(SEARCH)).expect("a searchable addon has the switch");
    let toggle = |b: &crate::addons::Built| b.sections.iter().flat_map(|s| &s.rows).nth(b.index_of(RowKey(SEARCH)).unwrap()).and_then(|r| r.toggle);
    let _ = search;
    assert_eq!(toggle(&b), Some(true));
    let catalogs: Vec<(String, Option<String>)> = b.sections[2].rows.iter().map(|r| (r.label.clone(), r.value.clone())).collect();
    assert_eq!(
        catalogs,
        [
            ("Popular".to_string(), Some(sn_platform::i18n::msg::app_addons_on_home().to_string())),
            ("Search".to_string(), Some(sn_platform::i18n::msg::app_addons_search_only().to_string())),
        ]
    );
    addons.toggle_search("com.linvo.cinemeta");
    let (b, _) = build_addon(&views(&catalog, &detail, &account, &addons, &library), "com.linvo.cinemeta");
    assert_eq!(toggle(&b), Some(false), "the switch reads the AddonFilter the search fan-out reads");
    let (b, _) = build_addon(&views(&catalog, &detail, &account, &addons, &library), "org.example.streams");
    assert!(b.index_of(RowKey(SEARCH)).is_none(), "a stream-only addon has nothing to search");
    let (_, gone) = build_addon(&views(&catalog, &detail, &account, &addons, &library), "org.gone");
    assert!(gone.is_none());
}

#[test]
fn the_addons_pages_fit_in_every_shipped_language() {
    let catalog = CatalogStore::from_addons(vec![cinemeta(), streamer()]);
    let (detail, account, mut addons, library) = (DetailStore::new(), account(), addons(), LibraryStore::new());
    addons.land(AddonsLanding::Fetched {
        user_id: "u1".into(),
        result: Err(sn_stremio::account::ApiError::Fetch(sn_stremio::FetchError::Transport("timed out".into()))),
    });
    let w = RouteLayout::screen().sectioned_table().w;
    let mut fails = Vec::new();
    for lang in sn_platform::i18n::SHIPPED {
        let _l = sn_platform::i18n::language_on_this_thread_for_test(lang);
        let v = views(&catalog, &detail, &account, &addons, &library);
        for (tag, built) in [("list", build_list(&v)), ("addon", build_addon(&v, "com.linvo.cinemeta").0), ("streamer", build_addon(&v, "org.example.streams").0)] {
            let mut t = TableView::new();
            t.open_sections(built.sections);
            fails.extend(t.app_fit_failures(w, &format!("addons {tag} {lang:?}")));
        }
    }
    sn_ui::table::assert_no_fit_failures(&fails);
}

/// Settings > Addons: the row under the account section pushes the Addons page.
fn open_addons_from_settings(app: &mut TestApp) {
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Settings));
    app.frames(5);
    let addons = sn_ui::form::FormId::key(&crate::settings::RootRow::Addons).0;
    for _ in 0..4 {
        if app.d.focus().map(|k| k.elem) == Some(addons) {
            break;
        }
        app.press(Key::Down);
    }
    assert_eq!(app.d.focus().map(|k| k.elem), Some(addons), "Addons follows the account rows");
    app.press(Key::Ok);
    app.frames(5);
    assert_eq!(app.top_name(), "addons");
    assert_eq!(app.top_arg(), Some(&AppArg::Addons));
}

/// Addons is no pill: it is pushed from Settings (or the account menu), wears the bar on the
/// account pill, and BACK returns to its row.
#[test]
fn addons_opens_from_settings_and_back_returns_to_its_row() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    open_addons_from_settings(&mut app);
    assert!(crate::tabs::TABS.iter().all(|e| e.tab.arg() != AppArg::Addons), "no pill");
    assert_eq!(crate::tabs::Tab::of_arg(&AppArg::Addons), Some(crate::tabs::Tab::Settings), "the account pill's page");
    app.press(Key::Back);
    app.frames(5);
    assert_eq!(app.top_name(), "settings");
    let addons = sn_ui::form::FormId::key(&crate::settings::RootRow::Addons).0;
    assert_eq!(app.d.focus().map(|k| k.elem), Some(addons), "focus is back on the Addons row");
}

#[test]
fn ok_on_the_search_switch_toggles_it_and_ok_on_an_addon_opens_it() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    open_addons_from_settings(&mut app);
    // the account section: "Signed in as"; then the addons
    app.press(Key::Down);
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(app.top_arg(), Some(&AppArg::Addon("com.linvo.cinemeta".into())));
    app.frames(3);
    // About: Version, Serves, Configurable; then the switch
    for _ in 0..3 {
        app.press(Key::Down);
    }
    app.take_effects();
    app.press(Key::Ok);
    assert!(app.take_effects().contains(&AppFx::Addons(AddonsCmd::ToggleSearch("com.linvo.cinemeta".into()))));
}

#[test]
fn the_library_shelves_open_their_items_and_signed_out_offers_the_sign_in() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Library));
    app.frames(5);
    assert_eq!(app.top_name(), "library");
    assert!(app.take_effects().contains(&AppFx::Library(crate::registry::LibraryCmd::Reload)), "the page refreshes the library");
    app.press(Key::Ok);
    assert_eq!(
        app.top_arg(),
        Some(&AppArg::Detail(ItemRef { kind: "movie".into(), id: "tt0111161".into(), name: "The Shawshank Redemption".into(), from: "Library".into() })),
        "movies first, and a library card navigates"
    );
    app.press(Key::Back);
    app.rig.account = AccountStore::signed_out(true);
    app.rig.library.sign_out();
    app.notify(StoreId::Account, 9);
    app.notify(StoreId::Library, 9);
    app.frames(40);
    app.press(Key::Ok);
    assert_eq!(app.top_arg(), Some(&AppArg::SignIn), "signed out, the read-out's action signs in");
}

#[test]
fn settings_shows_the_account_and_follows_a_sign_out() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Settings));
    app.frames(5);
    assert_eq!(app.top_name(), "settings");
    let settings = app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::settings::SettingsScreen>()).unwrap();
    let account_rows = &settings.table().expect("a table page").sections[0].rows;
    assert_eq!(account_rows[0].value.as_deref(), Some("viewer@example.com"), "who is signed in");
    assert!(account_rows.iter().all(|r| r.label != sn_platform::i18n::msg::app_addons_sign_out()), "the account menu owns Sign out");
    if app.d.focus().is_some_and(|k| crate::tabs::on_strip(k.elem)) {
        app.press(Key::Down);
    }
    let key = |r: crate::settings::RootRow| sn_ui::form::FormId::key(&r).0;
    assert_eq!(app.d.focus().map(|k| k.elem), Some(key(crate::settings::RootRow::Account)), "the account row is the first stop");
    app.take_effects();
    app.press(Key::Ok);
    app.frames(3);
    let menu = app.d.nav.modals.surfaces.last().map(|s| s.entry.arg.clone());
    assert!(
        matches!(menu, Some(AppArg::Menu(ref m)) if m.kind == crate::registry::MenuKind::Account),
        "OK on who is signed in opens the account menu (Sign out lives there): {menu:?}"
    );
    app.press(Key::Back);
    app.frames(5);
    assert_eq!(app.top_name(), "settings");
    assert_eq!(app.d.focus().map(|k| k.elem), Some(key(crate::settings::RootRow::Account)), "closing it returns to the row");
    app.press(Key::Down);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(key(crate::settings::RootRow::Addons)), "no Sign out row: the account menu owns it");
    app.rig.account = AccountStore::signed_out(true);
    app.notify(StoreId::Account, 9);
    app.frames(3);
    let settings = app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::settings::SettingsScreen>()).unwrap();
    assert_eq!(settings.table().unwrap().sections[0].rows[0].label, sn_platform::i18n::msg::app_addons_sign_in(), "signed out, it offers the sign-in");
    let sign_in = sn_ui::form::FormId::key(&crate::settings::RootRow::SignIn).0;
    app.press(Key::Up);
    app.press(Key::Down);
    if app.d.focus().map(|k| k.elem) != Some(sign_in) {
        app.press(Key::Up);
    }
    app.press(Key::Ok);
    assert_eq!(app.top_arg(), Some(&AppArg::SignIn));
}

/// The Library's type dropdown: UP from the first shelf reaches it, a pick of Series from its
/// popover keeps only the series shelf, and a type with nothing in the library shows the empty
/// read-out with the dropdown still there to choose again.
#[test]
fn the_library_type_dropdown_narrows_the_shelves() {
    use crate::discover_tests::{menu_of, pick};
    use crate::registry::{ChoiceTarget, MenuKind};
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Library));
    app.frames(5);
    app.press(Key::Up);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::library::TYPE_ELEM), "UP from the first shelf reaches the dropdown");
    app.press(Key::Right);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::library::SORT_ELEM), "then the sort dropdown");
    app.press(Key::Right);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::library::TOGGLE_ELEM), "then the View toggle");
    app.press(Key::Left);
    app.press(Key::Left);
    app.press(Key::Ok);
    let MenuKind::Choice { options, target, .. } = menu_of(&app) else { panic!("a choice") };
    assert_eq!((options.len(), target), (3, ChoiceTarget::LibraryType), "All, Movies, Series");
    pick(&mut app, 2);
    app.frames(3);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert!(matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id == "tt0903747"), "only the series shelf: {:?}", app.top_arg());
    app.press(Key::Back);
    app.frames(3);

    // a library of one movie: the options are the LIBRARY's types (not every addon catalog's),
    // so the remembered Series is no choice at all and the movie shows
    let mut app = signed_in_home(
        r#"[{"_id":"tt0111161","type":"movie","name":"The Shawshank Redemption","state":{"lastWatched":"2026-09-01T00:00:00Z"}}]"#,
    );
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Library));
    app.frames(5);
    let lib = app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::library::LibraryScreen>()).unwrap();
    assert_eq!(lib.type_options(), 0, "one type in the library: nothing to choose");
    app.press(Key::Ok);
    assert!(matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id == "tt0111161"), "the movie shows: {:?}", app.top_arg());
    crate::type_filter::remember(crate::type_filter::Page::Library, crate::type_filter::ALL);
}

/// An empty library says how to fill it.
#[test]
fn an_empty_library_says_how_to_add_a_title() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home("[]");
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Library));
    app.frames(5);
    let text = app.drawn_text().join(" ");
    assert!(text.contains(sn_platform::i18n::msg::app_library_empty()), "{text}");
    assert!(text.contains(sn_platform::i18n::msg::app_library_empty_hint()), "{text}");
}

#[test]
fn the_library_as_a_list_opens_its_rows_and_the_view_chip_switches_back() {
    use crate::stores::settings::ViewPage;
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    app.rig.settings.set_list(ViewPage::Library, true);
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Library));
    app.frames(5);
    let first = app.d.focus().map(|k| k.elem);
    app.press(Key::Down);
    assert_ne!(app.d.focus().map(|k| k.elem), first, "DOWN walks from the movies' row to the series' row");
    app.press(Key::Ok);
    assert!(matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id == "tt0903747"), "OK opens the row's item: {:?}", app.top_arg());
    app.press(Key::Back);
    app.frames(3);
    app.press(Key::Up);
    app.press(Key::Up);
    for _ in 0..4 {
        app.press(Key::Right);
    }
    app.take_effects();
    app.press(Key::Ok);
    assert!(!app.rig.settings.list(ViewPage::Library), "the View toggle at the end of the row switches back to posters");
}

/// The Library's type dropdown stays on the page while its popover is up and after a pick.
#[test]
fn the_dropdown_button_stays_drawn_while_open_and_after_a_pick() {
    let _g = sn_base::testlock::serial();
    let _l = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = signed_in_home(IN_PROGRESS);
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Library));
    app.frames(5);
    app.press(Key::Up);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::library::TYPE_ELEM));
    crate::nav_tests::the_dropdown_button_stays_drawn(&mut app, "All types", 2, "Series");
    crate::nav_tests::the_dropdown_button_stays_drawn(&mut app, "Series", 0, "All types");
}

/// The install list failing under a Continue Watching row is a band BELOW the row, not a
/// full-page read-out over it, and its *Try again* is reachable: DOWN from the row, OK.
#[test]
fn a_failed_addons_band_under_continue_watching_is_reachable() {
    use crate::stores::catalog::{AddonsState, Job, Landing};
    let _g = sn_base::testlock::serial();
    let mut store = CatalogStore::new(["https://one.example/manifest.json"]);
    for job in store.take_jobs() {
        if let Job::Manifest { source, .. } = job {
            store.land(Landing::Manifest { source, result: Err(sn_stremio::FetchError::Transport("down".into())) });
        }
    }
    assert_eq!(store.addons_state(), AddonsState::Failed);
    let mut app = TestApp::boot(store);
    app.rig.account = account();
    app.rig.library = library(IN_PROGRESS);
    for id in [StoreId::Account, StoreId::Library, StoreId::Catalog] {
        app.notify(id, 1);
    }
    app.frames(5);
    let verdict = sn_platform::i18n::msg::app_home_addons_failed();
    assert_eq!(app.drawn_text().iter().filter(|t| t.contains(verdict)).count(), 1, "the failure is said once");
    let cw = app.d.focus().map(|k| k.elem);
    assert!(cw.is_some_and(|e| e >= 0x4000_0000), "Continue Watching takes the first seat: {cw:?}");
    app.press(Key::Down);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::home::BAND_RETRY_ELEM), "DOWN reaches the band's Try again");
    app.take_effects();
    app.press(Key::Ok);
    let asks = app.take_effects().into_iter().filter(|fx| *fx == AppFx::Catalog(crate::registry::CatalogCmd::RetryAddons)).count();
    assert_eq!(asks, 1, "one press, one retry");
    app.press(Key::Up);
    let up = app.d.focus().map(|k| k.elem);
    assert!(up == cw || up == Some(crate::home::CW_MORE_ELEM), "UP returns to the row: {up:?}");
}

/// A Detail page's crumb names the page BACK returns to — the Library here, not always "Home".
#[test]
fn a_detail_page_opened_from_the_library_names_the_library_in_its_crumb() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Library));
    app.frames(5);
    app.press(Key::Ok);
    app.frames(3);
    assert_eq!(app.top_name(), "detail");
    let library = crate::tabs::Tab::Library.label();
    let text = app.drawn_text();
    assert!(text.iter().any(|t| t == library), "the crumb says {library}: {text:?}");
    assert!(!text.iter().any(|t| t == sn_platform::i18n::msg::app_nav_home()), "not Home: {text:?}");
}

/// A Home catalog card of a title the account has watched wears the watched tick, as the
/// Library's, Discover's and Search's cards do.
#[test]
fn a_home_catalog_card_wears_the_watched_tick() {
    use crate::stores::catalog::{Job, Landing};
    use sn_ui::cards::CardSource;
    let _g = sn_base::testlock::serial();
    let lib = library(r#"[{"_id":"tt7","type":"movie","name":"Seen","state":{"timesWatched":1,"lastWatched":"2026-09-01T00:00:00Z"}}]"#);
    let mut store = CatalogStore::new(["https://one.example/manifest.json"]);
    let _ = store.take_jobs();
    store.land(Landing::Manifest {
        source: 0,
        result: Ok(br#"{"id":"x","name":"X","resources":["catalog"],"types":["movie"],"catalogs":[{"type":"movie","id":"top","name":"Top"}]}"#.to_vec()),
    });
    store.set_view(0..1);
    for job in store.take_jobs() {
        if let Job::Page { row, .. } = job {
            let page = serde_json::from_str(r#"{"metas":[{"id":"tt7","type":"movie","name":"Seen"},{"id":"tt8","type":"movie","name":"New"}]}"#).unwrap();
            store.land(Landing::Page { row, result: Ok(page) });
        }
    }
    let row = &store.rows()[0];
    let at = std::collections::HashMap::new();
    let cards = crate::home::RowCards { row, elems: &[1, 2], at: &at, watched: Some(&lib) };
    let ticked = |i: usize| matches!(CardSource::<crate::registry::Bundle>::art(&cards, i), sn_ui::widgets::Art::Poster(Some(f)) if f.watched);
    assert!(ticked(0), "the watched title is ticked");
    assert!(!ticked(1));
}
