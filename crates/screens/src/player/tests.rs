//! The player page through the real dispatcher (`test_support::TestApp`): what each key and
//! pointer gesture asks of the playback, the menus the control row opens and what a pick sends,
//! Up Next, and the HUD's own strings fitting in every shipped language.

use sn_machine::machine::{InputKind, Key, MachineId, NavOp};
use sn_platform::i18n::msg;

use super::*;
use crate::registry::{MediaFacts, PlayClock, PlaybackView, PlayerPanel, TrackFacts};
use crate::stores::catalog::{CatalogStore, Job, Landing};
use crate::stores::detail::{DetailLanding, MetaKey};
use crate::test_support::TestApp;

const CINEMETA: &str = r#"{
    "id":"com.linvo.cinemeta","version":"3.0.13","name":"Cinemeta",
    "resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"],
    "catalogs":[]}"#;

fn boot() -> TestApp {
    let mut s = CatalogStore::new(vec!["https://a.example/cinemeta/manifest.json"]);
    for job in s.take_jobs() {
        if let Job::Manifest { source, .. } = job {
            s.land(Landing::Manifest { source, result: Ok(CINEMETA.as_bytes().to_vec()) });
        }
    }
    let mut app = TestApp::boot(s);
    app.frames(5);
    app
}

fn episode() -> PlayItem {
    PlayItem { kind: "series".into(), id: "tt9".into(), name: "Show".into(), video_id: "tt9:1:3".into(), episode: "S1 E3 Three".into(), release: None }
}

fn playing(pos_ms: u64) -> PlaybackView {
    let t = |lang: &str| TrackFacts { language: lang.into(), ..TrackFacts::default() };
    PlaybackView {
        phase: PlayPhase::Playing,
        clock: PlayClock { pos_ms, dur_ms: 3_000_000, seeking: false, ended: false, picture: true, buffer_pct: None, buffered_ms: 0 },
        media: MediaFacts { audio: vec![t("eng"), t("fre")], audio_sel: Some(0), subs: vec![t("eng"), t("ger")], addon_subs: Vec::new(), sub_sel: None, video: None, chapters: Vec::new() },
        ..PlaybackView::IDLE
    }
}

/// The player pushed over Home, playing `item` at `pos_ms`.
fn play(app: &mut TestApp, item: Option<PlayItem>, pos_ms: u64) {
    app.rig.playback = playing(pos_ms);
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::Player(PlayerArg { title: "Film".into(), item, addon: "AIOStreams".into(), resumed_ms: 0 })));
    app.frames(10);
    assert_eq!(app.top_name(), "player");
    app.take_effects();
}

fn player_cmds(app: &mut TestApp) -> Vec<PlayerCmd> {
    app.take_effects()
        .into_iter()
        .filter_map(|fx| match fx {
            AppFx::Player(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn player(app: &TestApp) -> &PlayerScreen {
    app.d.top_screen().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<PlayerScreen>()).expect("the player is the top page")
}

fn menu_kind(app: &TestApp) -> MenuKind {
    match app.d.nav.modals.surfaces.last().map(|s| s.entry.arg.clone()) {
        Some(AppArg::Menu(m)) => m.kind,
        other => panic!("a menu over the player: {other:?}"),
    }
}

/// How many frames the HUD's linger lasts at the rig's 60 Hz.
const LINGER_FRAMES: usize = (hud::LINGER_MS / 16) as usize + 2;

#[test]
fn ok_pauses_and_a_tap_of_right_seeks_ten_seconds_on() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::TogglePause]);
    app.press(Key::Right);
    app.frames(10);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Seek(610_000)], "one tap, one seek, after the burst closes");
}

#[test]
fn back_hides_the_hud_then_leaves_and_ends_the_playback() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press(Key::Back);
    assert_eq!(app.top_name(), "player", "the first BACK puts the transport away");
    assert!(player_cmds(&mut app).is_empty());
    app.press(Key::Back);
    assert_eq!(app.top_name(), "home");
    assert!(player_cmds(&mut app).contains(&PlayerCmd::Stop), "leaving ends the playback");
}

fn subtitle_menu(app: &TestApp) -> &subtitle_menu::SubtitleMenu {
    app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<subtitle_menu::SubtitleMenu>()).expect("the subtitle menu is up")
}

/// What the focused row of the subtitle menu is: a group on the left, or a variant on the right.
fn focused_row(app: &TestApp) -> (Option<subtitle_menu::Group>, Option<subtitle_menu::Variant>) {
    use sn_ui::form::{FormId, RowKey};
    let elem = app.d.focus().expect("focus").elem;
    let left = [subtitle_menu::Group::Off, subtitle_menu::Group::Embedded, subtitle_menu::Group::Lang("EN".into()), subtitle_menu::Group::Lang("DE".into())]
        .into_iter()
        .find(|g| g.key() == RowKey(elem));
    let right = (0..8).map(subtitle_menu::Variant::Embedded).chain((0..16).map(subtitle_menu::Variant::Addon)).find(|v| v.key() == RowKey(elem));
    (left, right)
}

