//! **The loop** — one iteration per frame, in the frame algorithm's order (restructure spec §3.3):
//!
//! 1. ingest: the SDL queue, every event a reason to repaint; keys through the remote's hotkeys
//!    (`sn_screens::tabs::route_hotkey`) and then to the dispatcher, the television keyboard's text
//!    to the Search field, the lifecycle edges to the tree, the window and the playback;
//! 2. landings: worker outcomes folded into the stores, the artwork handed to the render cache;
//! 3. the playback pump, and the video plane's edge to the present gate;
//! 4. the frame clock to the bridge (the search debounce, the settings file), the stores' notices
//!    to the tree, and the top bar published for the page being shown;
//! 5. the dispatcher's frame (steps 1–9: the bus pump, input, the tick, the drain, the one nav
//!    commit), without its draw, then the bar's motion;
//! 6. the present decision — the gate's damage or motion, or queued uploads — and on the
//!    presenting side the draw (`Dispatcher::draw`, which runs the upload step first) and the
//!    swap; otherwise one idle poll's sleep, so a settled screen is not a CPU spinner.
//!
//! Nothing is presented while the app is backgrounded: the window is not ours then.

use std::time::Instant;

use sn_base::eventlog::log;
use sn_machine::machine::{InputEvent, MachineId, NavOp, Tick};
use sn_ui::dispatch::NoTap;

use super::input::{decode_key, edge_of, is_exit, key_input, rd_u32};
use super::{bridge, App};

const SDL_QUIT: u32 = 0x100;
const SDL_KEYDOWN: u32 = 0x300;
const SDL_KEYUP: u32 = 0x301;
const SDL_MOUSEMOTION: u32 = 0x400;
const SDL_MOUSEBUTTONDOWN: u32 = 0x401;
const SDL_MOUSEBUTTONUP: u32 = 0x402;
const SDL_MOUSEWHEEL: u32 = 0x403;

extern "C" {
    fn SDL_PollEvent(event: *mut std::os::raw::c_void) -> std::os::raw::c_int;
    fn SDL_GL_SwapWindow(win: *mut std::os::raw::c_void);
    fn SDL_Delay(ms: u32);
    fn glViewport(x: i32, y: i32, w: i32, h: i32);
}

/// Milliseconds since the loop started: the frame's tick and the playback clock.
///
/// The animation step `dt_us` is the MICROSECOND interval, not a difference of whole
/// milliseconds: at 60 fps the truncated form alternated 16 and 17 ms (a ±3% step jitter, ~3 px
/// a frame on a 3000 px/s scroll glide) for intervals that were all 16.7 ms.
struct Clock {
    start: Instant,
    last_us: u64,
}

impl Clock {
    fn tick(&mut self) -> Tick {
        self.tick_at(u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX))
    }

    fn tick_at(&mut self, us: u64) -> Tick {
        let ms = u32::try_from(us / 1000).unwrap_or(u32::MAX);
        let dt_us = u32::try_from(us.saturating_sub(self.last_us)).unwrap_or(u32::MAX).clamp(1, 100_000);
        self.last_us = us;
        Tick { ms, dt_us }
    }
}

/// The once-a-second frame heartbeat: `hb: loop=<iterations> fps=<presented> worst=<ms>
/// (logic=<ms> draw=<ms> swap=<ms>)`. `worst` is the slowest PRESENTED frame of the second, split
/// into the work before the draw (input, stores, the dispatcher's frame), the draw, and the swap;
/// a slow UI shows up here as the leg that dominates.
#[derive(Default)]
struct Heartbeat {
    since_ms: u32,
    loops: u32,
    presented: u32,
    worst: (u32, u32, u32),
    /// The worst frame's named sub-spans (`sn_base::diag::spans`), e.g. `page:31.2,prep:4.0`.
    worst_spans: String,
}

impl Heartbeat {
    fn frame(&mut self, now_ms: u32, presented: Option<((u32, u32, u32), String)>) -> Option<String> {
        self.loops += 1;
        if let Some((legs, spans)) = presented {
            self.presented += 1;
            if legs.0 + legs.1 + legs.2 > self.worst.0 + self.worst.1 + self.worst.2 {
                self.worst = legs;
                self.worst_spans = spans;
            }
        }
        if now_ms.wrapping_sub(self.since_ms) < 1000 {
            return None;
        }
        let (l, d, s) = self.worst;
        let mut line = format!(
            "hb: loop={} fps={} worst={}ms (logic={l} draw={d} swap={s})",
            self.loops, self.presented, l + d + s
        );
        if !self.worst_spans.is_empty() {
            line.push(' ');
            line.push_str(&self.worst_spans);
        }
        *self = Heartbeat { since_ms: now_ms, ..Default::default() };
        Some(line)
    }
}

