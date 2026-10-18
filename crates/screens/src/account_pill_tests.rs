//! The top bar's account pill (`tabs::ACCOUNT_ELEM`) through the real dispatcher: where it sits in
//! the bar, what its menu offers signed in and signed out and where each row goes, the sign-out
//! confirmation, and the current-page mark it wears while Settings or the Addons list is up.

use sn_machine::machine::{Key, MachineId, NavOp};
use sn_platform::i18n::msg;

use crate::registry::{AppArg, AppFx, MenuKind};
use crate::stores::account::{Account, AccountCmd, AccountLanding, AccountStore, Secret, Stored};
use crate::stores::catalog::CatalogStore;
use crate::stores::StoreId;
use crate::tabs::{Tab, ACCOUNT_ELEM};
use crate::test_support::TestApp;

const CINEMETA: &str = r#"{
    "id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta",
    "resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"],
    "catalogs":[{"type":"movie","id":"top","name":"Popular","extra":[{"name":"search"},{"name":"skip"}]}]}"#;

/// Home over Cinemeta, its row landed.
fn boot() -> TestApp {
    use crate::stores::catalog::{Job, Landing};
    use sn_stremio::types::{CatalogPage, MetaPreview};
    let mut s = CatalogStore::new(["https://a.example/cinemeta/manifest.json"]);
    for job in s.take_jobs() {
        if let Job::Manifest { source, .. } = job {
            s.land(Landing::Manifest { source, result: Ok(CINEMETA.as_bytes().to_vec()) });
        }
    }
    s.set_view(0..2);
    for job in s.take_jobs() {
        if let Job::Page { row, .. } = job {
            let metas = (0..10).map(|i| MetaPreview { id: format!("tt{i}"), kind: "movie".into(), name: format!("Title {i}"), ..Default::default() }).collect();
            s.land(Landing::Page { row, result: Ok(CatalogPage { metas }) });
        }
    }
    let gen = s.gen();
    let mut app = TestApp::boot(s);
    app.notify(StoreId::Catalog, gen);
    app.frames(4);
    app
}

fn sign_in(app: &mut TestApp) {
    let mut account = AccountStore::new();
    let _ = account.land(
        AccountLanding::Restored(Stored::Session(Account { user_id: "u1".into(), email: "viewer@example.com".into(), auth_key: Secret::new("k") })),
        0,
    );
    app.rig.account = account;
    app.notify(StoreId::Account, 7);
    app.frames(3);
}

fn focus_elem(app: &TestApp) -> Option<u32> {
    app.d.focus().map(|k| k.elem)
}

/// From wherever focus is on the page, UP until the bar holds it, then LEFT/RIGHT onto `elem`.
fn to_pill(app: &mut TestApp, elem: u32) {
    for _ in 0..8 {
        if focus_elem(app).is_some_and(crate::tabs::on_strip) {
            break;
        }
        app.press(Key::Up);
    }
    assert!(focus_elem(app).is_some_and(crate::tabs::on_strip), "UP reaches the bar: {:?}", app.d.focus());
    for _ in 0..6 {
        if focus_elem(app) == Some(elem) {
            return;
        }
        app.press(Key::Left);
    }
    for _ in 0..6 {
        if focus_elem(app) == Some(elem) {
            return;
        }
        app.press(Key::Right);
    }
    panic!("never reached {elem:#x}: {:?}", app.d.focus());
}

fn menu(app: &TestApp) -> Option<&crate::menu::MenuScreen> {
    app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<crate::menu::MenuScreen>())
}

/// The open menu's rows: label and the line under it.
fn rows(app: &TestApp) -> Vec<(String, String)> {
    let m = menu(app).expect("the account menu is open");
    assert_eq!(m.kind(), &MenuKind::Account);
    m.table().sections.iter().flat_map(|s| s.rows.iter()).map(|r| (r.label.clone(), r.detail.clone())).collect()
}

/// The open menu's focused row label.
fn focused_row(app: &TestApp) -> String {
    let m = menu(app).expect("the menu is open");
    let i = m.table().sel as usize;
    m.table().sections.iter().flat_map(|s| s.rows.iter()).nth(i).map(|r| r.label.clone()).unwrap_or_default()
}

