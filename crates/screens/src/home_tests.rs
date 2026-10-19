//! Home through the real dispatcher (`test_support::TestApp`): the rows it shows, what it asks the
//! catalog store for, and where OK takes it.

use sn_machine::machine::Key;
use sn_stremio::types::{CatalogPage, MetaPreview};

use crate::registry::{AppArg, AppFx, CatalogCmd, ItemRef};
use crate::stores::catalog::{CatalogStore, Job, Landing, RowId};
use crate::stores::StoreId;
use crate::test_support::TestApp;

const CINEMETA: &str = r#"{
    "id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta",
    "resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"],
    "catalogs":[
        {"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]},
        {"type":"series","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]},
        {"type":"movie","id":"imdbRating","name":"Featured","extra":[{"name":"skip"}]},
        {"type":"series","id":"imdbRating","name":"Featured","extra":[{"name":"skip"}]},
        {"type":"movie","id":"year","name":"New","extra":[{"name":"skip"}]},
        {"type":"series","id":"year","name":"New","extra":[{"name":"skip"}]}
    ]}"#;

fn page(prefix: &str, n: usize) -> CatalogPage {
    CatalogPage {
        metas: (0..n)
            .map(|i| MetaPreview {
                id: format!("{prefix}{i}"),
                kind: "movie".into(),
                name: format!("Title {i}"),
                poster: Some(format!("https://images.example/{prefix}{i}.jpg")),
                ..Default::default()
            })
            .collect(),
    }
}

/// A signed-out store whose Cinemeta manifest has landed; the page jobs it then wants are returned.
fn cinemeta() -> (CatalogStore, Vec<Job>) {
    let mut s = CatalogStore::signed_out();
    let jobs = s.take_jobs();
    assert!(matches!(&jobs[..], [Job::Manifest { source: 0, .. }]));
    assert!(s.land(Landing::Manifest { source: 0, result: Ok(CINEMETA.as_bytes().to_vec()) }));
    let jobs = s.take_jobs();
    (s, jobs)
}

fn row(catalog: u16) -> RowId {
    RowId { source: 0, catalog }
}

fn views(effects: &[AppFx]) -> Vec<std::ops::Range<usize>> {
    effects
        .iter()
        .filter_map(|fx| match fx {
            AppFx::Catalog(CatalogCmd::View(v)) => Some(v.rows.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn home_shows_a_shelf_per_catalog_and_reports_the_rows_on_screen() {
    let _g = sn_base::testlock::serial();
    let (store, _) = cinemeta();
    let mut app = TestApp::boot(store);
    app.frames(2);
    assert_eq!(app.top_name(), "home");
    let reported = views(&app.take_effects());
    let view = reported.last().cloned().expect("Home reports the rows it shows");
    assert_eq!(view.start, 0, "the page opens at its first row");
    assert!(view.end >= 2 && view.end < 6, "a screenful of rows, not every row ({view:?})");
    assert_eq!(
        crate::home::heading_of(&app.rig.catalog, row(1)),
        Some(("Popular".to_string(), sn_platform::i18n::msg::app_kind_series().to_string())),
        "a heading names the catalog and its type"
    );
}

#[test]
fn landed_cards_take_focus_and_ok_opens_the_items_detail() {
    let _g = sn_base::testlock::serial();
    let (mut store, jobs) = cinemeta();
    for job in jobs {
        if let Job::Page { row, .. } = job {
            store.land(Landing::Page { row, result: Ok(page(&format!("tt{}-", row.catalog), 20)) });
        }
    }
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail", "OK on the seated card opens its page");
    assert_eq!(
        app.top_arg(),
        Some(&AppArg::Detail(ItemRef { kind: "movie".into(), id: "tt0-0".into(), name: "Title 0".into(), from: "Home".into() }))
    );
    // the detail page opens what it reads: the meta and (a movie) the streams
    let effects = app.take_effects();
    assert!(effects.iter().any(|fx| matches!(fx, AppFx::Detail(crate::registry::DetailCmd::OpenMeta(_)))));
    assert!(effects.iter().any(|fx| matches!(fx, AppFx::Detail(crate::registry::DetailCmd::OpenStreams(_)))));
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home", "BACK returns to Home");
}

#[test]
fn moving_down_the_rows_moves_the_reported_view() {
    let _g = sn_base::testlock::serial();
    let (mut store, first) = cinemeta();
    // every row has cards, so every shelf is a focus stop
    store.set_view(0..6);
    // the store keeps a bounded number of page loads in flight: land rounds until every row has
    // its first page
    let mut jobs = first;
    for _ in 0..6 {
        for job in std::mem::take(&mut jobs) {
            if let Job::Page { row, .. } = job {
                let n = store.row(row).map_or(0, |r| r.items().len());
                store.land(Landing::Page { row, result: Ok(page(&format!("tt{}-{n}-", row.catalog), 20)) });
            }
        }
        jobs = store.take_jobs();
    }
    assert!(store.rows().iter().all(|r| !r.items().is_empty()), "every row landed a page");
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app.take_effects();
    for _ in 0..5 {
        app.press(Key::Down);
    }
    app.frames(60);
    let reported = views(&app.take_effects());
    let last = reported.last().cloned().expect("the view moved with focus");
    assert!(last.end == 6 && last.start > 0, "the last rows are in view and the first scrolled away ({last:?})");
}

/// A Home row is its catalog's first page: walking to its end reaches See all and asks for no
/// next page (one landing under the cards on screen swapped them for others, page after page).
#[test]
fn a_shelf_reaching_its_end_asks_for_no_next_page() {
    let _g = sn_base::testlock::serial();
    let (mut store, jobs) = cinemeta();
    for job in jobs {
        if let Job::Page { row, .. } = job {
            store.land(Landing::Page { row, result: Ok(page(&format!("tt{}-", row.catalog), 8)) });
        }
    }
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(2);
    for _ in 0..8 {
        app.press(Key::Right);
    }
    app.frames(30);
    assert_eq!(focus(&app), Some(crate::home::more_elem(row(0))), "the row ends in its See all tile");
    let pages: Vec<_> = app.rig.catalog.take_jobs().into_iter().filter(|j| matches!(j, Job::Page { row: r, .. } if *r == row(0))).collect();
    assert!(pages.is_empty(), "no next page is asked: {pages:?}");
}

#[test]
fn back_at_home_is_the_applications_call() {
    let _g = sn_base::testlock::serial();
    let (store, _) = cinemeta();
    let mut app = TestApp::boot(store);
    app.frames(2);
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home");
    assert_eq!(app.rig.back_at_root, 1, "BACK at the root is handed to the application (the TV's Home)");
}

/// Home mounts before anything has loaded (the install list is still on its way), so its first
/// Enter has nothing to seat. The first row that lands must take focus, or OK does nothing on a
/// screen full of posters.
#[test]
fn a_home_that_mounted_empty_seats_focus_when_its_first_row_lands() {
    let _g = sn_base::testlock::serial();
    let mut app = TestApp::boot(CatalogStore::signed_out());
    app.frames(3);
    assert_eq!(
        app.d.focus().map(|k| k.elem),
        Some(crate::tabs::Tab::Home.elem()),
        "while the manifests load the remote rests on the bar's Home pill"
    );
    let jobs = app.rig.catalog.take_jobs();
    assert!(matches!(&jobs[..], [Job::Manifest { source: 0, .. }]));
    app.rig.catalog.land(Landing::Manifest { source: 0, result: Ok(CINEMETA.as_bytes().to_vec()) });
    for job in app.rig.catalog.take_jobs() {
        if let Job::Page { row, .. } = job {
            app.rig.catalog.land(Landing::Page { row, result: Ok(page(&format!("tt{}-", row.catalog), 20)) });
        }
    }
    let gen = app.rig.catalog.gen();
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    assert!(
        app.d.focus().is_some_and(|k| !crate::tabs::on_strip(k.elem)),
        "the first landed row takes the seat nobody chose: {:?}",
        app.d.focus()
    );
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail", "and OK opens its card");
}

const STREAMER: &str = r#"{
    "id":"org.example.streams","version":"1.0.0","name":"Streamer",
    "resources":["stream"],"types":["movie"],"idPrefixes":["tt"],"catalogs":[]}"#;

/// The same for a Detail page: its stream rows land after it mounted on a table of read-outs.
#[test]
fn a_detail_page_seats_its_first_stream_when_it_lands() {
    use crate::stores::detail::{DetailJob, DetailLanding, StreamKey};
    use sn_stremio::streams::GroupState;
    let _g = sn_base::testlock::serial();
    let mut store = CatalogStore::new(["https://a.example/cinemeta/manifest.json", "https://a.example/streamer/manifest.json"]);
    let jobs = store.take_jobs();
    assert_eq!(jobs.len(), 2);
    store.land(Landing::Manifest { source: 0, result: Ok(CINEMETA.as_bytes().to_vec()) });
    store.land(Landing::Manifest { source: 1, result: Ok(STREAMER.as_bytes().to_vec()) });
    for job in store.take_jobs() {
        if let Job::Page { row, .. } = job {
            store.land(Landing::Page { row, result: Ok(page(&format!("tt{}-", row.catalog), 20)) });
        }
    }
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail");
    // the page asked for its streams; the store plans one group per stream addon, loading
    let key = StreamKey { kind: "movie".into(), video_id: "tt0-0".into() };
    let addons = app.rig.catalog.addons().to_vec();
    let jobs = app.rig.detail.open_streams(&addons, &Default::default(), key.clone());
    let gen = app.rig.detail.gen();
    app.notify(StoreId::Detail, gen);
    app.frames(3);
    let before = app.d.focus();
    for job in jobs {
        if let DetailJob::Streams { key, index, .. } = job {
            let streams: sn_stremio::types::StreamsResponse =
                serde_json::from_str(r#"{"streams":[{"url":"https://cdn.example/a.mp4","name":"1080p"}]}"#).unwrap();
            app.rig.detail.land(DetailLanding::Streams { key, index, state: GroupState::Ready(streams.streams) });
        }
    }
    let gen = app.rig.detail.gen();
    app.notify(StoreId::Detail, gen);
    app.frames(3);
    let after = app.d.focus();
    assert!(after.is_some() && after != before, "the landed stream row takes focus ({before:?} -> {after:?})");
    app.take_effects();
    app.press(Key::Ok);
    let effects = app.take_effects();
    assert!(effects.iter().any(|fx| matches!(fx, AppFx::Play(_))), "OK on it plays: {effects:?}");
}

/// Every row of `cinemeta()` landed its first page.
fn every_row_landed() -> CatalogStore {
    let (mut store, first) = cinemeta();
    store.set_view(0..6);
    let mut jobs = first;
    for _ in 0..6 {
        for job in std::mem::take(&mut jobs) {
            if let Job::Page { row, .. } = job {
                store.land(Landing::Page { row, result: Ok(page(&format!("tt{}-", row.catalog), 20)) });
            }
        }
        jobs = store.take_jobs();
    }
    store
}

fn windows(effects: &[AppFx]) -> Vec<crate::stores::catalog::RowWindow> {
    effects
        .iter()
        .filter_map(|fx| match fx {
            AppFx::Catalog(CatalogCmd::View(v)) => Some(v.clone()),
            _ => None,
        })
        .collect()
}

/// The type dropdown: UP from the first row reaches it, OK opens its popover (All checked), a pick
/// narrows the rows to Series (DOWN then lands on a series row's card), and the window Home
/// reports counts only the series rows — nothing hidden is asked for. The choice survives a
/// remount.
#[test]
fn the_type_dropdown_narrows_home_to_one_type_from_the_remote() {
    use crate::discover_tests::{menu_of, pick};
    use crate::registry::{ChoiceTarget, MenuKind};
    let _g = sn_base::testlock::serial();
    let _l = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let store = every_row_landed();
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app.press(Key::Up);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::home::TYPE_ELEM), "UP from the first row reaches the dropdown");
    assert!(app.drawn_text().iter().any(|t| t == "All types"), "it says what is shown");
    app.press(Key::Right);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::home::TOGGLE_ELEM), "RIGHT reaches the View toggle at the row's end");
    app.press(Key::Left);
    app.take_effects();
    app.press(Key::Ok);
    let MenuKind::Choice { options, current, target, .. } = menu_of(&app) else { panic!("a choice") };
    assert_eq!((options, current, target), (vec!["All".to_string(), "Movies".into(), "Series".into()], 0, ChoiceTarget::HomeType));
    pick(&mut app, 2);
    app.frames(3);
    assert_eq!(app.top_name(), "home");
    let w = windows(&app.take_effects());
    let last = w.last().expect("the new window is reported");
    assert_eq!(last.kind.as_deref(), Some("series"));
    assert!(last.rows.end <= 3, "positions among the three series rows: {last:?}");
    assert!(!app.rig.catalog.take_jobs().iter().any(|j| matches!(j, Job::Page { .. })), "nothing is fetched");
    app.press(Key::Down);
    app.press(Key::Ok);
    assert!(
        matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id.starts_with("tt1-")),
        "DOWN from the dropdown lands on the first SERIES row (catalog 1): {:?}",
        app.top_arg()
    );
    assert_eq!(crate::type_filter::remembered(crate::type_filter::Page::Home), "series", "kept for the session");
    // All again: today's order, the first row movies
    app.press(Key::Back);
    app.frames(3);
    app.press(Key::Up);
    app.press(Key::Ok);
    assert!(matches!(menu_of(&app), MenuKind::Choice { current: 2, .. }), "Series, checked");
    pick(&mut app, 0);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert!(matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id == "tt0-0"), "All shows the movie row first: {:?}", app.top_arg());
}