fn ms_since(t: Instant) -> u32 {
    u32::try_from(t.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// The render instruments `sn_gfx` carries, armed once from their `/tmp/stremionative-*`
/// triggers (the `profile-tv` skill's GPU legs): `overdraw` (per-class submitted area, one
/// `OVERDRAW` line a second), `drawmask=<classes>`, `profile[=phase]` (GL timer queries) XOR
/// `hwcnt[=phase]` (Mali vinstr), `cpuprof`, and `heroground` (the detail ground in one pass,
/// `widgets::veiled_backdrop`). All of it is compiled out without `devtriggers`.
#[cfg(feature = "devtriggers")]
fn arm_instruments() {
    use sn_base::devtrig::{flag, read};
    if flag("overdraw") {
        sn_gfx::overdraw::set_ledger(true);
    }
    if let Some(spec) = read("drawmask") {
        sn_gfx::overdraw::set_mask(&spec);
    }
    match (read("profile"), read("hwcnt")) {
        (Some(_), Some(_)) => log("PROFILE disabled: remove either /tmp/stremionative-profile or /tmp/stremionative-hwcnt"),
        (Some(filter), None) => sn_ui::profile::set_enabled(&filter),
        (None, Some(filter)) => sn_ui::profile::set_hwcnt_enabled(&filter),
        (None, None) => {}
    }
    if flag("cpuprof") {
        sn_ui::profile::set_cpu_enabled();
    }
    if flag("heroground") {
        sn_ui::widgets::set_hero_ground(true);
        log("hero: one-pass ground ENABLED by /tmp/stremionative-heroground");
    }
}
#[cfg(not(feature = "devtriggers"))]
fn arm_instruments() {}

/// The loop. Returns when the viewer pressed EXIT or SDL quit.
///
/// # Safety
/// The thread that created the window and the GL context.
pub unsafe fn run(app: &mut App) {
    let watchdog = sn_base::task::watchdog::LoopWatch::start();
    let mut clock = Clock { start: Instant::now(), last_us: 0 };
    arm_instruments();
    let mut heartbeat = Heartbeat::default();
    #[cfg(feature = "devtriggers")]
    let mut devcmd_at = 0u32;
    // Per-frame sub-spans for the heartbeat's worst frame: two clock reads per span.
    sn_base::diag::spans::arm();
    while app.running {
        let frame_start = Instant::now();
        watchdog.advance();
        let _frame_scope = sn_base::task::FrameScope::enter();
        sn_platform::tv::home::poll();

        let tick = clock.tick();
        let mut inputs = ingest(app, tick);
        inputs.extend(app.pointer.tick(tick.ms));
        #[cfg(feature = "devtriggers")]
        if tick.ms.wrapping_sub(devcmd_at) >= 500 {
            devcmd_at = tick.ms;
            inputs.extend(super::devcmd::poll(app, tick));
        }
        if !app.running {
            break;
        }

        app.bridge.land();
        app.bridge.pump_playback(tick.ms, &mut app.pages.present);
        // Stats for nerds samples after the pump has republished, at 2 Hz while it is on
        app.diagnostics.update(&app.bridge.playback, tick.ms);
        app.bridge.tick(tick.ms);
        app.bridge.notify(&mut app.pages);
        app.bridge.capture_chrome(&mut app.pages);

        let dt = tick.dt_us as f32 / 1_000_000.0;
        sn_machine::idle::frame_begin(dt);
        let report = app.pages.frame_with(&mut app.bridge, tick, inputs, Vec::new(), &mut NoTap, false);
        app.pages.prune(&report.unmounted);
        app.bridge.update_chrome(&mut app.pages, &mut app.glass, dt);
        if report.presented {
            sn_machine::idle::invalidate();
        }
        // What the frame's commands asked for starts now, not a frame later.
        app.bridge.notify(&mut app.pages);

        sn_ui::tex::note_queued(&mut app.pages.budget);
        let present = !app.backgrounded
            && !sn_gfx::gfx::snapshot_pending()
            && (sn_machine::idle::should_present(tick.ms) || app.pages.budget.has_queued_work());
        let logic_ms = ms_since(frame_start);
        let mut legs = None;
        if present {
            let draw_start = Instant::now();
            sn_gfx::text::begin_frame();
            sn_gfx::gfx::field_frame_begin();
            let (vx, vy, vw, vh) = sn_base::surface::viewport();
            glViewport(vx, vy, vw, vh);
            {
                #[cfg(feature = "threadcheck")]
                let _draw_scope = sn_base::task::watchdog::draw_scope();
                let (pages, rig, glass) = (&mut app.pages, &mut app.bridge, &mut app.glass);
                let stats = &app.diagnostics;
                // One panic barrier around every screen's draw: a panic unwinding out of `sn_run`
                // (extern "C") would abort mid-playback. The glass plan is the top bar's material.
                // `frame.ui` is the GPU profilers' whole-frame phase; `profile.empty` its floor
                // (no GL between its boundaries). Both are free unless `arm_instruments` armed one.
                sn_ui::profile::phase("profile.empty", || {});
                sn_ui::profile::phase("frame.ui", || sn_ui::guard(|| {
                    pages.draw_with_glass_below(rig, glass, true, sn_ui::frame::backdrop::Z::ALL);
                    // last, over every page; skipped while a menu is up (it may carry the switch)
                    if super::diagnostics::visible(pages) {
                        stats.draw();
                    }
                }));
            }
            let draw_ms = ms_since(draw_start);
            let swap_start = Instant::now();
            {
                #[cfg(feature = "threadcheck")]
                let _present_scope = sn_base::task::watchdog::present_scope();
                SDL_GL_SwapWindow(app.win);
            }
            legs = Some(((logic_ms, draw_ms, ms_since(swap_start)), sn_base::diag::spans::take()));
            watchdog.presented();
            // per DRAWN frame, so a profiler's once-per-N-frames line paces off frames that ran
            sn_ui::profile::frame_end();
            sn_gfx::overdraw::frame_end();
            sn_gfx::gfx::blur_frame_end();
            sn_gfx::gfx::field_frame_end();
            sn_gfx::gfx::snapshot_frame_end();
            sn_gfx::gfx::ground_probes_frame_end();
            sn_machine::idle::note_present(tick.ms);
        } else {
            SDL_Delay(sn_machine::idle::IDLE_POLL_MS);
        }
        if let Some(line) = heartbeat.frame(tick.ms, legs) {
            log(&line);
        }
    }
}

/// Drain SDL's queue into this frame's inputs.
unsafe fn ingest(app: &mut App, tick: Tick) -> Vec<InputEvent<u32>> {
    let mut inputs = Vec::new();
    while SDL_PollEvent(app.ev.as_mut_ptr().cast()) != 0 {
        // ANY event is a reason to repaint: a key moves focus, a lifecycle edge changes the screen.
        sn_machine::idle::invalidate();
        let et = rd_u32(&app.ev, 0);
        if et == SDL_QUIT {
            log("LIFECYCLE: quit");
            app.running = false;
        } else if et == 0x103 || et == 0x104 {
            // WILL/DID ENTER BACKGROUND
            log(&format!("LIFECYCLE: background (playing={})", bridge::player_on_top(&app.pages) as i32));
            // The tree hears it: `Suspend` down every mounted body at the next commit.
            bridge::background(&mut app.pages);
            // The OS takes the plane with the screen: end the playback and leave its page, so the
            // foreground comes back to the list it was chosen from rather than a dead picture.
            app.bridge.playback.stop();
            if bridge::player_on_top(&app.pages) {
                app.pages.request(MachineId::Nav, NavOp::Pop);
            }
            // Revoke the borrowed Wayland proxies before another frame could use them.
            sn_platform::tv::window::release();
            app.backgrounded = true;
        } else if et == 0x105 || et == 0x106 {
            // WILL/DID ENTER FOREGROUND — both edges, in whichever order webOS sends them.
            log("LIFECYCLE: foreground");
            bridge::foreground(&mut app.pages);
            if et == 0x106 {
                // Re-acquire the window's surface on DID foreground, before the next frame draws.
                sn_platform::tv::window::grab(app.win);
                app.backgrounded = false;
            }
        } else if et == SDL_KEYDOWN || et == SDL_KEYUP {
            let (state, wcode, sym) = decode_key(&app.ev);
            if et == SDL_KEYDOWN {
                app.pointer.key();
            }
            if is_exit(sym, wcode, state) {
                log("input: EXIT");
                app.running = false;
                continue;
            }
            // the colour keys and the channel rocker switch tabs, outside text entry, menus and
            // the player (`docs/remote-keys.md`); a key they take never reaches a page
            if let sn_screens::tabs::HotkeyRoute::Went(tab) = sn_screens::tabs::route_hotkey(&mut app.pages, sym, wcode, edge_of(state)) {
                if edge_of(state) == sn_machine::machine::Edge::Down {
                    log(&format!("input: hotkey -> {tab:?}"));
                }
                continue;
            }
            if edge_of(state) == sn_machine::machine::Edge::Down {
                note_unmapped_key(sym, wcode);
            }
            inputs.push(key_input(sym, wcode, state, Tick { ms: tick.ms, dt_us: 0 }));
        } else if et == SDL_MOUSEMOTION {
            let (x, y) = pointer_xy(&app.ev);
            inputs.extend(app.pointer.motion(x, y, tick.ms));
        } else if et == SDL_MOUSEBUTTONDOWN {
            let (x, y) = pointer_xy(&app.ev);
            inputs.push(app.pointer.press(x, y, tick.ms));
        } else if et == SDL_MOUSEBUTTONUP {
            inputs.push(app.pointer.release(tick.ms));
        } else if et == SDL_MOUSEWHEEL {
            // SDL2's SDL_MouseWheelEvent: Sint32 y at +20 (positive away from the viewer).
            let dy = i32::from_ne_bytes([app.ev[20], app.ev[21], app.ev[22], app.ev[23]]);
            inputs.extend(app.pointer.wheel(dy, tick.ms));
        } else if et == crate::textinput::SDL_TEXTINPUT {
            // The television's keyboard committing text. Only Search and Sign in have fields; text
            // with any other page up is queued (and bounded) by `textinput`, never delivered.
            if bridge::text_field_on_top(&app.pages) {
                let text = crate::textinput::decode(&app.ev);
                let at = Tick { ms: tick.ms, dt_us: 0 };
                inputs.extend(super::input::text_inputs(&text, crate::textinput::available(), at));
            } else {
                crate::textinput::on_event(&app.ev);
            }
        }
    }
    inputs
}

/// An SDL mouse event's position (x at +20, y at +24) in the UI's logical pixels.
fn pointer_xy(ev: &[u8]) -> (f32, f32) {
    let rd = |o: usize| i32::from_ne_bytes([ev[o], ev[o + 1], ev[o + 2], ev[o + 3]]) as f32;
    sn_base::surface::to_logical(rd(20), rd(24))
}

/// Log a key the app has no meaning for, once per distinct (sym, wcode) pair per run, so a press
/// of an unmapped remote key (the channel rocker) names its codes in the event log.
fn note_unmapped_key(sym: u32, wcode: u32) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<(u32, u32)>>> = Mutex::new(None);
    if sn_ui::consts::classify(sym, wcode) != sn_ui::consts::Key::Other {
        return;
    }
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.get_or_insert_with(HashSet::new).insert((sym, wcode)) {
        log(&format!("input: unmapped key sym={sym} wcode={wcode}"));
    }
}

#[cfg(test)]
mod heartbeat_tests {
    use super::{Clock, Heartbeat};

    #[test]
    fn the_animation_step_is_the_microsecond_interval() {
        let mut clock = Clock { start: std::time::Instant::now(), last_us: 0 };
        // three 60 Hz frames: whole-ms differences would read 16, 17, 17
        let steps: Vec<u32> = [16_667u64, 33_333, 50_000].iter().map(|&us| clock.tick_at(us).dt_us).collect();
        assert_eq!(steps, [16_667, 16_666, 16_667]);
        let t = clock.tick_at(50_000 + 250_000);
        assert_eq!((t.ms, t.dt_us), (300, 100_000), "a stall is still clamped to 100 ms");
        assert_eq!(clock.tick_at(300_000).dt_us, 1, "a repeated instant is never a zero step");
    }

    #[test]
    fn one_line_a_second_with_the_slowest_presented_frame() {
        let mut hb = Heartbeat::default();
        assert_eq!(hb.frame(10, Some(((2, 5, 9), String::new()))), None);
        assert_eq!(hb.frame(26, None), None);
        assert_eq!(hb.frame(40, Some(((30, 12, 4), "spans=page:9.1".into()))), None);
        let line = hb.frame(1001, Some(((1, 1, 1), "spans=x:1.0".into()))).unwrap();
        assert_eq!(line, "hb: loop=4 fps=3 worst=46ms (logic=30 draw=12 swap=4) spans=page:9.1");
        assert_eq!(hb.frame(1500, None), None, "the window restarts at the line");
    }
}
