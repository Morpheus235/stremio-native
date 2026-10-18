//! **`stremionative-memcmd`**: a debug build's remote control for memory profiling on the set.
//!
//! The trigger file holds whitespace-separated commands; it is read and removed twice a second
//! and its commands run in order on the main thread:
//!
//! * `mem` — one `MEM` line in the event log ([`mem_line`]): the kernel's view of the process
//!   (`/proc/self/status`), glibc's heap (`mallinfo`) and the app's own accounted buffers;
//! * `trim` — hand the heap's free pages back now (`app::heap`);
//! * `play` / `stop` — start the `stremionative-playurl` stream / end the playback and leave the
//!   player, as BACK would;
//! * `sub=<i>` — show the file's `i`-th subtitle track (`sub=off`: none);
//! * `up` `down` `left` `right` `ok` `back` — one synthetic press of that key.
//!
//! It exists because measuring "after N playbacks" needs N playbacks in ONE process, with no
//! remote in the loop. Compiled out without `devtriggers`.

use sn_base::eventlog::log;
use sn_machine::machine::{InputEvent, MachineId, NavOp, Tick};
use sn_ui::consts::{SDLK_DOWN, SDLK_ESCAPE, SDLK_LEFT, SDLK_RETURN, SDLK_RIGHT, SDLK_UP};

use super::input::key_input;
use super::{bridge, App};

const NAME: &str = "memcmd";

/// Read, remove and run the trigger's commands; the synthetic key presses come back as inputs.
pub fn poll(app: &mut App, tick: Tick) -> Vec<InputEvent<u32>> {
    let mut inputs = Vec::new();
    let Some(text) = sn_base::devtrig::read(NAME) else { return inputs };
    let _ = std::fs::remove_file(sn_base::devtrig::path(NAME));
    for cmd in text.split_whitespace() {
        let sym = match cmd {
            "mem" => {
                log(&mem_line(app));
                continue;
            }
            "stop" => {
                app.bridge.playback.stop();
                if bridge::player_on_top(&app.pages) {
                    app.pages.request(MachineId::Nav, NavOp::Pop);
                }
                log("memcmd: stop");
                continue;
            }
            "trim" => {
                super::heap::spawn_trim();
                continue;
            }
            "play" => {
                play(app);
                continue;
            }
            sub if sub.starts_with("sub=") => {
                let pick = sub[4..].parse::<usize>().ok();
                app.bridge.playback.command(sn_screens::registry::PlayerCmd::Subtitle(pick));
                log(&format!("memcmd: subtitle {pick:?}"));
                continue;
            }
            "up" => SDLK_UP,
            "down" => SDLK_DOWN,
            "left" => SDLK_LEFT,
            "right" => SDLK_RIGHT,
            "ok" => SDLK_RETURN,
            "back" => SDLK_ESCAPE,
            other => {
                log(&format!("memcmd: unknown command {other:?}"));
                continue;
            }
        };
        // SDL's key state: 1 = pressed, 0 = released (`input::edge_of`)
        inputs.push(key_input(sym, 0, 1, tick));
        inputs.push(key_input(sym, 0, 0, tick));
    }
    inputs
}

fn play(app: &mut App) {
    match sn_media::player::playurl::playurl().map(|p| p.and_then(|p| p.to_start().map_err(|e| format!("{e:?}")))) {
        Some(Ok(start)) => {
            app.bridge.playback.start_dev(start);
            if !bridge::player_on_top(&app.pages) {
                let arg = sn_screens::registry::PlayerArg { title: "playurl".into(), item: None, addon: String::new(), resumed_ms: 0 };
                app.pages.request(MachineId::Nav, NavOp::Push(sn_screens::registry::AppArg::Player(arg)));
            }
            log("memcmd: play");
        }
        Some(Err(e)) => log(&format!("memcmd: playurl unreadable ({e})")),
        None => log("memcmd: no playurl trigger"),
    }
}

/// `MEM rss=… anon=… file=… shmem=… swap=… hwm=… thr=… | heap arena=… mmap=… used=… free=… |
/// tex=… aq=…/… sub=… | gpu=… tex_ledger=n/… <size>x<count>…` — kilobytes
/// throughout.
pub fn mem_line(app: &mut App) -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |k: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix(':')))
            .and_then(|v| v.split_whitespace().next())
            .unwrap_or("?")
            .to_string()
    };
    let kb = |b: usize| b / 1024;
    let heap = heap_kb();
    let p = sn_media::player::memory(&mut app.bridge.playback.adapter);
    format!(
        "MEM rss={} anon={} file={} shmem={} swap={} hwm={} thr={} | heap {heap} | tex={} aq={}/{} sub={} playing={}",
        field("VmRSS"),
        field("RssAnon"),
        field("RssFile"),
        field("RssShmem"),
        field("VmSwap"),
        field("VmHWM"),
        field("Threads"),
        kb(sn_ui::tex::resident_bytes()),
        kb(p.video_queue),
        kb(p.audio_queue),
        kb(p.sub_bitmaps),
        bridge::player_on_top(&app.pages) as u8,
    ) + &gpu_kb()
}

/// The driver's count of this process's GPU pages (`/proc/gpu/<pid>`, LG's Mali kernel; absent
/// elsewhere) and the app's own texture ledger, grouped by size.
fn gpu_kb() -> String {
    let pages = std::fs::read_to_string(format!("/proc/gpu/{}", std::process::id()))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    let (n, bytes) = sn_gfx::gfx::tex_ledger::totals();
    let mut s = format!(" | gpu={} tex_ledger={n}/{}", pages.map_or("?".into(), |p| (p * 4).to_string()), bytes / 1024);
    for (b, count) in sn_gfx::gfx::tex_ledger::by_size().into_iter().take(8) {
        s += &format!(" {}Kx{count}", b / 1024);
    }
    s
}

/// glibc's own accounting: the main arena's size, the mmap'd chunks, in use and free.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn heap_kb() -> String {
    // SAFETY: mallinfo reads allocator statistics; no pointer is passed.
    #[allow(deprecated)]
    let m = unsafe { libc::mallinfo() };
    let kb = |v: libc::c_int| (v as u32) / 1024;
    format!("arena={} mmap={} used={} free={}", kb(m.arena), kb(m.hblkhd), kb(m.uordblks), kb(m.fordblks))
}
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn heap_kb() -> String {
    "n/a".into()
}