/// A pointer click on the dropdown and on an option goes through the same hit map as the remote.
#[test]
fn a_click_on_the_type_dropdown_narrows_home() {
    let _g = sn_base::testlock::serial();
    let store = every_row_landed();
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app.take_effects();
    crate::discover_tests::click(&mut app, crate::home::TYPE_ELEM);
    assert_eq!(app.top_name(), "menu");
    crate::discover_tests::click(&mut app, 0x10 + 1);
    let w = windows(&app.take_effects());
    assert_eq!(w.last().and_then(|w| w.kind.as_deref()), Some("movie"), "the click chose Movies: {w:?}");
}

/// The dropdown scrolled back into view from below stops under the top bar, where a shelf would:
/// a plain section stopped at the screen's edge, so the tab bar's pills sat over it.
#[test]
fn the_dropdown_revealed_from_below_sits_under_the_top_bar() {
    let _g = sn_base::testlock::serial();
    let store = every_row_landed();
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    let card = app.d.focus().map(|k| k.elem);
    for _ in 0..4 {
        app.press(Key::Down);
    }
    app.frames(30);
    for _ in 0..4 {
        app.press(Key::Up);
    }
    assert_eq!(app.d.focus().map(|k| k.elem), card, "back on the first row");
    app.press(Key::Up);
    app.frames(30);
    let r = app.d.last_stops().iter().rev().find(|s| s.key.elem == crate::home::TYPE_ELEM).map(|s| s.rect).expect("drawn");
    assert!(r.y >= sn_ui::widgets::TOP_BAR_BOTTOM && (r.y - crate::home::CONTENT_TOP).abs() < 1.0, "{r:?}");
    app.send(sn_machine::machine::InputKind::Click { x: r.cx(), y: r.cy(), hit: None });
    app.frames(3);
    assert_eq!(app.top_name(), "menu", "the button is where the bar leaves it");
}