/// The subtitle disc is a toggle: a short OK turns subtitles on or off, a held OK opens the two
/// columns on the file's own tracks: RIGHT crosses to them, DOWN walks them, OK shows one and
/// closes. GREEN closes the subtitle menu it would open, RED the audio menu, BACK either.
#[test]
fn the_subtitle_disc_opens_the_embedded_tracks_and_a_pick_reaches_the_playback() {
    use subtitle_menu::{Group, Variant};
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press(Key::Up);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::Disc(hud::DISC_SUBTITLES));
    assert!(!player(&app).disc_lit(hud::DISC_SUBTITLES, &app.rig.playback.media), "subtitles off: the disc is plain");
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "player", "a short press opens nothing");
    assert_eq!(player_cmds(&mut app), [PlayerCmd::ToggleSubtitles], "it turns subtitles on or off, as GREEN does");
    app.rig.playback.media.sub_sel = Some(0);
    app.frames(2);
    assert!(player(&app).disc_lit(hud::DISC_SUBTITLES, &app.rig.playback.media), "subtitles on: the disc wears its mark");
    app.rig.playback.media.sub_sel = None;
    app.frames(2);
    app.hold_ok();
    assert_eq!(app.top_name(), "menu");
    assert!(player_cmds(&mut app).is_empty(), "a hold toggles nothing");
    assert_eq!(menu_kind(&app), MenuKind::Subtitles);
    assert_eq!(*subtitle_menu(&app).shown(), Group::Embedded, "nothing in use, the file has tracks: the embedded group");
    assert_eq!(focused_row(&app).0, Some(Group::Embedded), "focus opens on it, in the left column");
    app.press(Key::Right);
    assert_eq!(focused_row(&app).1, Some(Variant::Embedded(0)));
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "player", "a pick closes the menu");
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Subtitle(Some(1))]);
    // Off, from the top of the left column
    app.hold_ok();
    app.press(Key::Up);
    assert_eq!(focused_row(&app).0, Some(Group::Off));
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Subtitle(None)]);
    // BACK closes with nothing asked
    app.hold_ok();
    app.press(Key::Back);
    assert_eq!(app.top_name(), "player");
    assert!(player_cmds(&mut app).is_empty());
    // GREEN, the disc's key, closes it too, toggling nothing
    app.hold_ok();
    assert_eq!(menu_kind(&app), MenuKind::Subtitles);
    assert!(player(&app).disc_lit(hud::DISC_SUBTITLES, &app.rig.playback.media), "the disc whose menu is open wears its mark");
    app.press_raw(0, crate::tabs::WCODE_GREEN);
    assert_eq!(app.top_name(), "player", "GREEN closes the subtitle menu");
    assert!(player_cmds(&mut app).is_empty());
    assert!(!player(&app).disc_lit(hud::DISC_SUBTITLES, &app.rig.playback.media));
    // the audio disc, beside it
    app.press(Key::Right);
    app.press(Key::Ok);
    assert!(matches!(menu_kind(&app), MenuKind::Choice { target: ChoiceTarget::PlayerAudio, current: 0, .. }));
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Audio(1)]);
    // RED opens the audio menu and RED closes it again
    app.press_raw(0, crate::tabs::WCODE_RED);
    assert!(matches!(menu_kind(&app), MenuKind::Choice { target: ChoiceTarget::PlayerAudio, .. }));
    assert!(player(&app).disc_lit(hud::DISC_AUDIO, &app.rig.playback.media));
    app.press_raw(0, crate::tabs::WCODE_RED);
    assert_eq!(app.top_name(), "player", "RED closes the audio menu it opened");
    assert!(player_cmds(&mut app).is_empty());
}

/// A language's column lists the file's track in it, then the addon's; a late answer landing
/// while the menu is open (sorting in ahead) moves neither the focus nor the pick, which goes by
/// the subtitle's key. Reopened, the menu stands on the language in use; LEFT comes back to the
/// groups, and the delay row at the bottom opens the delay steps.
#[test]
fn an_addon_subtitle_is_picked_by_key_and_the_delay_steps_in_an_open_menu() {
    use subtitle_menu::{Group, Variant};
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, None, 600_000);
    let sub = |language: &str, addon: &str, key: u32| crate::registry::AddonSubFacts { language: language.into(), addon: addon.into(), key, ..Default::default() };
    app.rig.playback.media.addon_subs = vec![sub("ger", "Subs", 7)];
    app.frames(2);
    app.press(Key::Up);
    app.hold_ok();
    app.press(Key::Down);
    assert_eq!(focused_row(&app).0, Some(Group::Lang("EN".into())), "the viewer's language first");
    app.press(Key::Down);
    assert_eq!(*subtitle_menu(&app).shown(), Group::Lang("DE".into()), "the right column follows the left");
    app.press(Key::Right);
    assert_eq!(focused_row(&app).1, Some(Variant::Embedded(1)), "the file's German first");
    app.press(Key::Down);
    assert_eq!(focused_row(&app).1, Some(Variant::Addon(7)));
    app.rig.playback.media.addon_subs = vec![sub("ger", "Late", 8), sub("ger", "Subs", 7)];
    app.frames(3);
    assert_eq!(focused_row(&app).1, Some(Variant::Addon(7)), "a late answer above does not move the focus");
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::AddonSubtitle(7)], "the pick is the key, not a place");
    // in use now: reopened on its language and checked
    app.rig.playback.media.sub_sel = Some(2 + 1);
    app.frames(2);
    app.hold_ok();
    assert_eq!(*subtitle_menu(&app).shown(), Group::Lang("DE".into()));
    app.press(Key::Right);
    assert_eq!(focused_row(&app).1, Some(Variant::Addon(7)), "across, on the subtitle in use");
    app.press(Key::Left);
    assert_eq!(focused_row(&app).0, Some(Group::Lang("DE".into())), "LEFT comes back to its group");
}

/// The subtitle menu's tabs: UP off the columns reaches the strip, RIGHT walks it (the tab follows
/// the cursor). Timing steps the delay a second or a quarter either way, and stays open; Style
/// steps the caption's look with LEFT/RIGHT — the Settings preferences, set at once, so the
/// caption on the picture changes under the menu.
#[test]
fn the_subtitle_menus_timing_and_style_tabs() {
    use crate::stores::settings::Pref;
    use subtitle_menu::Tab;
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press(Key::Up);
    app.hold_ok();
    for _ in 0..6 {
        app.press(Key::Up);
    }
    assert_eq!(subtitle_menu(&app).tab(), Tab::Tracks);
    app.press(Key::Right);
    assert_eq!(subtitle_menu(&app).tab(), Tab::Timing, "the tab follows the cursor");
    app.press(Key::Down);
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::SubtitleDelay(-1000)], "a second earlier");
    assert_eq!(app.top_name(), "menu", "the steps stay open");
    app.rig.playback.sub_delay_ms = -1000;
    app.frames(3);
    assert!(subtitle_menu(&app).opts().sections[0].header.ends_with("1.00 s"), "the header says the delay in force");
    app.press(Key::Down);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::SubtitleDelay(-750)], "a quarter later, from the delay in force");
    for _ in 0..6 {
        app.press(Key::Up);
    }
    app.press(Key::Right);
    assert_eq!(subtitle_menu(&app).tab(), Tab::Style);
    app.press(Key::Down);
    let size = app.rig.settings.get(Pref::SubtitleSize);
    app.take_effects();
    app.press(Key::Right);
    let effects = app.take_effects();
    assert!(effects.contains(&AppFx::Settings(SettingsCmd::Set(Pref::SubtitleSize, size + 1))), "RIGHT: one size up: {effects:?}");
    assert_eq!(app.top_name(), "menu");
}

/// The pointer: a hover on a language shows its subtitles, a click on one picks it.
#[test]
fn the_pointer_walks_the_columns_and_clicks_a_subtitle() {
    use subtitle_menu::{Group, Variant};
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press(Key::Up);
    app.hold_ok();
    use sn_ui::form::FormId;
    app.frames(30); // a popover in motion holds the pointer
    let stop = |app: &TestApp, elem: u32| app.d.last_stops().iter().rev().find(|s| s.key.elem == elem).map(|s| s.rect).expect("drawn");
    let de = stop(&app, Group::Lang("DE".into()).key().0);
    assert_eq!(subtitle_menu(&app).row_rect(Group::Lang("DE".into()).key().0), Some(de), "the stop is the row");
    app.send(InputKind::Pointer { x: de.cx(), y: de.cy(), hit: None });
    app.frames(2);
    assert_eq!(*subtitle_menu(&app).shown(), Group::Lang("DE".into()), "hovering a language shows it");
    let ger = stop(&app, Variant::Embedded(1).key().0);
    app.send(InputKind::Pointer { x: ger.cx(), y: ger.cy(), hit: None });
    app.send(InputKind::Click { x: ger.cx(), y: ger.cy(), hit: None });
    app.frames(60);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Subtitle(Some(1))]);
    assert_eq!(app.top_name(), "player", "the click closed the menu");
}

