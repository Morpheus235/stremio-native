//! Open a trailer in the television's own YouTube app: the port's half of
//! [`crate::tv::youtube`]. The app cannot play YouTube, so it hands the video id to
//! `youtube.leanback.v4` through the application manager.
//!
//! # What was measured (webOS 3.9.3, 2026-11-01)
//!
//! The YouTube app's appinfo declares `deeplinkingParams: {"handledBy":"app","contentTarget":"v=$CONTENTID"}`.
//!
//! - The launch that plays a video is exactly
//!   `{"id":"youtube.leanback.v4","params":{"handledBy":"app","contentTarget":"v=<ID>"}}`. Without
//!   `handledBy`, or with a full URL as the target, YouTube ignores the video.
//! - YouTube already RUNNING: that one launch plays the video.
//! - YouTube NOT running (the usual case on a television short of memory, where the system closes it): that launch only opens
//!   its main menu (or its profile picker). The same launch sent AGAIN once YouTube is in front
//!   plays the video (after a profile pick, if YouTube shows the picker). Timing: YouTube reached
//!   the foreground about 5.5 s after the first launch, and a second send 10 s after the first
//!   worked. Proven by hand with `luna-send`.
//! - **The app does not live to send that second launch.** About 0.3 s after the first launch
//!   webOS sends the app an `SDL_QUIT` (`LIFECYCLE: quit`), and the process exits cleanly with
//!   rc 0 (no signal); the worker thread dies with it. The device log of 2026-11-01 reads
//!   `youtube: opening trailer ...`, `youtube: running=false -> ...`, `LIFECYCLE: quit`, `shutdown`.
//!   So the second launch cannot come from a thread of the app.
//!
//! # Which webOS versions get the second launch
//!
//! Everything above was measured on webOS 3.9.3 only. On a newer webOS (the upstream author's own
//! TV is webOS 6) a cold launch may already play the video, and a second launch would restart it.
//! So the running query, the helper and the second launch are used only where it was measured,
//! webOS major version [`HELPER_WEBOS_MAJOR`] (`sn_platform::tv::device::info().major`, the
//! `major=` of the `webos:` log line); on any other version, and when the version is unknown
//! (major 0), [`run`] sends the single launch and nothing else. If a cold launch on another
//! version turns out to open only YouTube's menu, extend [`needs_helper`].
//!
//! # The sequence ([`run`])
//!
//! 0. A webOS version [`needs_helper`] says no to: launch once, in the app. Done (steps 1 to 4 are
//!    for the version it says yes to).
//! 1. `applicationManager/running` `{}`: YouTube runs when the reply lists its id. A failed query
//!    counts as not running.
//! 2. YouTube running: launch once, in the app. Done.
//! 3. YouTube not running: FIRST start the detached helper ([`super::trailer_helper`], through
//!    [`Bus::spawn_helper`]), because the app may get its `SDL_QUIT` within 0.3 s of the next
//!    step. A helper that cannot be started is logged, and the sequence goes on.
//! 4. Launch once, in the app. YouTube opens to its menu and the app closes (the viewer comes back
//!    to it from the home launcher). The helper polls the foreground app every second for at most
//!    15 s until it is YouTube, waits until the LATER of 5 s after that and 10 s after its own
//!    start, asks the foreground once more and sends the same launch again only if YouTube is
//!    still in front (the viewer may have left it meanwhile). Never in front: it exits.
//!
//! Known gap, practically unreachable: trailer B pressed while YouTube is already running, inside
//! an older helper A's window (about 25 s after A's start), starts no helper (a running YouTube
//! gets one in-app launch), so A, still waiting, may send its own video after B's and replay A's
//! over B's. A new helper replaces A only when YouTube is not running.
//!
//! The viewer is told (a toast) only when the first launch is refused or the bus fails: YouTube
//! does not open then, so the app stays and the toast is seen. The helper writes no log of its own.
//!
//! The decision logic (the payload, the reply parses, the step plan) is pure and runs against a
//! [`Bus`] the tests fake; the device's bus is [`Ls2Bus`], used by [`open`] only on the TV.

use crate::tv::youtube::Outcome;

/// The YouTube app's application id on webOS.
pub const APP_ID: &str = "youtube.leanback.v4";

const LAUNCH: &str = "luna://com.webos.applicationManager/launch";
const RUNNING: &str = "luna://com.webos.applicationManager/running";