/// The type dropdown stays on the page while its popover is up and after a pick of a type whose
/// rows have not landed yet: it stood only while a row OF THE CHOSEN TYPE held cards, so picking
/// Series on a Home that had loaded only movies took the button off the page until a series row
/// landed (or for good, when none did).
#[test]
fn the_dropdown_button_stays_drawn_while_open_and_after_a_pick() {
    let _g = sn_base::testlock::serial();
    let _l = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let (mut store, jobs) = cinemeta();
    for job in jobs {
        if let Job::Page { row, .. } = job {
            if row.catalog == 0 {
                store.land(Landing::Page { row, result: Ok(page("tt0-", 20)) });
            }
        }
    }
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app.press(Key::Up);
    assert_eq!(app.d.focus().map(|k| k.elem), Some(crate::home::TYPE_ELEM));
    crate::nav_tests::the_dropdown_button_stays_drawn(&mut app, "All types", 2, "Series");
    crate::type_filter::remember(crate::type_filter::Page::Home, crate::type_filter::ALL);
}

fn focus(app: &TestApp) -> Option<u32> {
    app.d.focus().map(|k| k.elem)
}

fn landed_home() -> TestApp {
    let store = every_row_landed();
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app
}

/// One addon at a fixture address (never a real addon's), the Cinemeta-shaped manifest landed,
/// every row's first page `n` cards long (and more to come: every catalog pages by `skip`).
fn one_addon_home(n: usize) -> TestApp {
    let mut store = CatalogStore::new(["https://one.example/manifest.json"]);
    assert!(matches!(&store.take_jobs()[..], [Job::Manifest { source: 0, .. }]));
    store.land(Landing::Manifest { source: 0, result: Ok(CINEMETA.as_bytes().to_vec()) });
    store.set_view(0..6);
    let mut jobs = store.take_jobs();
    for _ in 0..6 {
        for job in std::mem::take(&mut jobs) {
            if let Job::Page { row, .. } = job {
                store.land(Landing::Page { row, result: Ok(page(&format!("tt{}-", row.catalog), n)) });
            }
        }
        jobs = store.take_jobs();
    }
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app
}