/// Subtitle size and position live in the subtitle menu's Style tab only: the `…` menu keeps
/// the show info, the aspect and the stats.
#[test]
fn the_more_menu_leaves_subtitle_styling_to_the_subtitle_menu() {
    assert_eq!(tracks::MORE_ROWS, [tracks::MoreRow::Info, tracks::MoreRow::Aspect, tracks::MoreRow::Stats]);
}

#[test]
fn up_next_offers_the_next_episode_and_opens_its_streams() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, Some(episode()), 600_000);
    // the page holds the show's meta open; the store answers it
    let key = MetaKey { kind: "series".into(), id: "tt9".into() };
    let addons = app.rig.catalog.addons().to_vec();
    app.rig.detail.open_meta(&addons, key.clone());
    let meta: sn_stremio::types::Meta = serde_json::from_str(
        r#"{"id":"tt9","type":"series","name":"Show","videos":[
            {"id":"tt9:1:3","season":1,"episode":3,"title":"Three"},
            {"id":"tt9:1:4","season":1,"episode":4,"title":"Four"}]}"#,
    )
    .unwrap();
    app.rig.detail.land(DetailLanding::Meta { key, result: Ok(meta) });
    app.notify(crate::stores::StoreId::Detail, 1);
    app.frames(LINGER_FRAMES);
    assert!(!player(&app).controller().visible(&Now { ms: app.now(), ..Now::default() }), "the HUD has gone");
    // the episode's last half-minute
    app.rig.playback = playing(3_000_000 - 20_000);
    app.frames(3);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::UpNext(hud::UP_NEXT_PLAY), "the offer raised the HUD on Next episode");
    app.press(Key::Ok);
    assert!(player_cmds(&mut app).contains(&PlayerCmd::Stop));
    let Some(AppArg::Streams(v)) = app.top_arg() else { panic!("the next episode's streams: {:?}", app.top_arg()) };
    assert_eq!((v.video_id.as_str(), v.item_id.as_str(), v.show.as_str()), ("tt9:1:4", "tt9", "Show"));
    assert_eq!(v.title, format!("{} Four", sn_ui::fmt::episode_ordinal(1, 4)));
}

#[test]
fn a_movie_that_plays_to_its_end_returns_where_it_was_played_from() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    let mut end = playing(3_000_000);
    end.clock.ended = true;
    app.rig.playback = end;
    app.frames(20);
    assert_eq!(app.top_name(), "home");
    assert!(player_cmds(&mut app).contains(&PlayerCmd::Stop));
}

#[test]
fn the_pointer_drags_the_bar_and_clicks_the_discs() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    let y = SB_Y + SB_H * 0.5;
    let x = |frac: f32| SB_X + SB_W * frac;
    app.send(InputKind::Click { x: x(0.5), y, hit: None });
    app.send(InputKind::Drag { x: x(0.25), y: y + 80.0, hit: None });
    assert_eq!(player(&app).controller().scrub.preview, Some(750_000), "the knob follows the hand, even off the bar");
    app.send(sn_ui::fixture::key_up(Key::Ok, sn_machine::machine::Tick::default()).kind);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Seek(750_000)], "the release commits");
    // hover moves the cursor, a click opens the disc's menu
    let audio = disc_rect(hud::DISC_AUDIO);
    app.send(InputKind::Pointer { x: audio.cx(), y: audio.cy(), hit: None });
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::Disc(hud::DISC_AUDIO));
    app.send(InputKind::Click { x: audio.cx(), y: audio.cy(), hit: None });
    app.frames(10);
    assert_eq!(app.top_name(), "menu");
}

/// The HUD's own strings: the Up Next pair leaves the title block room, and the undo chip and the
/// Up Next line are short enough for their places, in every shipped language.
#[test]
fn the_hud_strings_fit_in_every_shipped_language() {
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
    let m = sn_base::fontcov::advances::ShippedMeasure;
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        let [credits, next] = up_next_rects(&m);
        let tag = language.tag();
        assert!(credits.x - TITLE_GAP - SB_X >= SB_W * 0.5, "{tag}: the Up Next pair leaves the title half the bar");
        for (r, label) in [(credits, msg::app_player_watch_credits()), (next, msg::app_player_next_episode())] {
            assert!(m.width_str(label, UP_NEXT_SZ, true) < r.w, "{tag}: {label} fits its pill");
        }
        let chip = msg::app_player_undo("1:23:45");
        assert!(m.width_str(&chip, theme::size::MICRO, false) < 240.0, "{tag}: the undo chip is a chip");
        let up = msg::app_player_up_next();
        assert!(m.width_str(up, theme::size::CAPTION, false) < 200.0, "{tag}: Up next leaves the episode's name room");
    }
}

/// The `…` menu ends with Stats for nerds (OK toggles it, the menu closes so the read-out shows),
/// and its Aspect ratio row opens the modes, the current one checked, a pick sent to the playback.
#[test]
fn the_more_menu_toggles_stats_for_nerds_and_picks_the_aspect() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press(Key::Up);
    app.press(Key::Right);
    app.press(Key::Right);
    app.press(Key::Ok);
    let MenuKind::List { rows, target: ChoiceTarget::PlayerMore, .. } = menu_kind(&app) else { panic!("the … menu") };
    assert_eq!(rows.len(), tracks::MORE_ROWS.len());
    let last = rows.last().unwrap();
    assert_eq!((last.label.as_str(), last.value.as_str(), last.opens), (msg::app_player_stats(), msg::app_player_stats_off(), false));
    assert_eq!(*tracks::MORE_ROWS.last().unwrap(), tracks::MoreRow::Stats, "Stats for nerds is the last row");
    for _ in 1..rows.len() {
        app.press(Key::Down);
    }
    app.press(Key::Ok);
    app.frames(10);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::ToggleStats]);
    assert_eq!(app.top_name(), "player", "the pick closes the menu");
    // on: the row says so when the menu opens again
    app.rig.playback.stats = true;
    app.rig.playback.aspect = 2;
    app.press(Key::Ok);
    let MenuKind::List { rows, .. } = menu_kind(&app) else { panic!("the … menu") };
    assert_eq!(rows.last().unwrap().value, msg::app_player_stats_on());
    assert_eq!(rows[rows.len() - 2].value, msg::app_player_aspect_stretch());
    // the aspect row opens the modes
    for _ in 1..rows.len() - 1 {
        app.press(Key::Down);
    }
    app.press(Key::Ok);
    app.frames(10);
    let MenuKind::Choice { options, current, target, .. } = menu_kind(&app) else { panic!("the aspect modes") };
    assert_eq!((options.len(), current, target), (5, 2, ChoiceTarget::PlayerAspect));
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Aspect(3)]);
}