/// OK on the account pill, then DOWN `down` rows and OK.
fn pick(app: &mut TestApp, down: usize) {
    to_pill(app, ACCOUNT_ELEM);
    app.press(Key::Ok);
    app.frames(3);
    assert!(menu(app).is_some(), "OK on the account pill opens its menu");
    for _ in 0..down {
        app.press(Key::Down);
    }
    app.press(Key::Ok);
    app.frames(6);
}

/// The pill leads the bar, LEFT of Home; LEFT/RIGHT walk between them; the boot's focus is on
/// the page (never the pill), and UP from Home's content enters the bar on Home's pill.
#[test]
fn the_account_pill_leads_the_bar_and_the_boot_seat_is_not_on_it() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    assert_eq!(app.d.nav.tabs.strip.first().map(|m| m.elem), Some(ACCOUNT_ELEM), "the account pill is the bar's first stop");
    assert_ne!(focus_elem(&app), Some(ACCOUNT_ELEM), "the boot's focus is not on the account pill");
    assert_eq!(app.rig.chrome.selected(), Tab::Home);
    for _ in 0..8 {
        if focus_elem(&app).is_some_and(crate::tabs::on_strip) {
            break;
        }
        app.press(Key::Up);
    }
    assert_eq!(focus_elem(&app), Some(Tab::Home.elem()), "UP enters the bar on Home's pill, not the account pill");
    app.press(Key::Left);
    assert_eq!(focus_elem(&app), Some(ACCOUNT_ELEM), "LEFT from Home reaches the account pill");
    app.press(Key::Left);
    assert_eq!(focus_elem(&app), Some(ACCOUNT_ELEM), "the bar ends there");
    app.press(Key::Right);
    assert_eq!(focus_elem(&app), Some(Tab::Home.elem()), "RIGHT goes back to Home");
    assert_eq!(app.top_name(), "home", "walking onto the pill opens nothing");
}

/// Signed out the pill says *Sign in* and its menu offers Settings, Addons and Sign in; signed in
/// it is the account's initial and name, and the menu shows the address on the account line.
#[test]
fn the_menu_offers_settings_addons_and_the_account() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    assert_eq!(app.rig.chrome.account_text(), ("", msg::app_addons_sign_in()));
    to_pill(&mut app, ACCOUNT_ELEM);
    app.press(Key::Ok);
    app.frames(3);
    let labels: Vec<String> = rows(&app).into_iter().map(|r| r.0).collect();
    assert_eq!(labels, [msg::app_tab_settings(), msg::app_addons_title(), msg::app_addons_sign_in()]);
    app.press(Key::Back);
    app.frames(6);
    assert!(menu(&app).is_none(), "BACK closes the menu");
    assert_eq!(focus_elem(&app), Some(ACCOUNT_ELEM), "and focus returns to the pill");

    sign_in(&mut app);
    assert_eq!(app.rig.chrome.account_text(), ("V", "viewer"), "the initial and the email's local part");
    app.press(Key::Ok);
    app.frames(3);
    let got = rows(&app);
    assert_eq!(got.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), [
        msg::app_tab_settings(),
        msg::app_addons_title(),
        msg::app_addons_signed_in(),
        msg::app_addons_sign_out()
    ]);
    assert_eq!(got[2].1, "viewer@example.com", "the account line is the one place the address shows");
    assert_eq!(focused_row(&app), msg::app_tab_settings(), "the menu opens on its first row");
    // the account line reads: OK on it does nothing
    app.press(Key::Down);
    app.press(Key::Down);
    let _ = app.take_effects();
    app.press(Key::Ok);
    app.frames(3);
    assert!(menu(&app).is_some(), "OK on the account line keeps the menu open");
    assert!(app.take_effects().is_empty(), "and asks nothing");
}