/// The catalog page on top: `(type, catalog id)`.
fn catalog_on_top(app: &TestApp) -> Option<(String, String)> {
    match app.top_arg() {
        Some(AppArg::Catalog(r)) => Some((r.key.kind.clone(), r.key.catalog_id.clone())),
        _ => None,
    }
}

fn movies() -> Option<(String, String)> {
    Some(("movie".into(), "top".into()))
}

/// A key edge of the remote's LEFT, as the dispatcher's input sees it.
fn left(app: &mut TestApp, edge: sn_machine::machine::Edge) {
    app.send(sn_machine::machine::InputKind::Key { key: Key::Left, sym: 0, wcode: 0, edge, at_edge: false });
}

/// LEFT on a row's first card opens the row's catalog whole, and BACK returns to the card; a held
/// LEFT walking back along the row stops at the first card instead (only a fresh press opens).
#[test]
fn left_on_a_rows_first_card_opens_its_catalog_and_a_held_left_stops_there() {
    use crate::registry::{CatalogRef, GridCmd};
    use crate::stores::grid::GridKey;
    use sn_machine::machine::Edge;
    let _g = sn_base::testlock::serial();
    let mut app = one_addon_home(20);
    let card = focus(&app);
    assert!(card.is_some_and(|e| e < sn_ui::geom::NAMESPACE_FLOOR), "seated on the first card: {card:?}");
    app.take_effects();
    app.press(Key::Left);
    let key = GridKey { addon_id: "com.linvo.cinemeta".into(), kind: "movie".into(), catalog_id: "top".into(), genre: None, search: None };
    assert_eq!(app.top_arg(), Some(&AppArg::Catalog(CatalogRef { key: key.clone(), title: "Cinemeta \u{2014} Popular".into() })));
    assert!(app.take_effects().contains(&AppFx::Grid(GridCmd::Open(key))), "the grid page holds its view open");
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home");
    assert_eq!(focus(&app), card, "BACK returns to the card LEFT was pressed on");
    // a held LEFT from the second card: its press moves to the first, its repeats stop there
    app.press(Key::Right);
    assert_ne!(focus(&app), card);
    left(&mut app, Edge::Down);
    for _ in 0..5 {
        left(&mut app, Edge::Repeat);
        app.frames(5);
    }
    left(&mut app, Edge::Up);
    app.frames(40);
    assert_eq!(app.top_name(), "home", "a held LEFT does not leave the page: {:?}", app.top_arg());
    assert_eq!(focus(&app), card, "it stops on the first card");
}