/// An episode with an intro at 1:00–2:00 and credits from 46:40 of its 50 minutes.
fn marked(pos_ms: u64) -> PlaybackView {
    use sn_stremio::skip::{Skips, Span};
    let span = |start_ms, end_ms| Some(Span { start_ms, end_ms });
    PlaybackView { skips: Skips { intro: span(60_000, 120_000), recap: None, credits: span(2_800_000, 3_000_000) }, ..playing(pos_ms) }
}

#[test]
fn the_skip_button_skips_the_intro_with_one_ok() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, Some(episode()), 10_000);
    app.rig.playback = marked(10_000);
    app.frames(LINGER_FRAMES);
    app.rig.playback = marked(65_000);
    app.frames(3);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::Skip, "over the hidden HUD it holds the cursor");
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::SkipTo(120_500)]);
    // Off: no button
    app.rig.settings.set(Pref::SkipIntros, crate::stores::settings::SKIP_OFF);
    app.rig.playback = marked(70_000);
    app.frames(LINGER_FRAMES);
    let n = Now { ms: app.now(), pos: 70_000, ..Now::default() };
    assert_eq!(player(&app).controller().skip_shown(&n), None);
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::TogglePause]);
}

#[test]
fn automatic_skips_the_intro_by_itself() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.rig.settings.set(Pref::SkipIntros, crate::stores::settings::SKIP_AUTO);
    play(&mut app, Some(episode()), 10_000);
    app.rig.playback = marked(61_000);
    app.frames(2);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::SkipTo(120_500)]);
    app.frames(5);
    assert!(player_cmds(&mut app).is_empty(), "once");
}

#[test]
fn up_next_is_offered_from_the_credits_marker() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, Some(episode()), 600_000);
    let key = MetaKey { kind: "series".into(), id: "tt9".into() };
    let addons = app.rig.catalog.addons().to_vec();
    app.rig.detail.open_meta(&addons, key.clone());
    let meta: sn_stremio::types::Meta = serde_json::from_str(
        r#"{"id":"tt9","type":"series","name":"Show","videos":[
            {"id":"tt9:1:3","season":1,"episode":3,"title":"Three"},
            {"id":"tt9:1:4","season":1,"episode":4,"title":"Four"}]}"#,
    )
    .unwrap();
    app.rig.detail.land(DetailLanding::Meta { key, result: Ok(meta) });
    app.notify(crate::stores::StoreId::Detail, 1);
    app.rig.playback = marked(2_799_000);
    app.frames(3);
    assert_ne!(player(&app).controller().hud.focus, hud::Focus::UpNext(hud::UP_NEXT_PLAY), "not before the credits");
    app.rig.playback = marked(2_800_000);
    app.frames(3);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::UpNext(hud::UP_NEXT_PLAY), "200 s before the end, at the credits");
}

/// Auto-play next episode off: Up Next is still offered, but an episode that plays to its end
/// returns to where it was played from instead of going on.
#[test]
fn with_auto_play_off_the_end_returns_and_up_next_waits_for_a_press() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.rig.settings.set(Pref::AutoPlay, 0);
    play(&mut app, Some(episode()), 600_000);
    land_show(&mut app);
    app.rig.playback = playing(3_000_000 - 20_000);
    app.frames(3);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::UpNext(hud::UP_NEXT_PLAY), "still offered");
    let mut end = playing(3_000_000);
    end.clock.ended = true;
    app.rig.playback = end;
    app.frames(20);
    assert_eq!(app.top_name(), "home", "back where it was played from: {:?}", app.top_arg());
    assert!(player_cmds(&mut app).contains(&PlayerCmd::Stop));
}

/// A fixed Up Next lead ignores the credits marker; the default follows it.
#[test]
fn a_fixed_up_next_lead_ignores_the_credits_marker() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.rig.settings.set(Pref::UpNextAt, 1); // 15 s before the end
    play(&mut app, Some(episode()), 600_000);
    land_show(&mut app);
    app.rig.playback = marked(2_900_000);
    app.frames(3);
    assert_ne!(player(&app).controller().hud.focus, hud::Focus::UpNext(hud::UP_NEXT_PLAY), "inside the credits, but 100 s left");
    app.rig.playback = marked(3_000_000 - 15_000);
    app.frames(3);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::UpNext(hud::UP_NEXT_PLAY), "15 s before the end");
}

/// The Skip button and the automatic skip's note are pills beside the title block, in every
/// shipped language.
#[test]
fn the_skip_strings_fit_in_every_shipped_language() {
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
    let m = sn_base::fontcov::advances::ShippedMeasure;
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        let tag = language.tag();
        for kind in [SegKind::Intro, SegKind::Recap] {
            for (label, icon) in [(skip_label(kind), false), (skipped_label(kind), true)] {
                let r = skip_rect(label, icon, &m);
                assert!(r.w <= SB_W * 0.3, "{tag}: {label} is a pill, not a banner ({})", r.w);
                assert!(m.width_str(label, UP_NEXT_SZ, true) < r.w, "{tag}: {label} fits its pill");
                assert!(r.y + r.h < UP_NEXT_CAPTION_Y, "{tag}: clear of the Up Next line");
            }
        }
    }
}

// ---- the next episode's same release ------------------------------------------------------------

const AIO: &str = r#"{"id":"aio","name":"AIOStreams","resources":["stream"],"types":["series"],"idPrefixes":["tt"]}"#;
const IDN: &str = "com.aiostreams.viren070|1080p|iDN_CreW";

fn aio_stream(name: &str, binge: &str, file: &str, url: &str) -> sn_stremio::types::Stream {
    serde_json::from_value(serde_json::json!({
        "url": url, "name": name, "description": "🎥 WEB-DL 🎞️ HEVC",
        "behaviorHints": { "bingeGroup": binge, "filename": file },
    }))
    .unwrap()
}

