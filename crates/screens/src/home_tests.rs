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
            AppFx::Catalog(CatalogCmd::View(v)) => Some(v.clone()),
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
        Some(&AppArg::Detail(ItemRef { kind: "movie".into(), id: "tt0-0".into(), name: "Title 0".into() }))
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

#[test]
fn a_shelf_reaching_its_end_asks_for_more_of_its_row() {
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
    app.take_effects();
    for _ in 0..7 {
        app.press(Key::Right);
    }
    let effects = app.take_effects();
    assert!(
        effects.iter().any(|fx| matches!(fx, AppFx::Catalog(CatalogCmd::More(r)) if *r == row(0))),
        "walking to the end of a short row asks for its next page: {effects:?}"
    );
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
    let jobs = app.rig.detail.open_streams(&addons, key.clone());
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