/// RIGHT past a row's last card reaches its See all tile (a card's stop, drawn with *See all* and
/// no artwork), RIGHT goes no further, OK opens the row's catalog, BACK returns to the tile, and a
/// held OK on it opens no item menu. The row still pages: the tile comes right after the loaded
/// cards.
#[test]
fn right_past_the_last_card_reaches_see_all_and_ok_opens_the_catalog() {
    let _g = sn_base::testlock::serial();
    let _l = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = one_addon_home(3);
    for _ in 0..3 {
        app.press(Key::Right);
    }
    let tile = Some(crate::home::more_elem(row(0)));
    assert_eq!(focus(&app), tile, "RIGHT past the third and last card");
    let stop = app.d.last_stops().iter().rev().find(|s| Some(s.key.elem) == tile).map(|s| s.rect).expect("the tile is a stop");
    let card = app.d.last_stops().iter().find(|s| s.key.elem < sn_ui::geom::NAMESPACE_FLOOR).map(|s| s.rect).expect("a card");
    assert!((stop.h / card.h - 1.09).abs() < 0.02, "the tile pops like the focused card it stands beside: {stop:?} vs {card:?}");
    assert!(app.drawn_text().iter().any(|t| t == "See all"), "the tile says what it does");
    app.press(Key::Right);
    assert_eq!(focus(&app), tile, "the tile ends the row");
    app.hold_ok();
    assert_eq!(app.top_name(), "home", "a held OK on the tile opens no item menu");
    app.press(Key::Ok);
    assert_eq!(catalog_on_top(&app), movies());
    app.press(Key::Back);
    assert_eq!(focus(&app), tile, "BACK returns to the tile");
    app.press(Key::Left);
    assert!(focus(&app).is_some_and(|e| e < sn_ui::geom::NAMESPACE_FLOOR), "LEFT from the tile is the last card");
}

/// The pointer clicks a See all tile through the same hit map: a card's click, which commits on
/// the button's release (`app::pointer`'s OK up).
#[test]
fn a_click_on_a_rows_see_all_tile_opens_its_catalog() {
    use sn_machine::machine::{Edge, InputKind};
    let _g = sn_base::testlock::serial();
    let mut app = one_addon_home(3);
    let r = app.d.last_stops().iter().rev().find(|s| s.key.elem == crate::home::more_elem(row(1))).map(|s| s.rect).expect("drawn");
    app.send(InputKind::Click { x: r.cx(), y: r.cy(), hit: None });
    app.send(InputKind::Key { key: Key::Ok, sym: 0, wcode: 0, edge: Edge::Up, at_edge: false });
    app.frames(40);
    assert_eq!(catalog_on_top(&app), Some(("series".into(), "top".into())), "{}", app.top_name());
}

/// A row's heading is the pointer's way to its catalog: a click on it opens the row whole, and
/// YELLOW on the catalog page closes it again. No D-pad move ever lands on a heading.
#[test]
fn a_click_on_a_rows_heading_opens_its_catalog_and_yellow_closes_it() {
    use crate::tabs::WCODE_YELLOW;
    use sn_machine::machine::{Edge, InputKind};
    let _g = sn_base::testlock::serial();
    let mut app = one_addon_home(20);
    let is_heading = |e: Option<u32>| e.is_some_and(|e| e >= sn_ui::geom::NAMESPACE_FLOOR + 0x0300_0000 && e < sn_ui::geom::NAMESPACE_FLOOR + 0x0400_0000);
    for key in [Key::Down, Key::Down, Key::Up, Key::Up, Key::Down, Key::Right, Key::Up, Key::Down] {
        app.press(key);
        assert!(!is_heading(focus(&app)), "{key:?} landed on a heading");
    }
    let r = app.d.last_stops().iter().find(|s| is_heading(Some(s.key.elem))).map(|s| s.rect).expect("a heading is a pointer stop");
    app.send(InputKind::Click { x: r.cx(), y: r.cy(), hit: None });
    app.send(InputKind::Key { key: Key::Ok, sym: 0, wcode: 0, edge: Edge::Up, at_edge: false });
    app.frames(40);
    assert_eq!(catalog_on_top(&app), movies(), "{}", app.top_name());
    app.press_raw(0, WCODE_YELLOW);
    app.frames(40);
    assert_eq!(app.top_name(), "home", "YELLOW closes the catalog it opened");
}

