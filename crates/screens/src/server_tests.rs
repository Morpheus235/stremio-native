//! The streaming server page through the real dispatcher (`test_support::TestApp`): what each row
//! does to the settings store and the server store, what typing an address does with the
//! television keyboard, and that a bad address keeps the page and says so.

use sn_machine::machine::{Key, MachineId, NavOp};

use crate::registry::{AppArg, AppFx, ServerCmd, SettingsPage};
use crate::settings::ServerRow;
use crate::stores::catalog::CatalogStore;
use crate::stores::server::{Found, Scan, ServerJob, ServerLanding, ServerStatus};
use crate::stores::StoreId;
use crate::test_support::TestApp;
use sn_ui::form::FormId;

const FOUND_A: &str = "http://192.168.1.40:11470/";
const FOUND_B: &str = "http://192.168.1.41:11470/";

/// The page on top, over an empty catalog.
fn page() -> TestApp {
    let mut app = TestApp::boot(CatalogStore::new(Vec::<&str>::new()));
    app.frames(4);
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::SettingsPage(SettingsPage::StreamingServer)));
    app.frames(40);
    assert_eq!(app.top_name(), "settings-page");
    app
}

fn focus(app: &TestApp) -> Option<u32> {
    app.d.focus().map(|k| k.elem)
}

/// DOWN until `row` has focus.
fn go_to(app: &mut TestApp, row: ServerRow) {
    let want = row.key().0;
    for _ in 0..12 {
        if focus(app) == Some(want) {
            return;
        }
        app.press(Key::Down);
    }
    assert_eq!(focus(app), Some(want), "{row:?} is on the page");
}

/// Land the scan the page asked for, with `found`, and tell the tree.
fn land_scan(app: &mut TestApp, found: Vec<Found>) {
    let jobs = std::mem::take(&mut app.rig.server_jobs);
    let seq = jobs.iter().find_map(|j| if let ServerJob::Scan { seq } = j { Some(*seq) } else { None }).expect("a scan was asked for");
    app.rig.server.land(ServerLanding::Scanned { seq, found });
    app.notify(StoreId::Server, app.rig.server.gen());
    app.frames(4);
}

fn found() -> Vec<Found> {
    vec![Found { base: FOUND_A.into(), version: Some("4.20.8".into()) }, Found { base: FOUND_B.into(), version: None }]
}

#[test]
fn find_on_my_network_asks_for_a_scan_and_a_found_server_becomes_the_setting() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = page();
    assert_eq!(app.rig.settings.streaming_server(), None);
    go_to(&mut app, ServerRow::Find);
    app.take_effects();
    app.press(Key::Ok);
    assert!(app.take_effects().contains(&AppFx::Server(ServerCmd::Scan)));
    assert_eq!(app.rig.server.scan_state(), &Scan::Searching);
    land_scan(&mut app, found());
    assert_eq!(app.rig.server.found().len(), 2);
    go_to(&mut app, ServerRow::Found(1));
    app.press(Key::Ok);
    assert_eq!(app.rig.settings.streaming_server(), Some(FOUND_B), "the found server is set");
    assert_eq!(app.rig.server.status(), &ServerStatus::Checking, "…and checked at once");
    assert!(matches!(app.rig.server_jobs.last(), Some(ServerJob::Check { base, .. }) if base == FOUND_B));
    assert_eq!(app.top_name(), "settings-page", "the page stays");
}

#[test]
fn off_clears_the_setting() {
    let _g = sn_base::testlock::serial();
    let mut app = page();
    app.rig.settings.set_streaming_server(Some(FOUND_A.into()));
    app.notify(StoreId::Settings, app.rig.settings.gen());
    app.frames(4);
    go_to(&mut app, ServerRow::Off);
    app.press(Key::Ok);
    assert_eq!(app.rig.settings.streaming_server(), None);
    assert_eq!(app.rig.server.status(), &ServerStatus::Unknown, "nothing to check");
}