/// Episode 3 playing from AIOStreams' iDN_CreW 1080p release, the show's meta landed, at Up Next.
fn binge_episode(app: &mut TestApp) -> StreamKey {
    let release = sn_stremio::stream_facts::Release::of(
        "aio",
        &aio_stream("[TB⚡] Comet 1080p", IDN, "Breaking.Bad.1x03.1080p.H265.mkv", "https://example.com/e3.mkv"),
    );
    play(app, Some(PlayItem { release: Some(release), ..episode() }), 600_000);
    let key = MetaKey { kind: "series".into(), id: "tt9".into() };
    let addons = app.rig.catalog.addons().to_vec();
    app.rig.detail.open_meta(&addons, key.clone());
    let meta: sn_stremio::types::Meta = serde_json::from_str(
        r#"{"id":"tt9","type":"series","name":"Show","videos":[
            {"id":"tt9:1:3","season":1,"episode":3,"title":"Three"},
            {"id":"tt9:1:4","season":1,"episode":4,"title":"Four"}]}"#,
    )
    .unwrap();
    app.rig.detail.land(DetailLanding::Meta { key, result: Ok(meta) });
    app.notify(crate::stores::StoreId::Detail, 1);
    app.rig.playback = playing(3_000_000 - 20_000);
    app.frames(3);
    StreamKey { kind: "series".into(), video_id: "tt9:1:4".into() }
}

/// The application's half: the store opens the next episode's streams (the rig only records the
/// ask), `streams` landing as AIOStreams' answer unless `None` (still loading).
fn open_next(app: &mut TestApp, key: &StreamKey, streams: Option<Vec<sn_stremio::types::Stream>>) {
    let aio = sn_stremio::manifest::Addon::new("https://aio.example/u/manifest.json", sn_stremio::manifest::Manifest::parse(AIO.as_bytes()).unwrap());
    app.rig.detail.open_streams(&[aio], &Default::default(), key.clone());
    if let Some(streams) = streams {
        land_next(app, key, streams);
    }
}

fn land_next(app: &mut TestApp, key: &StreamKey, streams: Vec<sn_stremio::types::Stream>) {
    use crate::stores::detail::DetailLanding;
    app.rig.detail.land(DetailLanding::Streams { key: key.clone(), index: 0, state: sn_stremio::streams::GroupState::Ready(streams) });
    app.notify(crate::stores::StoreId::Detail, 2);
}

fn played(effects: &[AppFx]) -> Vec<&PlayRequest> {
    effects.iter().filter_map(|fx| if let AppFx::Play(r) = fx { Some(r) } else { None }).collect()
}

fn e4_streams() -> Vec<sn_stremio::types::Stream> {
    vec![
        aio_stream("[TB⏳] Comet 2160p", "com.aiostreams.viren070|2160p|WEB-DL|NIMA4K", "Breaking.Bad.S01E04.2160p.WEB-DL.mkv", "https://example.com/4k.mkv"),
        aio_stream("[TB⚡] Comet 1080p", IDN, "Breaking.Bad.1x04.1080p.H265.mkv", "https://example.com/e4.mkv"),
    ]
}

#[test]
fn the_next_episode_plays_the_same_release_straight_away() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    let key = binge_episode(&mut app);
    assert!(app.take_effects().contains(&AppFx::Detail(DetailCmd::OpenStreams(key.clone()))), "the offer asks for the next episode's streams");
    open_next(&mut app, &key, Some(e4_streams()));
    app.frames(2);
    assert!(app.drawn_text().iter().any(|t| t.ends_with(msg::app_settings_next_same())), "the Up Next line says the release was found");
    app.press(Key::Ok);
    let effects = app.take_effects();
    let plays = played(&effects);
    assert_eq!(plays.len(), 1, "{effects:?}");
    let item = plays[0].item.as_ref().unwrap();
    assert_eq!((plays[0].url.as_str(), plays[0].start_ms, item.video_id.as_str()), ("https://example.com/e4.mkv", 0, "tt9:1:4"));
    assert_eq!(item.release.as_ref().and_then(|r| r.binge_group.as_deref()), Some(IDN), "the episode after it keeps the release too");
    assert!(effects.contains(&AppFx::Player(PlayerCmd::Stop)));
    assert!(effects.contains(&AppFx::Detail(DetailCmd::CloseStreams(key))), "the page lets the streams go");
    assert!(matches!(app.top_arg(), Some(AppArg::Streams(v)) if v.video_id == "tt9:1:4"), "over the next episode's Streams page");
}

#[test]
fn no_same_release_opens_the_stream_list() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    let key = binge_episode(&mut app);
    open_next(&mut app, &key, Some(e4_streams()[..1].to_vec()));
    app.frames(2);
    app.press(Key::Ok);
    assert!(played(&app.take_effects()).is_empty(), "another release is the viewer's to pick");
    assert!(matches!(app.top_arg(), Some(AppArg::Streams(v)) if v.video_id == "tt9:1:4"));
}

#[test]
fn always_choose_opens_the_stream_list_without_asking() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    app.rig.settings.set(Pref::NextEpisode, crate::stores::settings::NEXT_CHOOSE);
    let key = binge_episode(&mut app);
    assert!(!app.take_effects().contains(&AppFx::Detail(DetailCmd::OpenStreams(key.clone()))), "the offer asks nothing");
    open_next(&mut app, &key, Some(e4_streams()));
    app.press(Key::Ok);
    assert!(played(&app.take_effects()).is_empty());
    assert!(matches!(app.top_arg(), Some(AppArg::Streams(v)) if v.video_id == "tt9:1:4"), "the Streams page opens its own streams");
}

/// The episode ended before its addon answered: the page waits for the answer, then plays it.
#[test]
fn a_pick_landing_after_the_end_still_plays() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    let key = binge_episode(&mut app);
    open_next(&mut app, &key, None);
    let mut end = playing(3_000_000);
    end.clock.ended = true;
    app.rig.playback = end;
    app.frames(20);
    assert_eq!(app.top_name(), "player", "waiting on the addon");
    assert!(played(&app.take_effects()).is_empty());
    land_next(&mut app, &key, e4_streams());
    app.frames(3);
    assert_eq!(played(&app.take_effects()).len(), 1);
    assert!(matches!(app.top_arg(), Some(AppArg::Streams(v)) if v.video_id == "tt9:1:4"));
}

#[test]
fn an_addon_that_never_answers_opens_the_stream_list() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    let key = binge_episode(&mut app);
    open_next(&mut app, &key, None);
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "player", "Next episode waits on the addon");
    app.frames((PICK_WAIT_MS / 16) as usize + 5);
    assert!(played(&app.take_effects()).is_empty());
    assert!(matches!(app.top_arg(), Some(AppArg::Streams(v)) if v.video_id == "tt9:1:4"), "the wait ran out");
}

// ---- the pause badge, the Episodes panel, the show info, the player's own keys ------------------

/// The show's meta, two seasons, landed in the store the page holds open.
fn land_show(app: &mut TestApp) {
    let key = MetaKey { kind: "series".into(), id: "tt9".into() };
    let addons = app.rig.catalog.addons().to_vec();
    app.rig.detail.open_meta(&addons, key.clone());
    let meta: sn_stremio::types::Meta = serde_json::from_str(
        r#"{"id":"tt9","type":"series","name":"Show","description":"A teacher turns.","runtime":"47 min",
            "cast":["A","B"],"director":["G"],"videos":[
            {"id":"tt9:1:3","season":1,"episode":3,"title":"Three","overview":"The third."},
            {"id":"tt9:1:4","season":1,"episode":4,"title":"Four"},
            {"id":"tt9:2:1","season":2,"episode":1,"title":"Again"}]}"#,
    )
    .unwrap();
    app.rig.detail.land(DetailLanding::Meta { key, result: Ok(meta) });
    app.notify(crate::stores::StoreId::Detail, 1);
    app.frames(3);
}

