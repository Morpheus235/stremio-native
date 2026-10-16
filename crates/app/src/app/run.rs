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

extern "C" {
    fn SDL_PollEvent(event: *mut std::os::raw::c_void) -> std::os::raw::c_int;
    fn SDL_GL_SwapWindow(win: *mut std::os::raw::c_void);
    fn SDL_Delay(ms: u32);
    fn glViewport(x: i32, y: i32, w: i32, h: i32);
}

/// Milliseconds since the loop started: the frame's tick and the playback clock.
struct Clock {
    start: Instant,
    last_ms: u32,
}

impl Clock {
    fn tick(&mut self) -> Tick {
        let ms = u32::try_from(self.start.elapsed().as_millis()).unwrap_or(u32::MAX);
        let dt_us = ms.wrapping_sub(self.last_ms).saturating_mul(1000).clamp(1, 100_000);
        self.last_ms = ms;
        Tick { ms, dt_us }
    }
}

/// The loop. Returns when the viewer pressed EXIT or SDL quit.
///
/// # Safety
/// The thread that created the window and the GL context.
pub unsafe fn run(app: &mut App) {
    let watchdog = sn_base::task::watchdog::LoopWatch::start();
    let mut clock = Clock { start: Instant::now(), last_ms: 0 };
    while app.running {
        watchdog.advance();
        let _frame_scope = sn_base::task::FrameScope::enter();
        sn_platform::tv::home::poll();

        let tick = clock.tick();
        let inputs = ingest(app, tick);
        if !app.running {
            break;
        }

        app.bridge.land();
        app.bridge.pump_playback(tick.ms, &mut app.pages.present);
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
        if present {
            sn_gfx::text::begin_frame();
            sn_gfx::gfx::field_frame_begin();
            let (vx, vy, vw, vh) = sn_base::surface::viewport();
            glViewport(vx, vy, vw, vh);
            {
                #[cfg(feature = "threadcheck")]
                let _draw_scope = sn_base::task::watchdog::draw_scope();
                let (pages, rig, glass) = (&mut app.pages, &mut app.bridge, &mut app.glass);
                // One panic barrier around every screen's draw: a panic unwinding out of `sn_run`
                // (extern "C") would abort mid-playback. The glass plan is the top bar's material.
                sn_ui::guard(|| {
                    pages.draw_with_glass_below(rig, glass, true, sn_ui::frame::backdrop::Z::ALL);
                });
            }
            {
                #[cfg(feature = "threadcheck")]
                let _present_scope = sn_base::task::watchdog::present_scope();
                SDL_GL_SwapWindow(app.win);
            }
            watchdog.presented();
            sn_gfx::gfx::blur_frame_end();
            sn_gfx::gfx::field_frame_end();
            sn_gfx::gfx::snapshot_frame_end();
            sn_gfx::gfx::ground_probes_frame_end();
            sn_machine::idle::note_present(tick.ms);
        } else {
            SDL_Delay(sn_machine::idle::IDLE_POLL_MS);
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
            inputs.push(key_input(sym, wcode, state, Tick { ms: tick.ms, dt_us: 0 }));
        } else if et == crate::textinput::SDL_TEXTINPUT {
            // The television's keyboard committing text. Only the Search page has a field; text
            // with any other page up is queued (and bounded) by `textinput`, never delivered.
            if bridge::search_on_top(&app.pages) {
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