/// Settings is no pill any more but its page is one menu row away (and its route still opens it);
/// the Addons list opens directly; on both the account pill is the current page's mark.
#[test]
fn settings_and_addons_open_from_the_menu_and_mark_the_account_pill() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    assert!(crate::tabs::TABS.iter().all(|e| e.tab != Tab::Settings), "Settings is no pill");
    pick(&mut app, 0);
    assert_eq!(app.top_name(), "settings", "the menu's Settings opens the page");
    assert_eq!(app.rig.chrome.selected(), Tab::Settings, "the account pill is the current page's mark");
    assert!(app.rig.chrome.shown(), "Settings wears the bar");
    for _ in 0..8 {
        if focus_elem(&app).is_some_and(crate::tabs::on_strip) {
            break;
        }
        app.press(Key::Up);
    }
    assert_eq!(focus_elem(&app), Some(ACCOUNT_ELEM), "UP from Settings enters the bar on the account pill");
    app.press(Key::Right);
    app.press(Key::Ok);
    app.frames(6);
    assert_eq!((app.top_name(), app.rig.chrome.selected()), ("home", Tab::Home));

    pick(&mut app, 1);
    assert_eq!(app.top_name(), "addons", "the menu's Addons opens the Addons list directly");
    assert_eq!(app.rig.chrome.selected(), Tab::Settings, "…on the account pill too");
    assert!(app.rig.chrome.shown(), "the Addons list wears the bar");
    app.press(Key::Back);
    app.frames(6);
    assert_eq!(app.top_name(), "home", "BACK returns to the page it opened over");

    // the Settings route (a deep link, the remote's hotkey) still opens the page
    app.d.request(MachineId::Nav, NavOp::SelectTab(AppArg::Settings));
    app.frames(6);
    assert_eq!((app.top_name(), app.rig.chrome.selected()), ("settings", Tab::Settings));
}

/// Signed out, the menu's Sign in opens the sign-in page.
#[test]
fn sign_in_opens_the_sign_in_page() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    pick(&mut app, 2);
    assert_eq!(app.top_name(), "signin");
}

/// Sign out asks first, in place, with focus on Cancel: Cancel changes nothing; only the
/// confirmation's own Sign out asks the account store to sign out.
#[test]
fn sign_out_is_confirmed_and_cancel_changes_nothing() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    sign_in(&mut app);
    let _ = app.take_effects();
    pick(&mut app, 3);
    let labels: Vec<String> = rows(&app).into_iter().map(|r| r.0).collect();
    assert_eq!(labels, [msg::app_avatar_cancel(), msg::app_addons_sign_out()], "the rows are the confirmation now");
    assert_eq!(focused_row(&app), msg::app_avatar_cancel(), "focus is on Cancel");
    assert!(app.take_effects().is_empty(), "nothing is asked before the answer");
    app.press(Key::Ok);
    app.frames(6);
    assert!(menu(&app).is_none(), "Cancel closes the menu");
    assert!(app.take_effects().is_empty(), "and changes nothing");
    assert!(app.rig.account.account().is_some(), "still signed in");

    pick(&mut app, 3);
    app.press(Key::Down);
    app.press(Key::Ok);
    app.frames(6);
    assert!(menu(&app).is_none());
    assert_eq!(app.take_effects(), [AppFx::Account(AccountCmd::SignOut)], "the confirmed Sign out signs out");
}

/// The account menu, its confirmation and the account pill fit in every shipped language.
#[test]
fn the_account_menu_fits_its_panel_in_every_shipped_language() {
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
    use sn_ui::form::FormTable;
    let mut out = Vec::new();
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        for (what, form) in [
            ("signed out", crate::menu::account_form(None, false)),
            ("signed in", crate::menu::account_form(Some("viewer@example.com"), false)),
            ("confirm", crate::menu::account_form(Some("viewer@example.com"), true)),
        ] {
            let mut t = FormTable::<crate::menu::MenuRow, crate::menu::MenuRow, std::convert::Infallible>::new(0x1000);
            t.table.compact = true;
            t.set(form, None);
            let tag = format!("{} account menu {what}", language.tag());
            out.extend(t.table.app_fit_failures_hugged(&tag));
            if let Some(why) = t.table.menu_cap_failure(&sn_base::fontcov::advances::ShippedMeasure, &tag) {
                out.push(why);
            }
        }
    }
    sn_ui::table::assert_no_fit_failures(&out);
}
