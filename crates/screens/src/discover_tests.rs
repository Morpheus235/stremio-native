//! Discover: the sub-tab, catalog and genre derivation from real addon manifests (trimmed copies
//! of the public Cinemeta, Anime Kitsu and TvVoo manifests, fetched 2026-10-17), the paging
//! requests each selection makes, and the page through the real dispatcher (`TestApp`): the
//! dropdown row and its popovers by the remote and the pointer, BACK, the session memory, the
//! held views, and Live TV as its sub-tab.

use sn_machine::machine::{InputKind, Key, MachineId, NavOp};
use sn_platform::i18n::{language_on_this_thread_for_test, Preference, SHIPPED};
use sn_stremio::manifest::{Addon, Manifest};
use sn_stremio::types::{CatalogPage, MetaPreview};

use crate::discover::{
    catalog_labels, catalogs, in_tab, sub_tab_label, sub_tabs, DiscoverCatalog, DiscoverScreen, CATALOG_ELEM, CONTENT_TOP, GENRE_ELEM, LIVE,
    TOGGLE_ELEM, TYPE_ELEM,
};
use crate::registry::{AppArg, AppFx, ChoiceTarget, GridCmd, MenuKind};
use crate::stores::catalog::CatalogStore;
use crate::stores::grid::{GridKey, GridLanding, GridStore};
use crate::stores::StoreId;
use crate::tabs::{Tab, WCODE_GREEN};
use crate::test_support::TestApp;

/// `https://v3-cinemeta.strem.io/manifest.json`, genre options cut to four.
const CINEMETA: &str = r#"{"id":"com.linvo.cinemeta","version":"3.0.14","name":"Cinemeta","resources":["catalog","meta","addon_catalog"],"types":["movie","series"],"idPrefixes":["tt"],"catalogs":[{"type":"movie","id":"top","genres":["Action","Adventure","Animation","Biography"],"extra":[{"name":"genre","options":["Action","Adventure","Animation","Biography"]},{"name":"search"},{"name":"skip"}],"extraSupported":["search","genre","skip"],"name":"Popular"},{"type":"series","id":"top","genres":["Action","Adventure","Animation","Biography"],"extra":[{"name":"genre","options":["Action","Adventure","Animation","Biography"]},{"name":"search"},{"name":"skip"}],"extraSupported":["search","genre","skip"],"name":"Popular"},{"type":"movie","id":"year","genres":["2026","2025","2024","2023"],"extra":[{"name":"genre","options":["2026","2025","2024","2023"],"isRequired":true},{"name":"skip"}],"extraSupported":["genre","skip"],"extraRequired":["genre"],"name":"New"},{"type":"series","id":"imdbRating","genres":["Action","Adventure","Animation","Biography"],"extra":[{"name":"genre","options":["Action","Adventure","Animation","Biography"]},{"name":"skip"}],"extraSupported":["genre","skip"],"name":"Featured"},{"type":"series","id":"last-videos","extra":[{"name":"lastVideosIds","isRequired":true,"optionsLimit":100}],"extraSupported":["lastVideosIds"],"extraRequired":["lastVideosIds"],"name":"Last videos"}]}"#;
/// `https://anime-kitsu.strem.fun/manifest.json`, genre options cut to four (two with spaces).
const KITSU: &str = r#"{"id":"community.anime.kitsu","version":"0.0.10","name":"Anime Kitsu","resources":["catalog","meta","subtitles"],"types":["anime","movie","series"],"idPrefixes":["kitsu","mal","anilist","anidb"],"catalogs":[{"id":"kitsu-anime-trending","name":"Kitsu Trending","type":"anime","extra":[]},{"id":"kitsu-anime-airing","name":"Kitsu Top Airing","type":"anime","extra":[{"name":"genre","options":["Action","Comedy","Slice of Life","Martial Arts"]},{"name":"skip"}]},{"id":"kitsu-anime-list","name":"Kitsu","type":"anime","extra":[{"name":"search","isRequired":true},{"name":"lastVideosIds","isRequired":false,"optionsLimit":20},{"name":"skip"}]},{"id":"kitsu-anime-last-videos","name":"Last videos","type":"series","extra":[{"name":"lastVideosIds","isRequired":true,"optionsLimit":20}]}]}"#;
/// `https://tvvoo.hayd.uk/manifest.json`: two of its country catalogs and one search catalog,
/// genre options cut to four.
const TVVOO: &str = r#"{"id":"org.stremio.vavoo.clean","version":"5.1.0","name":"TvVoo | ElfHosted","resources":["catalog","meta","stream"],"types":["tv"],"idPrefixes":["vavoo","vavoo_"],"catalogs":[{"id":"vavoo_tv_it","type":"tv","name":"Vavoo TV • Italia","extra":[{"name":"search","isRequired":false},{"name":"skip","isRequired":false},{"name":"date","isRequired":false},{"name":"genre","options":["Tutti","Altro","Bambini","Documentari"],"isRequired":false}]},{"id":"vavoo_tv_uk","type":"tv","name":"Vavoo TV • United Kingdom","extra":[{"name":"search","isRequired":false},{"name":"skip","isRequired":false},{"name":"date","isRequired":false},{"name":"genre","options":["Tutti","Documentary","Kids","Movies"],"isRequired":false}]},{"id":"vavoo_search_it","type":"tv","name":"🔎 Italia","extra":[{"name":"search","isRequired":true}]}]}"#;
/// A channel addon whose genre is required through the legacy `extraRequired` list only, and a
/// second "Popular" movie catalog.
const OTHER: &str = r#"{"id":"org.example.other","name":"Other","catalogs":[
    {"type":"channel","id":"ch","name":"Channels","extra":[{"name":"genre","options":["News & Talk","Sport"]},{"name":"skip"}],"extraRequired":["genre"]},
    {"type":"movie","id":"pop","name":"Popular"},
    {"type":"podcasts","id":"pods","name":"Pods"}]}"#;