#[test]
fn a_typed_address_becomes_the_server_and_the_keyboard_goes_away() {
    let _g = sn_base::testlock::serial();
    let mut app = page();
    go_to(&mut app, ServerRow::Type);
    app.press(Key::Ok);
    assert_eq!(app.rig.keyboard.last(), Some(&true), "OK on the row raises the television keyboard");
    assert!(app.d.input.keyboard);
    app.type_text("192.168.1.9");
    app.press(Key::Ok);
    assert_eq!(app.rig.settings.streaming_server(), Some("http://192.168.1.9:11470/"), "no scheme and no port: both are added");
    assert_eq!(app.rig.keyboard.last(), Some(&false), "the keyboard goes away");
    assert!(!app.d.input.keyboard);
    assert!(matches!(app.rig.server_jobs.last(), Some(ServerJob::Check { .. })), "the new server is checked");
    assert_eq!(app.top_name(), "settings-page");
}

#[test]
fn an_invalid_address_toasts_and_keeps_the_page_and_the_text() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = page();
    go_to(&mut app, ServerRow::Type);
    app.press(Key::Ok);
    app.type_text("ftp://nope");
    app.take_effects();
    app.press(Key::Ok);
    let effects = app.take_effects();
    assert!(effects.contains(&AppFx::Toast(sn_platform::i18n::msg::app_settings_server_invalid().to_string())), "{effects:?}");
    assert_eq!(app.rig.settings.streaming_server(), None, "nothing is set");
    assert_eq!(app.top_name(), "settings-page", "the page stays");
    assert!(app.d.input.keyboard, "the keyboard stays up for another try");
    assert!(app.drawn_text().iter().any(|t| t.contains("ftp://nope")), "what was typed is still shown");
}

#[test]
fn back_while_typing_puts_the_keyboard_away_and_stays_on_the_page() {
    let _g = sn_base::testlock::serial();
    let mut app = page();
    go_to(&mut app, ServerRow::Type);
    app.press(Key::Ok);
    app.type_text("abc");
    app.press(Key::Back);
    assert_eq!(app.rig.keyboard.last(), Some(&false));
    assert_eq!(app.top_name(), "settings-page", "the first BACK ends typing, not the page");
    assert_eq!(app.rig.settings.streaming_server(), None);
    app.press(Key::Back);
    assert_ne!(app.top_name(), "settings-page", "the second leaves it");
}

#[test]
fn the_root_opens_the_page_from_its_streams_section() {
    let _g = sn_base::testlock::serial();
    let mut app = TestApp::boot(CatalogStore::new(Vec::<&str>::new()));
    app.frames(4);
    app.d.request(MachineId::Nav, NavOp::SelectTab(AppArg::Settings));
    app.frames(40);
    let row = crate::settings::RootRow::StreamingServer;
    let want = sn_ui::form::FormId::key(&row).0;
    for _ in 0..30 {
        if focus(&app) == Some(want) {
            break;
        }
        app.press(Key::Down);
    }
    assert_eq!(focus(&app), Some(want));
    app.press(Key::Ok);
    assert_eq!(app.top_arg(), Some(&AppArg::SettingsPage(SettingsPage::StreamingServer)));
}

#[test]
fn the_overview_asks_again_whether_the_server_answers() {
    let _g = sn_base::testlock::serial();
    let mut app = TestApp::boot(CatalogStore::new(Vec::<&str>::new()));
    app.rig.settings.set_streaming_server(Some(FOUND_A.into()));
    app.frames(4);
    app.rig.server_jobs.clear();
    app.d.request(MachineId::Nav, NavOp::SelectTab(AppArg::Settings));
    app.frames(4);
    assert!(
        matches!(app.rig.server_jobs.last(), Some(ServerJob::Check { base, .. }) if base == FOUND_A),
        "a 'Connected' from minutes ago is not shown as the server's state now"
    );
}