fn raw(app: &mut TestApp, sym: u32, wcode: u32, edge: sn_machine::machine::Edge) {
    app.send(InputKind::Key { key: Key::Other, sym, wcode, edge, at_edge: false });
}

fn episodes_panel(app: &TestApp) -> &episodes::EpisodesPanel {
    app.top_surface().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<episodes::EpisodesPanel>()).expect("the Episodes panel is up")
}

/// Paused, the centre badge says so and stays after the HUD has gone; it never pins the HUD.
#[test]
fn the_pause_badge_stays_while_paused_after_the_hud_hides() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.rig.playback.phase = PlayPhase::Paused;
    app.frames(LINGER_FRAMES);
    assert!(!player(&app).controller().visible(&Now { ms: app.now(), paused: true, ..Now::default() }), "the HUD went");
    let text = app.drawn_text();
    assert!(text.iter().any(|t| t == msg::app_player_paused()), "the badge says Paused: {text:?}");
    assert!(text.iter().any(|t| t == "10:00 / 50:00"), "and where: {text:?}");
    app.rig.playback.phase = PlayPhase::Playing;
    app.frames(60);
    assert!(!app.drawn_text().iter().any(|t| t == msg::app_player_paused()), "gone on resume");
}

/// The Episodes disc stays while the store lets the show's meta go (another page asking for it,
/// the next episode's prefetch): the page plays on its last ready copy.
#[test]
fn the_episodes_disc_survives_the_store_dropping_the_shows_meta() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, Some(episode()), 600_000);
    land_show(&mut app);
    app.frames(5);
    app.rig.detail = crate::stores::detail::DetailStore::new();
    let gen = app.rig.detail.gen();
    app.notify(crate::stores::StoreId::Detail, gen.wrapping_add(1));
    app.frames(5);
    app.press(Key::Up);
    app.press(Key::Left);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::Episodes, "the disc is still there");
    app.press(Key::Ok);
    assert!(matches!(menu_kind(&app), MenuKind::Player(PlayerPanel::Episodes { .. })), "and opens its panel");
}

/// An episode its show's meta gives no title (anime often has none) is still an episode: the
/// automatic next episode carried that empty title and lost the Episodes disc.
#[test]
fn an_untitled_episode_keeps_its_episodes_disc() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, Some(PlayItem { episode: String::new(), ..episode() }), 600_000);
    land_show(&mut app);
    app.press(Key::Up);
    app.press(Key::Left);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::Episodes);
}

/// The Episodes pill leads an episode's row; its panel opens on the season playing with the
/// episode marked, the season dropdown lists the other, and a pick plays it through Up Next's
/// road (no release to match: its Streams page).
#[test]
fn the_episodes_panel_opens_on_what_plays_and_a_pick_opens_that_episode() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, Some(episode()), 600_000);
    land_show(&mut app);
    app.press(Key::Up);
    app.press(Key::Left);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::Episodes);
    app.press(Key::Ok);
    assert!(matches!(menu_kind(&app), MenuKind::Player(PlayerPanel::Episodes { .. })));
    let panel = episodes_panel(&app);
    assert_eq!(panel.season(), Some(1));
    let rows = &panel.table().sections[0].rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].value.as_deref(), Some(msg::app_player_now_playing()), "the episode playing is marked");
    let elem = app.d.focus().expect("focus").elem;
    use sn_ui::form::FormId;
    assert_eq!(elem, episodes::Ep(0).key().0, "focused on it");
    // the season dropdown, above
    app.press(Key::Up);
    app.press(Key::Ok);
    assert!(matches!(menu_kind(&app), MenuKind::Choice { target: ChoiceTarget::PlayerEpisodeSeason, current: 0, .. }));
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(episodes_panel(&app).season(), Some(2), "the other season listed");
    // BACK closes and the film plays on
    app.press(Key::Back);
    assert_eq!(app.top_name(), "player");
    assert!(!player_cmds(&mut app).contains(&PlayerCmd::Stop));
    // YELLOW opens it too; OK on episode 4
    app.press_raw(0, crate::tabs::WCODE_YELLOW);
    assert_eq!(episodes_panel(&app).season(), Some(1));
    app.press(Key::Down);
    app.press(Key::Ok);
    assert!(player_cmds(&mut app).contains(&PlayerCmd::Stop));
    assert!(matches!(app.top_arg(), Some(AppArg::Streams(v)) if v.video_id == "tt9:1:4"), "its Streams page: {:?}", app.top_arg());
}

/// A pick in the panel plays the same release straight away, as Up Next would.
#[test]
fn an_episode_picked_in_the_panel_plays_the_same_release() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    let key = binge_episode(&mut app);
    app.take_effects();
    app.rig.playback = playing(600_000);
    app.frames(3);
    app.press_raw(0, crate::tabs::WCODE_YELLOW);
    app.press(Key::Down);
    app.press(Key::Ok);
    // its streams were asked for when Up Next was offered: the pick waits on that answer
    assert!(played(&app.take_effects()).is_empty());
    assert_eq!(app.top_name(), "player", "playing on while they come");
    open_next(&mut app, &key, Some(e4_streams()));
    app.frames(3);
    let effects = app.take_effects();
    let plays = played(&effects);
    assert_eq!(plays.len(), 1, "{effects:?}");
    assert_eq!(plays[0].url, "https://example.com/e4.mkv");
}

/// The `…` menu's Show info (and BLUE) opens what the meta says; OK closes it.
#[test]
fn show_info_tells_the_episode_from_its_meta() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, Some(episode()), 600_000);
    land_show(&mut app);
    app.press(Key::Up);
    app.press(Key::Right);
    app.press(Key::Right);
    app.press(Key::Ok);
    let MenuKind::List { rows, .. } = menu_kind(&app) else { panic!("the … menu") };
    assert_eq!(rows[0].label, msg::app_player_show_info());
    app.press(Key::Ok);
    app.frames(10);
    let MenuKind::Player(PlayerPanel::ShowInfo { tech, .. }) = menu_kind(&app) else { panic!("the show info") };
    assert_eq!(tech, "AIOStreams", "nothing known of the stream but its addon");
    let text = app.drawn_text();
    for want in ["Show", "The third.", "47 min"] {
        assert!(text.iter().any(|t| t.contains(want)), "{want}: {text:?}");
    }
    app.press(Key::Ok);
    assert_eq!(app.top_name(), "player");
    app.press_raw(0, crate::tabs::WCODE_BLUE);
    assert!(matches!(menu_kind(&app), MenuKind::Player(PlayerPanel::ShowInfo { .. })), "BLUE");
}