fn addon(url: &str, body: &str) -> Addon {
    Addon::new(url, Manifest::parse(body.as_bytes()).unwrap())
}

fn addons() -> Vec<Addon> {
    vec![
        addon("https://v3-cinemeta.strem.io/manifest.json", CINEMETA),
        addon("https://anime-kitsu.strem.fun/manifest.json", KITSU),
        addon("https://tvvoo.hayd.uk/manifest.json", TVVOO),
    ]
}

fn names(row: &[&DiscoverCatalog]) -> Vec<String> {
    row.iter().map(|c| c.catalog.display_name().to_string()).collect()
}

fn find<'a>(all: &'a [DiscoverCatalog], kind: &str, id: &str) -> &'a DiscoverCatalog {
    all.iter().find(|c| c.catalog.kind == kind && c.catalog.id == id).unwrap()
}

#[test]
fn the_sub_tabs_are_the_browsable_catalogs_types_movies_series_anime_live_tv_then_the_rest() {
    let all = catalogs(&addons());
    assert_eq!(sub_tabs(&all), ["movie", "series", "anime", "tv"]);
    let mut more = addons();
    more.push(addon("https://other.example/manifest.json", OTHER));
    let all = catalogs(&more);
    assert_eq!(sub_tabs(&all), ["movie", "series", "anime", "tv", "podcasts"], "channel joins tv as Live TV; the rest after");
    assert_eq!(names(&in_tab(&all, LIVE)), ["Vavoo TV • Italia", "Vavoo TV • United Kingdom", "Channels"]);
    let _g = language_on_this_thread_for_test(Preference::En);
    assert_eq!(["movie", "series", "anime", LIVE, "podcasts"].map(sub_tab_label), ["Movies", "Series", "Anime", "Live TV", "Podcasts"]);
}

#[test]
fn a_sub_tabs_catalogs_are_the_ones_nothing_typed_can_open_in_install_order() {
    let all = catalogs(&addons());
    assert_eq!(names(&in_tab(&all, "movie")), ["Popular", "New"]);
    assert_eq!(names(&in_tab(&all, "series")), ["Popular", "Featured"], "Last videos needs ids: not browsable");
    assert_eq!(names(&in_tab(&all, "anime")), ["Kitsu Trending", "Kitsu Top Airing"], "Kitsu's search-only catalog is not offered");
    assert_eq!(names(&in_tab(&all, LIVE)), ["Vavoo TV • Italia", "Vavoo TV • United Kingdom"], "nor TvVoo's");
    let mut more = addons();
    more.push(addon("https://other.example/manifest.json", OTHER));
    let all = catalogs(&more);
    assert_eq!(catalog_labels(&in_tab(&all, "movie")), ["Popular \u{b7} Cinemeta", "New", "Popular \u{b7} Other"], "a shared name says whose");
    assert_eq!(catalog_labels(&in_tab(&all, "series")), ["Popular", "Featured"]);
}