/// YELLOW on any card of a row opens that row's catalog (it is no tab shortcut any more); no
/// heading draws a key hint for it.
#[test]
fn yellow_on_a_card_opens_its_rows_catalog_without_a_hint() {
    use crate::tabs::{HotkeyRoute, WCODE_YELLOW};
    let _g = sn_base::testlock::serial();
    let _l = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = one_addon_home(20);
    app.press(Key::Down);
    app.press(Key::Right);
    app.press(Key::Right);
    let text = app.drawn_text();
    assert!(!text.iter().any(|t| t == "Yellow"), "no key hint on the heading: {text:?}");
    assert_eq!(app.press_raw(0, WCODE_YELLOW), HotkeyRoute::NotHotkey, "YELLOW goes on to the page");
    assert_eq!(catalog_on_top(&app), Some(("series".into(), "top".into())), "the focused card's row: the second");
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home");
    // on the dropdown row there is no row to open
    app.press(Key::Up);
    app.press(Key::Up);
    assert_eq!(focus(&app), Some(crate::home::TYPE_ELEM));
    app.press_raw(0, WCODE_YELLOW);
    assert_eq!(app.top_name(), "home", "YELLOW off a row does nothing");
}

/// UP and DOWN go card row to card row: no heading is a stop. UP from the first row: the dropdown
/// row, then the bar; DOWN from the bar and the dropdown row comes back to the first card.
#[test]
fn up_and_down_move_row_to_row_and_up_from_the_first_row_reaches_dropdown_row_and_bar() {
    let _g = sn_base::testlock::serial();
    let mut app = landed_home();
    let card = focus(&app);
    let is_card = |e: Option<u32>| e.is_some_and(|e| e < sn_ui::geom::NAMESPACE_FLOOR);
    app.press(Key::Down);
    let second = focus(&app);
    assert!(is_card(second) && second != card, "DOWN is the next row's card: {second:?}");
    app.press(Key::Down);
    let third = focus(&app);
    assert!(is_card(third) && third != second, "and the next: {third:?}");
    app.press(Key::Up);
    assert_eq!(focus(&app), second, "UP is the row above's card, no heading between");
    app.press(Key::Up);
    assert_eq!(focus(&app), card);
    for want in [Some(crate::home::TYPE_ELEM), Some(crate::tabs::Tab::Home.elem())] {
        app.press(Key::Up);
        assert_eq!(focus(&app), want);
    }
    app.press(Key::Down);
    assert_eq!(focus(&app), Some(crate::home::TYPE_ELEM));
    app.press(Key::Down);
    assert_eq!(focus(&app), card, "DOWN from the dropdown row lands on the first card");
}

/// List mode ends each row's list in a See all row: DOWN past the last listed item reaches it, OK
/// opens the catalog, and DOWN from it is the next row's first item (UP comes back).
#[test]
fn a_listed_row_ends_in_a_see_all_row() {
    let _g = sn_base::testlock::serial();
    let store = every_row_landed();
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.rig.settings.set_list(crate::stores::settings::ViewPage::Home, true);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    for _ in 0..crate::home::LIST_PREVIEW {
        app.press(Key::Down);
    }
    let see_all = Some(crate::home::more_elem(row(0)));
    assert_eq!(focus(&app), see_all, "the list's last row");
    app.press(Key::Down);
    let next = focus(&app);
    assert!(next.is_some_and(|e| e < sn_ui::geom::NAMESPACE_FLOOR), "DOWN is the next row's first item: {next:?}");
    app.press(Key::Up);
    assert_eq!(focus(&app), see_all, "UP comes back to it");
    app.press(Key::Ok);
    assert_eq!(catalog_on_top(&app), movies());
}