/// RED opens the audio menu; GREEN toggles subtitles with a note, and held opens their menu.
#[test]
fn the_colour_keys_open_the_players_menus_and_green_toggles_subtitles() {
    use sn_machine::machine::Edge;
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press_raw(0, crate::tabs::WCODE_RED);
    assert!(matches!(menu_kind(&app), MenuKind::Choice { target: ChoiceTarget::PlayerAudio, .. }));
    app.press(Key::Back);
    app.press_raw(0, crate::tabs::WCODE_GREEN);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::ToggleSubtitles]);
    app.rig.playback.media.sub_sel = Some(1);
    app.frames(3);
    assert_eq!(player(&app).sub_note(app.now()), Some("Subtitles on: Deutsch"));
    app.frames((SUB_NOTE_MS / 16) as usize + 5);
    assert_eq!(player(&app).sub_note(app.now()), None, "the note goes");
    // held: the subtitle menu, and no toggle
    raw(&mut app, 0, crate::tabs::WCODE_GREEN, Edge::Down);
    app.frames((GREEN_HOLD_MS / 16) as usize + 5);
    raw(&mut app, 0, crate::tabs::WCODE_GREEN, Edge::Up);
    app.frames(10);
    assert_eq!(menu_kind(&app), MenuKind::Subtitles);
    assert!(!player_cmds(&mut app).contains(&PlayerCmd::ToggleSubtitles));
    // YELLOW on a film is its show info
    app.press(Key::Back);
    app.press_raw(0, crate::tabs::WCODE_YELLOW);
    assert!(matches!(menu_kind(&app), MenuKind::Player(PlayerPanel::ShowInfo { .. })));
}

/// A number key seeks once, after the commit delay; ⏩ holds the picture and OK seeks and plays;
/// CH ▲ jumps ten minutes in a file without chapters.
#[test]
fn number_keys_fast_forward_and_ch_seek_from_the_remote() {
    use sn_ui::consts::WCODE_FASTFORWARD;
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press_raw(u32::from(b'3'), 0);
    app.press_raw(u32::from(b'5'), 0);
    app.frames(10);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Seek(1_500_000)], "only the last counts");
    app.press_raw(0, crate::tabs::WCODE_CH_UP);
    app.frames(10);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Seek(600_000 + hud::JUMP_MS)]);
    app.press_raw(0, WCODE_FASTFORWARD);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::SetPaused(true)], "the picture holds");
    app.rig.playback.phase = PlayPhase::Paused;
    app.press_raw(0, WCODE_FASTFORWARD);
    let text = app.drawn_text();
    assert!(text.iter().any(|t| t.starts_with('\u{d7}')), "the rate: {text:?}");
    app.press(Key::Ok);
    let cmds = player_cmds(&mut app);
    assert!(matches!(cmds.as_slice(), [PlayerCmd::Seek(t), PlayerCmd::SetPaused(false)] if *t > 600_000), "{cmds:?}");
}

/// The Episodes pill and the toggle's notes fit their places in every
/// shipped language.
#[test]
fn the_new_hud_strings_fit_in_every_shipped_language() {
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
    let m = sn_base::fontcov::advances::ShippedMeasure;
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        let tag = language.tag();
        let pill = episodes_rect(&m);
        assert!(pill.x - TITLE_GAP - SB_X >= SB_W * 0.4, "{tag}: the Episodes pill leaves the title room");
        for note in [msg::app_player_subtitles_on("Deutsch \u{b7} OpenSubtitles"), msg::app_player_subtitles_turned_off().to_string()] {
            assert!(skip_rect(&note, false, &m).w <= SB_W * 0.4, "{tag}: {note} is a pill");
        }
        assert!(m.width_str(msg::app_player_paused(), theme::size::TITLE, true) < SB_W * 0.3, "{tag}");
    }
}

/// A link that answered something other than a video (`ff::probe`'s sniff: an image from the
/// torrent, an archive, a text file, a service's web page) is named on the failure read-out —
/// the verdict and the reason under it are what the media layer published.
#[test]
fn the_failure_screen_says_the_link_is_not_a_video() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 0);
    app.rig.playback = PlaybackView {
        phase: PlayPhase::Failed,
        caption: msg::widgets_failure_not_video_image_c(),
        reason: msg::widgets_reason_wrong_file_c(),
        ..PlaybackView::IDLE
    };
    app.frames(5);
    // the reason wraps in its two-line slot: read the runs as one text
    let text = app.drawn_text().join(" ");
    for line in [msg::widgets_failure_not_video_image(), msg::widgets_reason_wrong_file()] {
        assert!(text.contains(line), "{line:?} is drawn: {text:?}");
    }
}

/// The not-a-video verdicts fit the read-out's one verdict line, and their reasons its two-line
/// slot, in every shipped language.
#[test]
fn the_not_a_video_read_outs_fit_in_every_shipped_language() {
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
    let m = sn_base::fontcov::advances::ShippedMeasure;
    let (verdict_sz, bold, _) = StatusOverlay::verdict_face(StatusKind::Failed);
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        let tag = language.tag();
        for caption in [
            msg::widgets_failure_not_video_image(),
            msg::widgets_failure_not_video_archive(),
            msg::widgets_failure_not_video_document(),
            msg::widgets_failure_web_page(),
        ] {
            assert!(m.width_str(caption, verdict_sz, bold) < SCR_W - 2.0 * MARGIN_X, "{tag}: {caption} fits the verdict line");
        }
        for reason in [msg::widgets_reason_wrong_file_c(), msg::widgets_reason_web_page_c()] {
            assert!(!StatusOverlay::failed_reason_truncates(reason, &m, 1.0), "{tag}: {reason:?} fits the reason slot");
        }
    }
}

/// A failed playback is no dead end: its read-out offers *Choose another stream* (focused; OK
/// returns to the stream list) and *Try again* (RIGHT, OK: the same stream again, from where it
/// stopped), and BACK still leaves.
#[test]
fn the_failure_read_out_offers_another_stream_or_a_retry() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    let failed = || PlaybackView { phase: PlayPhase::Failed, caption: msg::widgets_failure_not_video_image_c(), reason: c"", ..PlaybackView::IDLE };
    play(&mut app, None, 600_000);
    app.rig.playback = failed();
    app.frames(5);
    let text = app.drawn_text().join(" ");
    for label in [msg::app_player_choose_stream(), msg::app_player_try_again()] {
        assert!(text.contains(label), "{label:?} is offered: {text:?}");
    }
    app.press(Key::Right);
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Retry], "Try again asks for the same stream again");
    assert_eq!(app.top_name(), "player", "and stays to show it start");
    app.press(Key::Left);
    app.press(Key::Ok);
    app.frames(5);
    assert_eq!(app.top_name(), "home", "Choose another stream returns to where it was played from");
    assert!(player_cmds(&mut app).contains(&PlayerCmd::Stop));
    // BACK leaves too
    play(&mut app, None, 0);
    app.rig.playback = failed();
    app.frames(5);
    app.press(Key::Back);
    app.frames(5);
    assert_eq!(app.top_name(), "home");
}