#[test]
fn the_genres_are_the_catalogs_options_with_all_first_unless_required() {
    let _g = language_on_this_thread_for_test(Preference::En);
    let mut more = addons();
    more.push(addon("https://other.example/manifest.json", OTHER));
    let all = catalogs(&more);
    let keys = |c: &DiscoverCatalog| c.genre_chips().into_iter().map(|(k, _)| k).collect::<Vec<_>>();
    let popular = find(&all, "movie", "top");
    assert_eq!(keys(popular), ["", "Action", "Adventure", "Animation", "Biography"]);
    assert_eq!(popular.genre_chips()[0].1, "All");
    assert_eq!(keys(find(&all, "movie", "year")), ["2026", "2025", "2024", "2023"], "isRequired: no All");
    assert_eq!(keys(find(&all, "channel", "ch")), ["News & Talk", "Sport"], "extraRequired alone: no All either");
    assert!(find(&all, "anime", "kitsu-anime-trending").genre_chips().is_empty(), "no genre extra: no genre row");
}

/// The URL each selection asks, page after page (`skip`), encoded as stremio-core encodes extras.
#[test]
fn each_selection_asks_its_genre_and_pages_by_skip() {
    let mut more = addons();
    more.push(addon("https://other.example/manifest.json", OTHER));
    let all = catalogs(&more);
    let mut s = GridStore::new();
    let mut walk = |key: GridKey| {
        let first = s.open(&more, key.clone(), None).pop().unwrap().url;
        let page = CatalogPage { metas: (0..20).map(|i| MetaPreview { id: format!("i{i}"), name: format!("I{i}"), ..Default::default() }).collect() };
        s.land(GridLanding { key: key.clone(), result: Ok(page) });
        (first, s.more(&key).pop().unwrap().url)
    };
    let cinemeta = "https://v3-cinemeta.strem.io/catalog";
    let popular = find(&all, "movie", "top");
    assert_eq!(walk(popular.key(None)), (format!("{cinemeta}/movie/top.json"), format!("{cinemeta}/movie/top/skip=20.json")), "All sends no genre");
    assert_eq!(
        walk(popular.key(Some("Action"))),
        (format!("{cinemeta}/movie/top/genre=Action.json"), format!("{cinemeta}/movie/top/genre=Action&skip=20.json"))
    );
    assert_eq!(walk(find(&all, "movie", "year").key(None)).0, format!("{cinemeta}/movie/year/genre=2026.json"), "a required genre starts on its first option");
    let airing = find(&all, "anime", "kitsu-anime-airing");
    assert_eq!(
        walk(airing.key(Some("Slice of Life"))).1,
        "https://anime-kitsu.strem.fun/catalog/anime/kitsu-anime-airing/genre=Slice%20of%20Life&skip=20.json"
    );
    assert_eq!(walk(find(&all, "channel", "ch").key(None)).0, "https://other.example/catalog/channel/ch/genre=News%20%26%20Talk.json");
}

/// Every Discover option the app owns (the sub-tabs, All) fits a quarter of the row, and the top
/// bar with the Discover pill fits the safe width, in every shipped language.
#[test]
fn the_discover_labels_fit_in_every_shipped_language() {
    use sn_ui::widgets::Button;
    let m = sn_base::fontcov::advances::ShippedMeasure;
    let row = sn_ui::consts::SCR_W - 2.0 * sn_ui::consts::MARGIN_X;
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        let labels = ["movie", "series", "anime", LIVE, ""].map(|t| if t.is_empty() { sn_platform::i18n::msg::app_filter_all().to_string() } else { sub_tab_label(t) });
        for l in labels {
            let w = Button::pill_w_measured(&std::ffi::CString::new(l.clone()).unwrap(), sn_ui::theme::size::BODY, true, false, &m);
            assert!(w <= row / 4.0, "{}: {l:?} is {w}px chosen", language.tag());
        }
        // the whole bar, Discover's pill among them, stands without scrolling
        let labels: Vec<String> = crate::tabs::TABS.iter().map(|e| if e.tab == Tab::Search { String::new() } else { e.tab.label().to_string() }).collect();
        let bar: f32 = sn_ui::widgets::tab_widths(&labels, &m).iter().sum();
        assert!(bar <= row, "{}: the bar is {bar}px", language.tag());
    }
}

