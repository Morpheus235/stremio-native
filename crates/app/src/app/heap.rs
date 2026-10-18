//! **The C heap's policy** (glibc's malloc, which every Rust and native allocation goes through).
//!
//! glibc's defaults are tuned for a server, and on this 32-bit, 4-core set they hold memory the
//! process no longer uses:
//!
//! * up to 2 x cores = 8 thread arenas, each a chain of 1 MiB heaps (`HEAP_MAX_SIZE` on 32-bit).
//!   A thread arena only gives memory back from the TOP of its chain, so after the demuxer's FIFO
//!   of access units (tens of MiB, freed oldest-first) its heaps stay mapped and resident;
//! * a mmap threshold that RISES to the size of every mapped block freed (up to 512 KiB), which
//!   moves the 100-500 KiB access units of a high-bitrate stream out of `mmap` and into those
//!   arenas after the first few.
//!
//! A long session measured 283 such heaps on the set (216 MB resident, 45 MB more in swap) in a
//! 443 MB process. [`tune`] fixes both knobs before any thread exists, and [`trim_soon`] hands the
//! free pages of every arena back after a playback's teardown.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use sn_base::eventlog::log;

/// [`trim_soon`] is on unless a debug build's trigger said `trim=0` (the A/B leg).
static TRIM: AtomicBool = AtomicBool::new(true);

/// Thread arenas: the main thread's arena plus this many. Two keeps the demuxer and the poster
/// workers off the frame thread's arena lock without one arena per short-lived worker.
const ARENAS: i32 = 2;
/// Every block from 128 KiB up is its own mapping, returned to the system when freed: glibc's
/// own initial threshold, held fixed instead of rising.
const MMAP_THRESHOLD: i32 = 128 * 1024;

/// Set the policy. Call once, at boot, before the first thread is spawned. A debug build may
/// override it with `stremionative-malloc` (`arena=N mmap=BYTES trim=0|1`, 0 = glibc's default).
pub fn tune() {
    #[cfg(feature = "devtriggers")]
    let spec = sn_base::devtrig::read("malloc");
    #[cfg(not(feature = "devtriggers"))]
    let spec: Option<String> = None;
    let p = Policy::parse(spec.as_deref());
    TRIM.store(p.trim, Relaxed);
    let ok = sys::tune(p.arenas, p.mmap_threshold);
    log(&format!("heap: arenas={} mmap_threshold={} trim={} applied={ok}", p.arenas, p.mmap_threshold, p.trim));
}

/// The three knobs; 0 leaves a knob at glibc's default.
#[derive(Debug, PartialEq, Eq)]
struct Policy {
    arenas: i32,
    mmap_threshold: i32,
    trim: bool,
}

impl Policy {
    /// This module's defaults, with a trigger's `key=value` words over them.
    fn parse(spec: Option<&str>) -> Self {
        let mut p = Policy { arenas: ARENAS, mmap_threshold: MMAP_THRESHOLD, trim: true };
        for kv in spec.unwrap_or("").split_whitespace() {
            match kv.split_once('=').map(|(k, v)| (k, v.parse::<i32>())) {
                Some(("arena", Ok(v))) => p.arenas = v,
                Some(("mmap", Ok(v))) => p.mmap_threshold = v,
                Some(("trim", Ok(v))) => p.trim = v != 0,
                _ => log(&format!("heap: malloc trigger: ignored {kv:?}")),
            }
        }
        p
    }
}

/// Return the free pages of every arena to the system, off the frame thread (the trim takes each
/// arena's lock in turn). For after a playback's teardown, when tens of MiB were just freed.
pub fn trim_soon() {
    if TRIM.load(Relaxed) {
        spawn_trim();
    }
}

/// [`trim_soon`]'s worker, whatever the policy (`app::devcmd`'s `trim`).
pub fn spawn_trim() {
    sn_base::task::spawn_small("heap-trim", || {
        // the subtitle renderer and the stores retire on their own threads, just after the stop
        std::thread::sleep(std::time::Duration::from_secs(1));
        let before = sys::rss_kb();
        let released = sys::trim();
        log(&format!("heap: trimmed (released={released}) rss {before} -> {} KiB", sys::rss_kb()));
    });
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod sys {
    /// `mallopt`'s parameter numbers (glibc `malloc.h`).
    const M_MMAP_THRESHOLD: libc::c_int = -3;
    const M_ARENA_MAX: libc::c_int = -8;

    pub fn tune(arenas: i32, threshold: i32) -> bool {
        // SAFETY: mallopt only sets allocator parameters; 0 leaves a knob at glibc's default.
        unsafe {
            (arenas <= 0 || libc::mallopt(M_ARENA_MAX, arenas) == 1)
                && (threshold <= 0 || libc::mallopt(M_MMAP_THRESHOLD, threshold) == 1)
        }
    }

    pub fn trim() -> bool {
        // SAFETY: malloc_trim walks the arenas under their own locks.
        unsafe { libc::malloc_trim(0) == 1 }
    }

    pub fn rss_kb() -> u64 {
        std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
            .map_or(0, |pages| pages * 4)
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
mod sys {
    pub fn tune(_arenas: i32, _threshold: i32) -> bool {
        false
    }
    pub fn trim() -> bool {
        false
    }
    pub fn rss_kb() -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trigger_overrides_only_the_knobs_it_names() {
        assert_eq!(Policy::parse(None), Policy { arenas: ARENAS, mmap_threshold: MMAP_THRESHOLD, trim: true });
        assert_eq!(
            Policy::parse(Some("arena=0 mmap=0 trim=0\n")),
            Policy { arenas: 0, mmap_threshold: 0, trim: false },
            "the A/B leg: glibc's defaults, no trim"
        );
        assert_eq!(Policy::parse(Some("mmap=65536 bogus arena=x")).mmap_threshold, 65536);
        assert_eq!(Policy::parse(Some("arena=x")).arenas, ARENAS, "an unreadable value keeps the default");
    }
}