/// The failure's two actions fit side by side in every shipped language.
#[test]
fn the_failure_actions_fit_in_every_shipped_language() {
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
    let m = sn_base::fontcov::advances::ShippedMeasure;
    for language in SHIPPED {
        let _g = language_on_this_thread_for_test(language);
        let o = StatusOverlay::new(Rect::FULL, msg::widgets_failure_not_video_image_c(), StatusKind::Failed)
            .page(Icon::Alert)
            .row(&[msg::app_player_choose_stream_c(), msg::app_player_try_again_c()]);
        let [a, b, ..] = o.row_frames_measured(&m);
        let (a, b) = (a.expect("the first pill"), b.expect("the second pill"));
        assert!(a.x >= MARGIN_X && b.x + b.w <= SCR_W - MARGIN_X && a.x + a.w <= b.x, "{}: {a:?} {b:?}", language.tag());
    }
}

/// The key that opened a panel closes it, as RED and GREEN do their menus: YELLOW the Episodes
/// panel (and a film's show info, which YELLOW opens there), BLUE the show info.
#[test]
fn yellow_closes_the_episodes_panel_and_blue_the_show_info() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, Some(episode()), 600_000);
    land_show(&mut app);
    app.press_raw(0, crate::tabs::WCODE_YELLOW);
    assert!(matches!(menu_kind(&app), MenuKind::Player(PlayerPanel::Episodes { .. })));
    app.press_raw(0, crate::tabs::WCODE_YELLOW);
    app.frames(10);
    assert_eq!(app.top_name(), "player", "YELLOW closes the Episodes panel");
    app.press_raw(0, crate::tabs::WCODE_BLUE);
    assert!(matches!(menu_kind(&app), MenuKind::Player(PlayerPanel::ShowInfo { .. })));
    app.press_raw(0, crate::tabs::WCODE_BLUE);
    app.frames(10);
    assert_eq!(app.top_name(), "player", "BLUE closes the show info");
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press_raw(0, crate::tabs::WCODE_YELLOW);
    assert!(matches!(menu_kind(&app), MenuKind::Player(PlayerPanel::ShowInfo { .. })), "a film's YELLOW is its show info");
    app.press_raw(0, crate::tabs::WCODE_YELLOW);
    app.frames(10);
    assert_eq!(app.top_name(), "player", "and YELLOW closes it too");
}

/// A playback that resumed a saved position offers *Start over* in the Skip slot for a few
/// seconds; over a hidden HUD it holds the cursor, and one OK seeks to the start.
#[test]
fn a_resumed_playback_offers_start_over_for_a_few_seconds() {
    let _g = sn_base::testlock::serial();
    let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
    let mut app = boot();
    app.rig.playback = playing(600_000);
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::Player(PlayerArg { title: "Film".into(), item: None, addon: String::new(), resumed_ms: 600_000 })));
    app.frames(10);
    assert!(app.drawn_text().iter().any(|t| t == msg::app_player_start_over()), "offered");
    app.frames(LINGER_FRAMES);
    assert_eq!(player(&app).controller().hud.focus, hud::Focus::Skip, "alone on the picture it holds the cursor");
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::Seek(0)]);
    assert!(!app.drawn_text().iter().any(|t| t == msg::app_player_start_over()), "taken, it goes");
    // a playback from the start offers nothing, and the offer ends on its own
    let mut app = boot();
    play(&mut app, None, 0);
    assert!(!app.drawn_text().iter().any(|t| t == msg::app_player_start_over()));
    let mut app = boot();
    app.rig.playback = playing(600_000);
    app.d.request(MachineId::Nav, NavOp::Push(AppArg::Player(PlayerArg { title: "Film".into(), item: None, addon: String::new(), resumed_ms: 600_000 })));
    app.frames((START_OVER_MS / 16) as usize + 20);
    assert!(!app.drawn_text().iter().any(|t| t == msg::app_player_start_over()), "gone after its time");
}

/// A stall with the picture up shows the centre buffering plate after a beat, not just the
/// clock's small spinner.
#[test]
fn a_mid_playback_stall_shows_the_buffering_plate_after_a_beat() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.frames(LINGER_FRAMES);
    app.rig.playback.phase = PlayPhase::Working;
    app.rig.playback.clock.seeking = true;
    app.rig.playback.caption = msg::widgets_status_buffering_c();
    app.frames(10);
    let plate = |app: &mut TestApp| app.drawn_text().iter().any(|t| t == msg::widgets_status_buffering());
    assert!(!plate(&mut app), "a short stall is the clock's spinner");
    app.frames((STALL_PLATE_MS / 16) as usize + 5);
    assert!(plate(&mut app), "past a beat, the plate says why the picture stopped");
    app.rig.playback.phase = PlayPhase::Playing;
    app.frames(3);
    assert!(!plate(&mut app));
}

/// The HUD fades rather than cutting: part-way after it is called up.
#[test]
fn the_hud_fades_in() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.frames(LINGER_FRAMES + 20);
    assert_eq!(player(&app).hud_alpha(), 0.0, "gone");
    app.tap(Key::Down);
    let a = player(&app).hud_alpha();
    assert!(a > 0.0 && a < 1.0, "fading in: {a}");
    app.frames(15);
    assert_eq!(player(&app).hud_alpha(), 1.0);
}

/// RED's audio menu ends in *Audio sync*: its steps move the sound a tenth of a second either way
/// (Reset back to none) and stay open, as the subtitle Timing does.
#[test]
fn the_audio_menu_steps_the_audio_sync() {
    let _g = sn_base::testlock::serial();
    let mut app = boot();
    play(&mut app, None, 600_000);
    app.press_raw(0, crate::tabs::WCODE_RED);
    for _ in 0..2 {
        app.press(Key::Down);
    }
    app.press(Key::Ok);
    assert!(matches!(menu_kind(&app), MenuKind::List { target: ChoiceTarget::PlayerAudioSync, .. }), "the sync's steps");
    app.take_effects();
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::AudioDelay(-100)], "a tenth earlier");
    app.rig.playback.audio_delay_ms = -100;
    app.frames(3);
    app.press(Key::Down);
    app.press(Key::Down);
    app.press(Key::Ok);
    assert_eq!(player_cmds(&mut app), [PlayerCmd::AudioDelay(0)], "Reset");
    assert_eq!(app.top_name(), "menu", "the steps stay open");
}