// --- the page ---------------------------------------------------------------------------------

fn boot() -> TestApp {
    let mut app = TestApp::boot(CatalogStore::from_addons(addons()));
    app.notify(StoreId::Catalog, 1);
    app.frames(3);
    app
}

fn screen(app: &TestApp) -> &DiscoverScreen {
    app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<DiscoverScreen>()).expect("Discover is up")
}

fn focus(app: &TestApp) -> Option<u32> {
    app.d.focus().map(|k| k.elem)
}

fn opened(app: &mut TestApp) -> Vec<GridKey> {
    app.take_effects()
        .into_iter()
        .filter_map(|fx| match fx {
            AppFx::Grid(GridCmd::Open(k)) => Some(k),
            _ => None,
        })
        .collect()
}

fn land(app: &mut TestApp, key: &GridKey, n: usize) {
    let metas = (0..n).map(|i| MetaPreview { id: format!("tt{i}"), kind: key.kind.clone(), name: format!("T{i}"), ..Default::default() }).collect();
    app.rig.grid.land(GridLanding { key: key.clone(), result: Ok(CatalogPage { metas }) });
    app.notify(StoreId::Grid, app.rig.grid.gen());
    app.frames(3);
}

fn key(kind: &str, id: &str, genre: Option<&str>) -> GridKey {
    let addon = if kind == "tv" { "org.stremio.vavoo.clean" } else { "com.linvo.cinemeta" };
    GridKey { addon_id: addon.into(), kind: kind.into(), catalog_id: id.into(), genre: genre.map(Into::into), search: None }
}

/// The popover over the page, its rows as it shows them now.
pub(crate) fn menu_of(app: &TestApp) -> MenuKind {
    let menu = app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::menu::MenuScreen>());
    menu.unwrap_or_else(|| panic!("a popover, not {}", app.top_name())).kind().clone()
}

/// The popover's anchor and where it stands.
pub(crate) fn menu_panel(app: &TestApp) -> (sn_ui::Rect, Option<sn_ui::Rect>) {
    match app.d.nav.modals.surfaces.last().map(|s| s.entry.arg.clone()) {
        Some(AppArg::Menu(m)) => (m.anchor_rect(), crate::dropdown::panel(&m, 0.0, 0.0)),
        other => panic!("a popover: {other:?}"),
    }
}

/// Walk the open choice popover from its current option to option `n` and press OK.
pub(crate) fn pick(app: &mut TestApp, n: usize) {
    let MenuKind::Choice { current, .. } = menu_of(app) else { panic!("a choice") };
    let key = if n > current { Key::Down } else { Key::Up };
    for _ in 0..n.abs_diff(current) {
        app.press(key);
    }
    app.press(Key::Ok);
}

/// Click the last drawn stop of element `elem` (a popover row is `0x10 + n`).
pub(crate) fn click(app: &mut TestApp, elem: u32) {
    app.frames(30); // a popover in motion holds the pointer
    let r = app.d.last_stops().iter().rev().find(|s| s.key.elem == elem).map(|s| s.rect).expect("drawn");
    app.send(InputKind::Click { x: r.cx(), y: r.cy(), hit: None });
    app.frames(60);
}

