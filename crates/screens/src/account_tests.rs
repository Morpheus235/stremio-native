//! The account's screens through the real dispatcher (`test_support::TestApp`): Continue Watching
//! resuming the episode into its stream request, the Resume page, the Addons pages (model, fit,
//! the search switch) and the Library.

use sn_machine::machine::Key;
use sn_stremio::manifest::{Addon, Manifest};
use sn_stremio::streams::GroupState;
use sn_ui::form::RowKey;
use sn_ui::route_screen::RouteLayout;
use sn_ui::table::TableView;

use crate::addons::{build_addon, build_list, Action, SEARCH, SIGN_OUT};
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
            title: sn_platform::i18n::msg::core_episode_ordinal(2, 5),
            resume_ms: 1_260_000,
        },
        "the EPISODE, never the bare series id"
    );
    let key = StreamKey { kind: "series".into(), video_id: "tt0903747:2:5".into() };
    let effects = app.take_effects();
    assert!(effects.contains(&AppFx::Detail(DetailCmd::OpenStreams(key.clone()))), "{effects:?}");
    // the stream request `streams::plan` makes for it: the episode's own stream resource
    let addons = app.rig.catalog.addons().to_vec();
    let jobs = app.rig.detail.open_streams(&addons, key.clone());
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

#[test]
fn a_series_with_no_recorded_episode_resumes_on_its_first_episode_once_the_meta_lands() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(r#"[{"_id":"tt1","type":"series","name":"Show","state":{"timeOffset":5}}]"#);
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(app.top_arg(), Some(&AppArg::Resume(ItemRef { kind: "series".into(), id: "tt1".into(), name: "Show".into() })));
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
        playback: &crate::registry::PlaybackView::IDLE,
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
    assert_eq!(b.action_of(SIGN_OUT), Some(&Action::SignOut));
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

#[test]
fn ok_on_the_search_switch_toggles_it_and_ok_on_an_addon_opens_it() {
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Addons));
    app.frames(5);
    assert_eq!(app.top_name(), "addons");
    // the account section: "Signed in as", Sign out; then the addons
    app.press(Key::Down);
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
        Some(&AppArg::Detail(ItemRef { kind: "movie".into(), id: "tt0111161".into(), name: "The Shawshank Redemption".into() })),
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
fn settings_shows_the_account_and_signs_out_through_the_account_store() {
    use crate::stores::account::AccountCmd;
    let _g = sn_base::testlock::serial();
    let mut app = signed_in_home(IN_PROGRESS);
    app.d.request(sn_machine::machine::MachineId::Nav, sn_machine::machine::NavOp::SelectTab(AppArg::Settings));
    app.frames(5);
    assert_eq!(app.top_name(), "settings");
    let settings = app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::settings::SettingsScreen>()).unwrap();
    let account_rows = &settings.table().expect("a table page").sections[0].rows;
    assert_eq!(account_rows[0].value.as_deref(), Some("viewer@example.com"), "who is signed in");
    if app.d.focus().is_some_and(|k| crate::tabs::on_strip(k.elem)) {
        app.press(Key::Down);
    }
    let key = |r: crate::settings::RootRow| sn_ui::form::FormId::key(&r).0;
    assert_eq!(app.d.focus().map(|k| k.elem), Some(key(crate::settings::RootRow::Account)), "the account row is the first stop");
    app.take_effects();
    app.press(Key::Ok);
    assert!(app.take_effects().is_empty(), "OK on who is signed in does nothing");
    assert_eq!(app.top_name(), "settings");
    app.press(Key::Down);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(key(crate::settings::RootRow::SignOut)));
    app.take_effects();
    app.press(Key::Ok);
    assert!(app.take_effects().contains(&AppFx::Account(AccountCmd::SignOut)), "Sign out is the account store's");
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
