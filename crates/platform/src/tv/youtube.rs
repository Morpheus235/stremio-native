//! Opening a YouTube video in the television's OWN YouTube app, as everything outside the port
//! sees it. The app has no YouTube playback; a trailer is handed to `youtube.leanback.v4`. The
//! bus sequence is the port's (`webos::youtube`); this is the vocabulary it answers in and the
//! one door callers go through.
//!
//! **Blocking.** The sequence makes a few LS2 round trips (each waits up to three seconds), so
//! [`open_trailer`] must run on a worker, never on the frame thread; it asserts that itself
//! (`sn_base::task::assert_may_block`). One open at a time: a press while one is under way is
//! [`Outcome::Busy`] and does nothing.
//!
//! **The app does not outlive a cold start (webOS 3.9.3).** When YouTube is not running, its first
//! launch only opens its menu, the video needs the same launch sent again once YouTube is in
//! front, and webOS closes this app about 0.3 s after the first launch (an `SDL_QUIT`, measured
//! 2026-11-01). So the second launch comes from a small detached helper process the port starts
//! first (`webos::trailer_helper`); [`Outcome::Opened`] means the first launch was accepted,
//! nothing more is known to the app. The viewer returns to the app from the home launcher. This
//! was measured on webOS 3 only: any other version gets the single launch (`webos::youtube`).
//!
//! **Without a platform bus** (the simulator, a host test, or no port installed) nothing here
//! touches one: a log line and [`Outcome::NoBus`].

use std::sync::atomic::{AtomicBool, Ordering};

/// How an attempt ended.
#[derive(Debug, PartialEq, Eq)]
// The simulator has no bus, so only `NoBus` and `Busy` are ever constructed there.
#[cfg_attr(feature = "hostsim", allow(dead_code))]
pub enum Outcome {
    /// The launch was accepted. When YouTube had to be started first, its detached helper sends
    /// the second launch (or was never started: logged, and YouTube then stays on its menu).
    Opened,
    /// The application manager answered and said no (or the id was not a YouTube id); its own
    /// `errorText` travels with it. YouTube did not open, so the app stays and shows the toast.
    Refused { error_text: String },
    /// The bus never carried a call: the failing stage and the hub's words.
    Bus { detail: String },
    /// Another open is still running.
    Busy,
    /// No platform bus: the simulator, a host test, or no port.
    NoBus,
}

impl Outcome {
    /// The viewer is told (a toast): the FIRST launch was refused or the bus failed before YouTube
    /// opened. Not `Opened`, not `Busy`, not `NoBus`.
    pub fn failed(&self) -> bool {
        matches!(self, Outcome::Refused { .. } | Outcome::Bus { .. })
    }
}

static OPENING: AtomicBool = AtomicBool::new(false);

/// Releases [`OPENING`] when the attempt ends, however it ends.
struct Opening;

impl Drop for Opening {
    fn drop(&mut self) {
        OPENING.store(false, Ordering::Release);
    }
}

/// Open YouTube on the video `id` (a well-formed YouTube id; the port refuses any other).
///
/// Blocks for a few LS2 round trips: never call it from the frame thread.
pub fn open_trailer(id: &str) -> Outcome {
    let _block = sn_base::task::assert_may_block(const { &sn_base::task::BlockingLabel::new("LS2 YouTube open") });
    if OPENING.swap(true, Ordering::AcqRel) {
        sn_base::eventlog::log(&format!("youtube: an open is already under way - ignoring {id}"));
        return Outcome::Busy;
    }
    let _opening = Opening;
    (super::port().open_trailer)(id)
}

/// What an absent port answers: nothing was sent, and the log says so.
pub(super) fn open_without_port(id: &str) -> Outcome {
    sn_base::eventlog::log(&format!("youtube: no platform port - the trailer {id} was not opened"));
    Outcome::NoBus
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_device_no_bus_is_touched() {
        let _g = sn_base::testlock::serial();
        assert_eq!(open_trailer("PLl99DlL6b4"), Outcome::NoBus);
        assert!(!OPENING.load(Ordering::Acquire), "the claim is handed back");
    }

    #[test]
    fn a_second_open_while_one_runs_is_busy_and_changes_nothing() {
        let _g = sn_base::testlock::serial();
        OPENING.store(true, Ordering::Release);
        assert_eq!(open_trailer("PLl99DlL6b4"), Outcome::Busy);
        assert!(OPENING.load(Ordering::Acquire), "the running open keeps its claim");
        OPENING.store(false, Ordering::Release);
    }

    #[test]
    fn only_a_refusal_or_a_bus_failure_is_shown() {
        assert!(Outcome::Refused { error_text: "x".into() }.failed());
        assert!(Outcome::Bus { detail: "x".into() }.failed());
        for quiet in [Outcome::Opened, Outcome::Busy, Outcome::NoBus] {
            assert!(!quiet.failed(), "{quiet:?}");
        }
    }
}