#[test]
fn green_opens_discover_on_movies_and_the_dpad_walks_the_dropdown_row_and_the_grid() {
    let _g = sn_base::testlock::serial();
    let _l = language_on_this_thread_for_test(Preference::En);
    let mut app = boot();
    app.take_effects();
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    assert_eq!(app.top_name(), "discover", "Discover is the pill after Home");
    assert_eq!(opened(&mut app), [key("movie", "top", None)], "Movies, its first catalog, All genres");
    let s = screen(&app);
    assert_eq!(focus(&app), Some(TYPE_ELEM), "seated on the type dropdown");
    assert_eq!(s.bar(), [TYPE_ELEM, CATALOG_ELEM, GENRE_ELEM, TOGGLE_ELEM], "one row: type, catalog, genre, then View");
    let values = |app: &TestApp| [ChoiceTarget::DiscoverType, ChoiceTarget::DiscoverCatalog, ChoiceTarget::DiscoverGenre].map(|t| screen(app).value(t));
    assert_eq!(values(&app), [Some("Movies".into()), Some("Popular".into()), Some("All genres".into())], "each says its value");
    // the type: OK opens its popover on the current option, a pick applies and closes it
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "menu");
    let MenuKind::Choice { options, current, target, .. } = menu_of(&app) else { panic!("a choice") };
    assert_eq!((options, current, target), (vec!["Movies".to_string(), "Series".into(), "Anime".into(), "Live TV".into()], 0, ChoiceTarget::DiscoverType));
    let (anchor, panel) = menu_panel(&app);
    let panel = panel.expect("a dropdown's popover");
    assert!(panel.y >= anchor.y + anchor.h && panel.x == anchor.x, "under its button: {anchor:?} {panel:?}");
    pick(&mut app, 1);
    assert_eq!(app.top_name(), "discover", "a pick closes the popover");
    assert_eq!(focus(&app), Some(TYPE_ELEM), "focus returns to the button");
    assert_eq!(screen(&app).tab(), Some("series"));
    assert_eq!(opened(&mut app), [key("series", "top", None)]);
    // RIGHT along the row: the catalog
    app.press(Key::Right);
    assert_eq!(focus(&app), Some(CATALOG_ELEM));
    app.press(Key::Ok);
    pick(&mut app, 1);
    assert_eq!(opened(&mut app), [key("series", "imdbRating", None)]);
    assert_eq!(screen(&app).value(ChoiceTarget::DiscoverCatalog).as_deref(), Some("Featured"));
    // BACK closes a popover with nothing changed
    app.press(Key::Right);
    assert_eq!(focus(&app), Some(GENRE_ELEM));
    app.press(Key::Ok);
    assert!(matches!(menu_of(&app), MenuKind::Choice { target: ChoiceTarget::DiscoverGenre, current: 0, .. }), "All, checked");
    app.press(Key::Down);
    app.press(Key::Back);
    assert_eq!(app.top_name(), "discover");
    assert_eq!(focus(&app), Some(GENRE_ELEM));
    assert!(opened(&mut app).is_empty(), "nothing asked");
    // the genre
    app.press(Key::Ok);
    pick(&mut app, 1);
    let k = key("series", "imdbRating", Some("Action"));
    assert_eq!(opened(&mut app), [k.clone()]);
    assert_eq!(screen(&app).value(ChoiceTarget::DiscoverGenre).as_deref(), Some("Action"));
    assert_eq!(app.rig.grid.next_url(&k).as_deref(), Some("https://v3-cinemeta.strem.io/catalog/series/imdbRating/genre=Action.json"));
    app.press(Key::Right);
    assert_eq!(focus(&app), Some(TOGGLE_ELEM), "View closes the row");
    app.press(Key::Right);
    assert_eq!(focus(&app), Some(TOGGLE_ELEM), "the row's end");
    // the grid pages by skip after the pick, as before it
    land(&mut app, &k, 30);
    assert_eq!(app.rig.grid.next_url(&k).as_deref(), Some("https://v3-cinemeta.strem.io/catalog/series/imdbRating/genre=Action&skip=30.json"));
    // the grid, and BACK out through the row to the bar
    app.press(Key::Down);
    let card = focus(&app);
    assert!(card.is_some_and(|e| e < sn_ui::geom::NAMESPACE_FLOOR), "DOWN reaches the grid: {:?}", app.d.focus());
    app.press(Key::Up);
    assert!(focus(&app).is_some_and(|e| screen(&app).bar().contains(&e)), "UP from the grid's first row is the row");
    app.press(Key::Down);
    app.press(Key::Back);
    assert_eq!(focus(&app), Some(TYPE_ELEM), "BACK from the grid returns to the type dropdown");
    app.press(Key::Up);
    assert!(focus(&app).is_some_and(crate::tabs::on_strip), "UP from the row is the bar");
    app.press(Key::Down);
    app.press(Key::Back);
    assert_eq!(focus(&app), Some(Tab::Discover.elem()), "BACK from the row is the bar");
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home");
}