/// The webOS major version the two-step launch was measured on (3.9.3), and the only one that
/// gets the helper (module doc).
pub const HELPER_WEBOS_MAJOR: u32 = 3;

/// Does webOS major version `major` get the running query, the helper and the second launch?
/// Only the version they were measured on; `0` (unknown) and every other version launch once.
pub fn needs_helper(major: u32) -> bool {
    major == HELPER_WEBOS_MAJOR
}

/// A YouTube video id as it may go into the payload: 6 to 20 characters of `[A-Za-z0-9_-]`. The
/// same rule `sn_stremio::types::is_youtube_id` applies on the screen's side; repeated here
/// because the platform layer sits below the addon protocol, and the id steers what YouTube opens.
pub(super) fn well_formed(id: &str) -> bool {
    (6..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The launch payload that plays video `id`, or `None` for an id that is not well formed. Built
/// with `serde_json`, so nothing in `id` can leave its string.
pub fn launch_payload(id: &str) -> Option<String> {
    well_formed(id).then(|| {
        serde_json::json!({ "id": APP_ID, "params": { "handledBy": "app", "contentTarget": format!("v={id}") } }).to_string()
    })
}

fn parse(reply: &str) -> Option<serde_json::Value> {
    serde_json::from_str(reply).ok()
}

/// The `running` reply lists YouTube: `{"running":[{"id":"youtube.leanback.v4",...}],...}`.
pub fn is_running(reply: &str) -> bool {
    parse(reply)
        .and_then(|v| v.get("running").and_then(|r| r.as_array().cloned()))
        .is_some_and(|apps| apps.iter().any(|a| a.get("id").and_then(|i| i.as_str()) == Some(APP_ID)))
}

/// Grade a `launch` reply: `Ok` for `returnValue: true`, else the service's `errorText`.
pub fn graded(reply: &str) -> Result<(), String> {
    let v = parse(reply);
    if v.as_ref().and_then(|v| v.get("returnValue")).and_then(|r| r.as_bool()) == Some(true) {
        return Ok(());
    }
    Err(v
        .as_ref()
        .and_then(|v| v.get("errorText"))
        .and_then(|t| t.as_str())
        .map_or_else(|| "no errorText in the reply".to_string(), str::to_string))
}

/// What [`run`] needs from the world: the bus, the helper and the log.
pub trait Bus {
    /// One call and its reply; `Err` is why no reply came (a bus failure, never a refusal).
    fn call(&mut self, uri: &str, payload: &str) -> Result<String, String>;
    /// Start the detached helper that sends the second launch of video `id` (module doc); `Err`
    /// is why none was started.
    fn spawn_helper(&mut self, id: &str) -> Result<(), String>;
    fn log(&mut self, line: &str);
}

/// The launch of video `payload`, graded, with its reply logged.
fn send(bus: &mut dyn Bus, payload: &str) -> Result<(), Outcome> {
    match bus.call(LAUNCH, payload) {
        Ok(reply) => match graded(&reply) {
            Ok(()) => {
                bus.log(&format!("youtube: launch accepted -> {reply}"));
                Ok(())
            }
            Err(error_text) => {
                bus.log(&format!("youtube: launch refused -> {reply}"));
                Err(Outcome::Refused { error_text })
            }
        },
        Err(detail) => {
            bus.log(&format!("youtube: launch failed: {detail}"));
            Err(Outcome::Bus { detail })
        }
    }
}

/// The whole in-app sequence for video `id` on webOS major version `webos_major` (module doc).
pub fn run(bus: &mut dyn Bus, id: &str, webos_major: u32) -> Outcome {
    let Some(payload) = launch_payload(id) else {
        bus.log("youtube: not a YouTube id - nothing sent");
        return Outcome::Refused { error_text: "not a YouTube id".into() };
    };
    bus.log(&format!("youtube: opening trailer {id}"));
    if !needs_helper(webos_major) {
        bus.log(&format!("youtube: webOS major {webos_major}: one launch, no helper (the two-step launch was measured on {HELPER_WEBOS_MAJOR})"));
        return match send(bus, &payload) {
            Ok(()) => Outcome::Opened,
            Err(outcome) => outcome,
        };
    }
    // a failed or refused `running` is no reason to stop: YouTube is then taken as not running,
    // which starts the helper and launches once, and at worst the video plays after the menu
    let running = match bus.call(RUNNING, "{}") {
        Ok(reply) => {
            let running = is_running(&reply);
            bus.log(&format!("youtube: running={running} -> {reply}"));
            running
        }
        Err(detail) => {
            bus.log(&format!("youtube: running query failed ({detail}), treating YouTube as not running"));
            false
        }
    };
    if !running {
        // BEFORE the launch: the app may be closed within 0.3 s of it, and the helper must exist by then
        match bus.spawn_helper(id) {
            Ok(()) => bus.log("youtube: helper started, it sends the second launch once YouTube is in front"),
            Err(detail) => bus.log(&format!("youtube: no helper started ({detail}) - YouTube opens to its menu only")),
        }
    }
    match send(bus, &payload) {
        Ok(()) => Outcome::Opened,
        Err(outcome) => outcome,
    }
}

/// The port's `open_trailer`, off-device: no bus.
#[cfg(any(feature = "hostsim", test, feature = "test-support"))]
pub fn open(id: &str) -> Outcome {
    sn_base::eventlog::log(&format!("youtube: no LS2 bus off-device - the trailer {id} would open on a television"));
    Outcome::NoBus
}

/// The device's bus: one anonymous registration for the whole sequence.
#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
struct Ls2Bus {
    registration: super::ls2::Registration,
}

#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
impl Bus for Ls2Bus {
    fn call(&mut self, uri: &str, payload: &str) -> Result<String, String> {
        use super::ls2::Fail;
        self.registration.call(uri, payload, std::time::Duration::from_secs(3)).map_err(|f| match f {
            Fail::Timeout => "timeout".to_string(),
            Fail::Setup { stage, detail, .. } if detail.is_empty() => format!("stage={stage}"),
            Fail::Setup { stage, detail, .. } => format!("stage={stage} ({detail})"),
        })
    }
    fn spawn_helper(&mut self, id: &str) -> Result<(), String> {
        super::trailer_helper::spawn(id, &sn_base::paths::in_runtime_dir(super::trailer_helper::LOCK))
    }
    fn log(&mut self, line: &str) {
        sn_base::eventlog::log(line);
    }
}

/// The port's `open_trailer`, on the television. Blocks for a few LS2 round trips (the door asserts it is off the frame thread).
#[cfg(all(not(feature = "hostsim"), not(any(test, feature = "test-support"))))]
pub fn open(id: &str) -> Outcome {
    match super::ls2::register() {
        Ok(registration) => run(&mut Ls2Bus { registration }, id, crate::tv::device::info().major),
        Err(e) => {
            sn_base::eventlog::log(&format!("youtube: LS2 {e}"));
            Outcome::Bus { detail: e.to_string() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YT_RUNNING: &str = r#"{"returnValue":true,"running":[{"id":"com.webos.app.home","processid":"1"},{"id":"youtube.leanback.v4","processid":"2","webprocessid":"3"}]}"#;
    const NONE_RUNNING: &str = r#"{"returnValue":true,"running":[{"id":"com.webos.app.home","processid":"1"}]}"#;
    const OK: &str = r#"{"returnValue":true}"#;
    const NO: &str = r#"{"returnValue":false,"errorCode":-1,"errorText":"Unknown app"}"#;

    /// One thing the sequence did, in order.
    #[derive(Debug, PartialEq)]
    enum Step {
        Call(String),
        Spawn(String),
    }

    /// A scripted bus: replies are consumed per URI in order (the last one repeats); every call,
    /// spawn and log is kept in order, so a test can say what came BEFORE what.
    struct Fake {
        replies: Vec<(&'static str, Vec<Result<String, String>>)>,
        steps: Vec<Step>,
        payloads: Vec<String>,
        logs: Vec<String>,
        spawn_result: Result<(), String>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake { replies: Vec::new(), steps: Vec::new(), payloads: Vec::new(), logs: Vec::new(), spawn_result: Ok(()) }
        }
        fn reply(mut self, uri: &'static str, replies: &[Result<&str, &str>]) -> Fake {
            self.replies.push((uri, replies.iter().map(|r| r.map(str::to_string).map_err(str::to_string)).collect()));
            self
        }
        fn steps(&self) -> Vec<String> {
            self.steps
                .iter()
                .map(|s| match s {
                    Step::Call(u) => u.rsplit('/').next().unwrap().to_string(),
                    Step::Spawn(_) => "spawn".to_string(),
                })
                .collect()
        }
    }

    impl Bus for Fake {
        fn call(&mut self, uri: &str, payload: &str) -> Result<String, String> {
            self.steps.push(Step::Call(uri.to_string()));
            if uri == LAUNCH {
                self.payloads.push(payload.to_string());
            }
            let slot = self.replies.iter_mut().find(|(u, _)| *u == uri).unwrap_or_else(|| panic!("no script for {uri}"));
            if slot.1.len() > 1 { slot.1.remove(0) } else { slot.1[0].clone() }
        }
        fn spawn_helper(&mut self, id: &str) -> Result<(), String> {
            self.steps.push(Step::Spawn(id.to_string()));
            self.spawn_result.clone()
        }
        fn log(&mut self, line: &str) {
            self.logs.push(line.to_string());
        }
    }

    fn logged(f: &Fake, needle: &str) -> bool {
        f.logs.iter().any(|l| l.contains(needle))
    }

    #[test]
    fn the_payload_is_exactly_what_plays_a_video() {
        let v: serde_json::Value = serde_json::from_str(&launch_payload("PLl99DlL6b4").unwrap()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"id":"youtube.leanback.v4","params":{"handledBy":"app","contentTarget":"v=PLl99DlL6b4"}})
        );
    }

    #[test]
    fn a_malformed_id_builds_no_payload_and_spawns_and_sends_nothing() {
        for bad in ["", "short", "has space1", "a\"b\\c12345", "x\",\"id\":\"evil", "abcdefghij01234567890", "v=abcdef", "ünïcödé12"] {
            assert_eq!(launch_payload(bad), None, "{bad:?}");
            let mut f = Fake::new();
            assert!(matches!(run(&mut f, bad, HELPER_WEBOS_MAJOR), Outcome::Refused { .. }), "{bad:?}");
            assert!(f.steps.is_empty(), "nothing is spawned or sent for {bad:?}");
        }
    }

    #[test]
    fn the_replies_are_read_for_youtube_and_nothing_else() {
        assert!(is_running(YT_RUNNING));
        assert!(!is_running(NONE_RUNNING));
        for odd in ["", "not json", "{}", r#"{"running":null}"#, r#"{"running":"youtube.leanback.v4"}"#, r#"{"running":[{"id":"youtube.leanback.v4x"}]}"#] {
            assert!(!is_running(odd), "{odd:?}");
        }
        assert_eq!(graded(OK), Ok(()));
        assert_eq!(graded(NO), Err("Unknown app".into()));
        assert_eq!(graded(r#"{"returnValue":"true"}"#), Err("no errorText in the reply".into()));
        assert!(graded("").is_err());
    }

    #[test]
    fn running_youtube_gets_one_launch_and_no_helper() {
        let mut f = Fake::new().reply(RUNNING, &[Ok(YT_RUNNING)]).reply(LAUNCH, &[Ok(OK)]);
        assert_eq!(run(&mut f, "PLl99DlL6b4", HELPER_WEBOS_MAJOR), Outcome::Opened);
        assert_eq!(f.steps(), ["running", "launch"], "no spawn");
        assert_eq!(f.payloads, [launch_payload("PLl99DlL6b4").unwrap()]);
    }

    /// The point of the whole design: the helper exists before the launch that gets the app closed.
    #[test]
    fn a_cold_youtube_gets_the_helper_first_and_then_one_launch() {
        let mut f = Fake::new().reply(RUNNING, &[Ok(NONE_RUNNING)]).reply(LAUNCH, &[Ok(OK)]);
        assert_eq!(run(&mut f, "PLl99DlL6b4", HELPER_WEBOS_MAJOR), Outcome::Opened);
        assert_eq!(f.steps(), ["running", "spawn", "launch"], "the spawn comes BEFORE the first launch");
        assert_eq!(f.steps[1], Step::Spawn("PLl99DlL6b4".into()));
        assert_eq!(f.payloads, [launch_payload("PLl99DlL6b4").unwrap()], "one launch in the app");
        assert!(logged(&f, "helper started"), "{:?}", f.logs);
        assert!(logged(&f, "launch accepted"));
    }

    #[test]
    fn a_failed_running_query_takes_the_cold_path() {
        let mut f = Fake::new().reply(RUNNING, &[Err("timeout")]).reply(LAUNCH, &[Ok(OK)]);
        assert_eq!(run(&mut f, "PLl99DlL6b4", HELPER_WEBOS_MAJOR), Outcome::Opened);
        assert_eq!(f.steps(), ["running", "spawn", "launch"]);
        assert!(logged(&f, "running query failed"), "{:?}", f.logs);
    }

    /// No helper: logged, and the first launch still goes (YouTube opens to its menu), no toast.
    #[test]
    fn a_helper_that_cannot_start_is_logged_and_the_launch_still_goes() {
        let mut f = Fake::new().reply(RUNNING, &[Ok(NONE_RUNNING)]).reply(LAUNCH, &[Ok(OK)]);
        f.spawn_result = Err("could not run /bin/sh: gone".into());
        let outcome = run(&mut f, "PLl99DlL6b4", HELPER_WEBOS_MAJOR);
        assert_eq!(outcome, Outcome::Opened);
        assert!(!outcome.failed(), "no toast");
        assert_eq!(f.steps(), ["running", "spawn", "launch"]);
        assert!(logged(&f, "no helper started (could not run /bin/sh: gone)"), "{:?}", f.logs);
    }

    /// A refused first launch: the app stays open, so the toast is seen; the helper (already
    /// started) finds YouTube never in front and ends quietly.
    #[test]
    fn a_refused_first_launch_asks_for_the_toast() {
        let mut f = Fake::new().reply(RUNNING, &[Ok(NONE_RUNNING)]).reply(LAUNCH, &[Ok(NO)]);
        let outcome = run(&mut f, "PLl99DlL6b4", HELPER_WEBOS_MAJOR);
        assert_eq!(outcome, Outcome::Refused { error_text: "Unknown app".into() });
        assert!(outcome.failed());
        assert_eq!(f.steps(), ["running", "spawn", "launch"], "nothing after the refusal");
        assert!(logged(&f, "launch refused"));
        let mut f = Fake::new().reply(RUNNING, &[Ok(YT_RUNNING)]).reply(LAUNCH, &[Ok(NO)]);
        assert!(run(&mut f, "PLl99DlL6b4", HELPER_WEBOS_MAJOR).failed(), "also when YouTube was running");
        assert_eq!(f.steps(), ["running", "launch"]);
    }

    #[test]
    fn a_bus_failure_on_the_launch_asks_for_the_toast() {
        let mut f = Fake::new().reply(RUNNING, &[Err("timeout")]).reply(LAUNCH, &[Err("stage=call (code -1: no)")]);
        let outcome = run(&mut f, "PLl99DlL6b4", HELPER_WEBOS_MAJOR);
        assert_eq!(outcome, Outcome::Bus { detail: "stage=call (code -1: no)".into() });
        assert!(outcome.failed());
        assert!(logged(&f, "launch failed"), "{:?}", f.logs);
    }

    /// The two-step launch was measured on webOS 3 only: every other version, and an unknown one,
    /// gets the one launch, with no running query (the fake has no script for it, so a call
    /// would panic), no helper and no second launch.
    #[test]
    fn other_webos_versions_get_one_launch_and_no_helper() {
        assert!(needs_helper(3));
        for major in [0, 1, 2, 4, 5, 6, 10, u32::MAX] {
            assert!(!needs_helper(major), "{major}");
            let mut f = Fake::new().reply(LAUNCH, &[Ok(OK)]);
            assert_eq!(run(&mut f, "PLl99DlL6b4", major), Outcome::Opened, "{major}");
            assert_eq!(f.steps(), ["launch"], "{major}: nothing but the launch");
            assert_eq!(f.payloads, [launch_payload("PLl99DlL6b4").unwrap()], "{major}");
            assert!(logged(&f, "one launch, no helper"), "{major}: {:?}", f.logs);
        }
    }

    /// On another version a refused launch or a bus failure is still the toast, and a spawn is
    /// never attempted, whatever the bus says.
    #[test]
    fn a_failed_single_launch_on_another_version_still_asks_for_the_toast() {
        let mut f = Fake::new().reply(LAUNCH, &[Ok(NO)]);
        assert_eq!(run(&mut f, "PLl99DlL6b4", 6), Outcome::Refused { error_text: "Unknown app".into() });
        assert_eq!(f.steps(), ["launch"]);
        let mut f = Fake::new().reply(LAUNCH, &[Err("timeout")]);
        assert_eq!(run(&mut f, "PLl99DlL6b4", 0), Outcome::Bus { detail: "timeout".into() });
        assert_eq!(f.steps(), ["launch"]);
        // a malformed id is refused before the version matters
        let mut f = Fake::new();
        assert!(matches!(run(&mut f, "bad id", 6), Outcome::Refused { .. }));
        assert!(f.steps.is_empty());
    }
}