/// The View toggle at the dropdown row's end: the choice is the settings store's (persisted per
/// page), and the rows draw as lists — each row's first items as list rows whose element is the
/// card's, so OK opens the same item.
#[test]
fn the_view_toggle_lists_home_and_persists_the_choice() {
    use crate::stores::settings::ViewPage;
    let _g = sn_base::testlock::serial();
    let mut app = landed_home();
    let card = focus(&app).expect("seated");
    let poster_w = app.d.last_stops().iter().rev().find(|s| s.key.elem == card).map(|s| s.rect.w).expect("drawn");
    app.press(Key::Up);
    app.press(Key::Right);
    assert_eq!(focus(&app), Some(crate::home::TOGGLE_ELEM));
    app.press(Key::Ok);
    assert!(app.rig.settings.list(ViewPage::Home), "the choice is the settings store's (persisted)");
    assert!(!app.rig.settings.list(ViewPage::Catalog), "per page");
    app.notify(StoreId::Settings, app.rig.settings.gen());
    app.frames(5);
    assert_eq!(focus(&app), Some(crate::home::TOGGLE_ELEM), "focus stays on the toggle");
    app.press(Key::Down);
    assert_eq!(focus(&app), Some(card), "DOWN lands on the first list row: the first card's element");
    let w = app.d.last_stops().iter().rev().find(|s| s.key.elem == card).map(|s| s.rect.w).expect("drawn");
    assert!(w > 4.0 * poster_w, "a list row spans the page ({w} vs a poster's {poster_w})");
    let next = {
        app.press(Key::Down);
        focus(&app)
    };
    assert!(next.is_some_and(|e| e != card && e < sn_ui::geom::NAMESPACE_FLOOR), "DOWN walks the rows: {next:?}");
    app.press(Key::Ok);
    assert!(matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id == "tt0-1"), "OK opens the row's item: {:?}", app.top_arg());
    app.press(Key::Back);
    // and back to posters
    app.rig.settings.set_list(ViewPage::Home, false);
    app.notify(StoreId::Settings, app.rig.settings.gen());
    app.frames(5);
    let w = app.d.last_stops().iter().rev().find(|s| Some(s.key.elem) == next).map(|s| s.rect.w).expect("drawn");
    assert!(w < 2.0 * poster_w, "posters again: {w}");
}

/// List mode stays lazy: the window the store is told is the rows whose list section is on
/// screen, and a list row off the screen registers (and draws) nothing.
#[test]
fn list_mode_asks_only_for_the_rows_on_screen() {
    let _g = sn_base::testlock::serial();
    let store = every_row_landed();
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.rig.settings.set_list(crate::stores::settings::ViewPage::Home, true);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    let view = views(&app.take_effects()).last().cloned().expect("a window");
    assert_eq!(view.start, 0);
    assert!(view.end <= 2, "a list section is a screenful: the rows past it are not asked for ({view:?})");
    let drawn: Vec<u32> = app.d.last_stops().iter().map(|s| s.key.elem).collect();
    let n = crate::home::LIST_PREVIEW;
    assert!(drawn.iter().filter(|e| **e < sn_ui::geom::NAMESPACE_FLOOR).count() < n, "only the rows on screen register: {drawn:?}");
    assert!(focus(&app).is_some_and(|e| e < sn_ui::geom::NAMESPACE_FLOOR), "the boot seat is the first list row: {:?}", app.d.focus());
    // walking down to the third row's list moves the window on, the first row scrolled away
    for _ in 0..2 * n + 2 {
        app.press(Key::Down);
    }
    app.frames(30);
    let last = views(&app.take_effects()).last().cloned().expect("the window moved");
    assert!(last.start >= 1 && last.end <= 4, "{last:?}");
}

/// App-owned text on Home fits in every shipped language: the dropdown row (the widest type
/// label and the View chip) on one line, Continue Watching's heading in half the page, and *See all* on one line inside its tile.
#[test]
fn home_strings_fit_in_every_shipped_language() {
    use sn_machine::machine::Measure;
    use sn_platform::i18n::{language_on_this_thread_for_test, msg, SHIPPED};
    use sn_ui::consts::{MARGIN_X, SCR_W};
    let m = sn_base::fontcov::advances::ShippedMeasure;
    let safe = SCR_W - 2.0 * MARGIN_X;
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        let widest = [msg::app_filter_all_types(), msg::app_kind_movie(), msg::app_kind_series(), msg::app_kind_channel(), msg::app_kind_tv()]
            .into_iter()
            .map(|l| crate::dropdown::face_w(l, &m))
            .fold(0.0, f32::max);
        let row = crate::dropdown::row(&[widest], Some(crate::item_list::toggle_width(&m)), 0.0);
        assert!(row[0].x + row[0].w < row[1].x && row[1].x + row[1].w <= SCR_W - MARGIN_X + 0.5, "{}: {row:?}", language.tag());
        let text = m.width_str(msg::app_home_continue(), sn_ui::theme::size::HEADLINE, true);
        assert!(text <= safe / 2.0, "{}: the Continue Watching heading is {text}px", language.tag());
        let tile = m.width_str(msg::widgets_cards_see_all(), sn_ui::theme::size::BODY, true);
        assert!(tile <= sn_ui::consts::CARD_W - 2.0 * sn_ui::theme::space::MD, "{}: See all is {tile}px on its tile", language.tag());
    }
}