/// A catalog with no genre options has no genre dropdown.
#[test]
fn the_genre_dropdown_stands_only_when_the_catalog_has_options() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    app.press(Key::Ok);
    pick(&mut app, 2); // Anime: Kitsu Trending, no genre extra
    assert_eq!(screen(&app).bar(), [TYPE_ELEM, CATALOG_ELEM, TOGGLE_ELEM]);
    assert_eq!(screen(&app).value(ChoiceTarget::DiscoverGenre), None);
    app.press(Key::Right);
    app.press(Key::Ok);
    pick(&mut app, 1); // Kitsu Top Airing: genres
    assert_eq!(screen(&app).bar(), [TYPE_ELEM, CATALOG_ELEM, GENRE_ELEM, TOGGLE_ELEM]);
}

/// A long option list scrolls inside its popover and opens on the current option.
#[test]
fn a_long_genre_list_opens_on_its_choice_and_scrolls_inside_the_popover() {
    let _g = sn_base::testlock::serial();
    let genres: Vec<String> = (0..60).map(|i| format!("\"G{i}\"")).collect();
    let body = format!(r#"{{"id":"x.long","name":"Long","catalogs":[{{"type":"movie","id":"l","name":"Long","extra":[{{"name":"genre","options":[{}]}}]}}]}}"#, genres.join(","));
    let mut app = TestApp::boot(CatalogStore::from_addons(vec![addon("https://l.example/manifest.json", &body)]));
    app.notify(StoreId::Catalog, 1);
    app.frames(3);
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    app.press(Key::Right);
    app.press(Key::Right);
    assert_eq!(focus(&app), Some(GENRE_ELEM));
    app.press(Key::Ok);
    pick(&mut app, 50);
    assert_eq!(screen(&app).value(ChoiceTarget::DiscoverGenre).as_deref(), Some("G49"));
    app.press(Key::Ok);
    assert!(matches!(menu_of(&app), MenuKind::Choice { current: 50, .. }), "opens on the choice");
    app.frames(40);
    let menu = app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::menu::MenuScreen>()).expect("the popover");
    let rows = menu.table().n_rows();
    assert_eq!(rows, 61);
    let stop = app.d.last_stops().iter().rev().find(|s| s.key.elem == 0x10 + 50).map(|s| s.rect).expect("the focused option is drawn");
    assert!(stop.y + stop.h <= sn_ui::consts::SCR_H, "scrolled into the panel: {stop:?}");
    app.press(Key::Back);
    assert_eq!(app.top_name(), "discover");
}

/// The pointer: a click on a button opens its popover, a click on an option picks it.
#[test]
fn a_click_opens_a_dropdown_and_a_click_picks() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    app.take_effects();
    click(&mut app, CATALOG_ELEM);
    assert!(matches!(menu_of(&app), MenuKind::Choice { target: ChoiceTarget::DiscoverCatalog, current: 0, .. }));
    click(&mut app, 0x10 + 1);
    assert_eq!(app.top_name(), "discover");
    assert_eq!(opened(&mut app), [key("movie", "year", Some("2026"))], "New, on its first required genre");
    click(&mut app, GENRE_ELEM);
    assert!(matches!(menu_of(&app), MenuKind::Choice { target: ChoiceTarget::DiscoverGenre, .. }));
    click(&mut app, 0x10 + 2);
    assert_eq!(opened(&mut app), [key("movie", "year", Some("2024"))]);
}

#[test]
fn ok_on_a_card_opens_its_page() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    land(&mut app, &key("movie", "top", None), 12);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert!(matches!(app.top_arg(), Some(AppArg::Detail(i)) if i.id == "tt0"), "{:?}", app.top_arg());
}

#[test]
fn a_held_grid_card_stays_drawn_while_its_menu_is_up() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    land(&mut app, &key("movie", "top", None), 12);
    app.press(Key::Down);
    app.frames(20);
    crate::nav_tests::the_held_card_stays_drawn(&mut app, "T0");
}

/// Switching back to a selection the page already loaded asks nothing; a remount remembers the
/// sub-tab, catalog and genre (and asks again: the grid store forgets a view nobody holds).
#[test]
fn a_loaded_selection_is_not_asked_again_and_the_choices_outlive_the_page() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    let popular = key("movie", "top", None);
    assert_eq!(opened(&mut app), [popular.clone()]);
    land(&mut app, &popular, 20);
    app.press(Key::Right);
    app.press(Key::Right);
    app.press(Key::Ok);
    pick(&mut app, 1); // Action
    let action = key("movie", "top", Some("Action"));
    assert_eq!(opened(&mut app), [action.clone()]);
    app.press(Key::Ok);
    pick(&mut app, 0); // All again
    let effects = app.take_effects();
    assert!(!effects.iter().any(|fx| matches!(fx, AppFx::Grid(GridCmd::Open(_) | GridCmd::Close(_)))), "held: {effects:?}");
    assert_eq!(screen(&app).key(), Some(&popular));
    assert_eq!(app.rig.grid.items(&popular).len(), 20, "shown as it stood");
    app.press(Key::Ok);
    pick(&mut app, 1); // Action, remembered for this catalog
    app.take_effects();
    // leave and come back
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    assert_eq!(app.top_name(), "library");
    let closed = app.take_effects().into_iter().filter(|fx| matches!(fx, AppFx::Grid(GridCmd::Close(_)))).count();
    assert_eq!(closed, 2, "leaving lets every held view go");
    app.press_raw(0, sn_ui::consts::WCODE_CH_UP_KEY);
    app.frames(3);
    assert_eq!(app.top_name(), "discover");
    assert_eq!(opened(&mut app), [action], "the session's choice");
}

/// Live TV is Discover on its Live TV sub-tab: the channel catalogs, their genres, their cards.
#[test]
fn live_tv_opens_discover_on_its_live_tv_sub_tab() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.take_effects();
    app.d.request(MachineId::Nav, NavOp::SelectTab(AppArg::LiveTv));
    app.frames(4);
    assert_eq!(app.top_name(), "discover");
    assert_eq!(screen(&app).tab(), Some(LIVE));
    let it = key("tv", "vavoo_tv_it", None);
    assert_eq!(opened(&mut app), [it.clone()]);
    assert_eq!(focus(&app), Some(TYPE_ELEM));
    app.press(Key::Ok);
    assert!(matches!(menu_of(&app), MenuKind::Choice { current: 3, .. }), "Live TV is the fourth type");
    app.press(Key::Back);
    app.press(Key::Right);
    app.press(Key::Ok);
    pick(&mut app, 1); // Vavoo TV • United Kingdom
    app.press(Key::Right);
    app.press(Key::Ok);
    pick(&mut app, 1); // Tutti
    let tutti = key("tv", "vavoo_tv_uk", Some("Tutti"));
    assert_eq!(opened(&mut app), [key("tv", "vavoo_tv_uk", None), tutti.clone()]);
    assert_eq!(app.rig.grid.next_url(&tutti).as_deref(), Some("https://tvvoo.hayd.uk/catalog/tv/vavoo_tv_uk/genre=Tutti.json"));
    land(&mut app, &tutti, 5);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "detail", "a channel opens its streams page");
}