/// The rows stand (headings and skeletons) before their pages land: a row's See all tile is no
/// stop until the row holds cards, or the engine seated it and OK opened a catalog page instead of
/// the first card (the app rig's `home_rows_fill_from_the_installed_addons_and_ok_reaches_the_streams`).
#[test]
fn a_heading_standing_before_its_cards_does_not_take_the_boot_seat() {
    let _g = sn_base::testlock::serial();
    let mut app = TestApp::boot(CatalogStore::signed_out());
    app.frames(3);
    let jobs = app.rig.catalog.take_jobs();
    assert!(matches!(&jobs[..], [Job::Manifest { source: 0, .. }]));
    app.rig.catalog.land(Landing::Manifest { source: 0, result: Ok(CINEMETA.as_bytes().to_vec()) });
    let gen = app.rig.catalog.gen();
    app.notify(StoreId::Catalog, gen);
    app.frames(5);
    assert_eq!(focus(&app), Some(crate::tabs::Tab::Home.elem()), "rows without cards: the remote stays on the pill");
    // a page whose engine has no focus at all (the application's own Home root) seats the first
    // non-empty group: no heading may be one yet
    assert!(
        app.d.last_stops().iter().all(|s| s.key.elem != crate::home::more_elem(row(0))),
        "a row without cards has no See all stop"
    );
    for job in app.rig.catalog.take_jobs() {
        if let Job::Page { row, .. } = job {
            app.rig.catalog.land(Landing::Page { row, result: Ok(page(&format!("tt{}-", row.catalog), 20)) });
        }
    }
    let gen = app.rig.catalog.gen();
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail", "the first card took the seat: {:?}", app.top_arg());
}

/// A row whose first page failed is a stop: DOWN from the row above reaches its *Try again*, OK
/// asks the row again (no promise of a retry "when you return").
#[test]
fn a_failed_row_offers_a_reachable_retry() {
    let _g = sn_base::testlock::serial();
    let mut store = CatalogStore::new(["https://one.example/manifest.json"]);
    assert!(matches!(&store.take_jobs()[..], [Job::Manifest { source: 0, .. }]));
    store.land(Landing::Manifest { source: 0, result: Ok(CINEMETA.as_bytes().to_vec()) });
    store.set_view(0..6);
    for job in store.take_jobs() {
        if let Job::Page { row, .. } = job {
            let result = if row.catalog == 1 { Err(sn_stremio::FetchError::Transport("down".into())) } else { Ok(page(&format!("tt{}-", row.catalog), 8)) };
            store.land(Landing::Page { row, result });
        }
    }
    let gen = store.gen();
    let mut app = TestApp::boot(store);
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    assert!(focus(&app).is_some_and(|e| e >= 0x1000), "the first row's card is seated: {:?}", focus(&app));
    app.press(Key::Down);
    assert_eq!(focus(&app), Some(crate::home::row_retry_elem(row(1))), "DOWN lands on the failed row's Try again");
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(app.take_effects().into_iter().filter(|fx| *fx == AppFx::Catalog(CatalogCmd::Retry(row(1)))).count(), 1, "one press, one retry");
    app.press(Key::Down);
    assert!(focus(&app).is_some_and(|e| (0x1000..0x4000_0000).contains(&e)), "and DOWN goes on to the next row: {:?}", focus(&app));
}

/// A row whose NEXT page failed does not look finished: its last tile says *Couldn't load more*,
/// and OK on it asks the row again.
#[test]
fn a_rows_failed_next_page_ends_in_a_try_again_tile() {
    let _g = sn_base::testlock::serial();
    let mut app = one_addon_home(3);
    let id = row(0);
    app.rig.catalog.want_more(id);
    for job in app.rig.catalog.take_jobs() {
        if let Job::Page { row, .. } = job {
            app.rig.catalog.land(Landing::Page { row, result: Err(sn_stremio::FetchError::Transport("down".into())) });
        }
    }
    let gen = app.rig.catalog.gen();
    app.notify(StoreId::Catalog, gen);
    app.frames(3);
    for _ in 0..3 {
        app.press(Key::Right);
    }
    assert_eq!(focus(&app), Some(crate::home::row_retry_elem(id)), "RIGHT past the last card reaches the Try again tile");
    let text = app.drawn_text().join(" ");
    assert!(text.contains(sn_platform::i18n::msg::widgets_cards_try_again()), "{text}");
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(app.take_effects().into_iter().filter(|fx| *fx == AppFx::Catalog(CatalogCmd::Retry(id))).count(), 1);
}