#[test]
fn an_install_without_catalogs_says_so_and_rests_on_the_bar() {
    let _g = sn_base::testlock::serial();
    let mut app = TestApp::boot(CatalogStore::from_addons(vec![addon("https://a.example/manifest.json", r#"{"id":"x","name":"X","catalogs":[]}"#)]));
    app.notify(StoreId::Catalog, 1);
    app.frames(3);
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    assert_eq!(app.top_name(), "discover");
    assert!(screen(&app).catalogs().is_empty());
    assert!(screen(&app).key().is_none(), "nothing is asked");
    assert_eq!(focus(&app), Some(Tab::Discover.elem()), "the remote rests on the bar");
}

/// The dropdown row scrolled back into view from the list stops under the top bar, where the
/// bar's pills do not cover it: a click where the row should stand hits its button. (The poster
/// grid homes the page as focus leaves it; a list scrolls row by row.)
#[test]
fn the_dropdown_row_revealed_from_below_sits_under_the_top_bar() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.rig.settings.set_list(crate::stores::settings::ViewPage::Catalog, true);
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    land(&mut app, &key("movie", "top", None), 60);
    for _ in 0..15 {
        app.press(Key::Down);
    }
    app.frames(30);
    for _ in 0..20 {
        if focus(&app).is_some_and(|e| screen(&app).bar().contains(&e)) {
            break;
        }
        app.press(Key::Up);
    }
    app.frames(30);
    let s = screen(&app);
    assert!(focus(&app).is_some_and(|e| s.bar().contains(&e)), "back on the row");
    let y = CONTENT_TOP - s.scroll();
    assert!(y >= sn_ui::widgets::TOP_BAR_BOTTOM, "the row is under the bar: {y}");
    let genre = app.d.last_stops().iter().rev().find(|st| st.key.elem == GENRE_ELEM).map(|st| st.rect).expect("drawn");
    assert!((genre.y - y).abs() < 1.0, "where the row stands: {genre:?}");
    app.take_effects();
    app.send(InputKind::Click { x: genre.cx(), y: genre.cy(), hit: None });
    app.frames(3);
    assert!(matches!(menu_of(&app), MenuKind::Choice { target: ChoiceTarget::DiscoverGenre, .. }), "the button is where the bar leaves it");
}

/// Discover's type and genre dropdowns stay on the page while their popover is up and after a pick
/// (the type's pick rebuilds the row under it: another catalog, other genres).
#[test]
fn the_dropdown_button_stays_drawn_while_open_and_after_a_pick() {
    let _g = sn_base::testlock::serial();
    let _l = language_on_this_thread_for_test(Preference::En);
    let mut app = boot();
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    assert_eq!(focus(&app), Some(TYPE_ELEM));
    crate::nav_tests::the_dropdown_button_stays_drawn(&mut app, "Movies", 1, "Series");
    app.press(Key::Right);
    app.press(Key::Right);
    assert_eq!(focus(&app), Some(GENRE_ELEM));
    crate::nav_tests::the_dropdown_button_stays_drawn(&mut app, "All genres", 1, "Action");
    app.press(Key::Left);
    app.press(Key::Left);
    // Anime's first catalog has no genres: the genre button goes, the type button stays
    crate::nav_tests::the_dropdown_button_stays_drawn(&mut app, "Series", 2, "Anime");
}

/// While the install list loads Discover says so (not "install an addon"), and when it failed it
/// says that, with a *Try again* DOWN from the bar reaches.
#[test]
fn discover_waits_for_the_addons_and_offers_a_retry_when_they_fail() {
    use crate::stores::catalog::{Job, Landing};
    use sn_platform::i18n::msg;
    let _g = sn_base::testlock::serial();
    let mut app = TestApp::boot(CatalogStore::new(["https://one.example/manifest.json"]));
    app.frames(3);
    app.press_raw(0, WCODE_GREEN);
    app.frames(3);
    assert_eq!(app.top_name(), "discover");
    let text = app.drawn_text().join(" ");
    assert!(text.contains(msg::app_home_loading_addons()), "{text}");
    assert!(!text.contains(msg::app_discover_none()), "no 'install an addon' while they load: {text}");
    for job in app.rig.catalog.take_jobs() {
        if let Job::Manifest { source, .. } = job {
            app.rig.catalog.land(Landing::Manifest { source, result: Err(sn_stremio::FetchError::Transport("down".into())) });
        }
    }
    app.notify(StoreId::Catalog, 2);
    app.frames(3);
    let text = app.drawn_text().join(" ");
    assert!(text.contains(msg::app_home_addons_failed()) && !text.contains(msg::app_discover_none()), "{text}");
    app.press(Key::Down);
    app.take_effects();
    app.press(Key::Ok);
    let asks = app.take_effects().into_iter().filter(|fx| *fx == AppFx::Catalog(crate::registry::CatalogCmd::RetryAddons)).count();
    assert_eq!(asks, 1, "Try again asks for the addons again");
}
