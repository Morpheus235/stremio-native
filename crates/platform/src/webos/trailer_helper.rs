//! The detached helper that sends a trailer's SECOND launch: the process half of
//! [`super::youtube`]. Why a separate process is in that module's doc: webOS closes the app about
//! 0.3 s after YouTube's first launch, and a worker thread dies with it.
//!
//! The helper is [`SCRIPT`] (`trailer_helper.sh`, POSIX sh for the television's busybox ash), run as
//! `/bin/sh -c <script> trailer-helper <video id> <log path>`. The id and the log path are
//! positional arguments, never part of the script's text; the script checks the id's charset
//! again, because it builds the launch payload from it.
//!
//! # Detaching
//!
//! [`spawn`] starts a short launcher shell and waits for it, so no zombie stays. The launcher
//! ignores HUP, closes every file descriptor above 2 it inherited, starts the helper under
//! `setsid` (its own session) with stdin, stdout and stderr on `/dev/null` in the background, and
//! exits at once: the helper is reparented to init and lives on after the app has been closed.
//! Nothing on the television kills leftover processes (the cgroups are flat). No `unsafe` and no
//! extra crate: the session comes from the television's own `setsid`, and the descriptors are
//! closed by the shell, because `Command` closes only those opened with CLOEXEC (the display,
//! LS2 and GL descriptors of the app may not be) and the helper must not keep any of them alive.
//!
//! # One at a time, the clock and the log
//!
//! The helper holds a lock directory (`<log>.lock`, holding `<pid> <seconds>`) while it runs and
//! removes it on exit, but only while the pid in it is still its own. The NEWEST helper wins: a
//! live, fresh holder that really is a trailer helper is sent TERM and given 5 s to go before the
//! lock is taken over (5 s because a TERM that lands during a `luna-send` call waits for it, up to
//! 3 s plus `script`). Every sleep in the helper is a background `sleep` it `wait`s for, because a
//! foreground one defers the TERM trap until it ends, and the final wait lasts up to 9 s. A lock
//! whose pid is dead, older than two minutes, or without a valid stamp is taken over at once.
//! Times are whole seconds of `/proc/uptime` (monotonic), or of `date +%s` where that file is
//! unreadable.
//!
//! The runtime directory is world-writable and the helper runs as root, so a symlink at the log's
//! or the lock's path is removed before anything is written there. The log is
//! `trailer-helper.log` in the app's runtime directory
//! ([`sn_base::paths::runtime_file::TRAILER_HELPER`]), truncated at each start: short timestamped
//! lines, the YouTube id and nothing else of the viewer's.

use std::path::Path;
use std::process::{Command, Stdio};

/// The helper, embedded.
pub const SCRIPT: &str = include_str!("trailer_helper.sh");

/// The launcher shell, run as `/bin/sh -c <this> trailer-launcher <script> <id> <log path>`.
///
/// In order: the tool search path (the script's rule; `SN_TRAILER_TEST_PATH` is for the host tests
/// only), ignore HUP, exit 3 when the television has no `setsid` (so no helper starts), close the
/// inherited descriptors above 2 (those listed in `/proc/<pid>/fd`, else 3 to 63; a shell that
/// cannot redirect numbers above 9, like dash, leaves those open), then start the helper in its
/// own session in the background and exit 0.
const LAUNCHER: &str = r#"PATH=${SN_TRAILER_TEST_PATH:-/usr/sbin:/usr/bin:/sbin:/bin}
export PATH
trap '' HUP
command -v setsid > /dev/null 2>&1 || exit 3
# busybox ash and bash redirect any fd number; dash only 0 to 9 (and dies on `exec 12>&-`)
if (eval "exec 99>&-") 2> /dev/null; then WIDE=1; else WIDE=0; fi
close_fd() {
    if [ "$1" -lt 10 ] || [ "$WIDE" = 1 ]; then eval "exec $1>&-"; fi
}
if [ -d /proc/$$/fd ]; then
    for f in /proc/$$/fd/*; do
        n=${f##*/}
        case $n in
            '' | *[!0-9]*) continue ;;
        esac
        if [ "$n" -gt 2 ]; then close_fd "$n"; fi
    done
else
    n=3
    while [ "$n" -le 63 ]; do
        close_fd "$n"
        n=$((n + 1))
    done
fi
setsid /bin/sh -c "$1" trailer-helper "$2" "$3" < /dev/null > /dev/null 2>&1 &
exit 0
"#;

/// Start the helper for video `id`, detached, logging to `log`. Returns once the launcher has
/// exited (a few milliseconds); `Err` says why no helper was started. A malformed id starts nothing.
pub fn spawn(id: &str, log: &Path) -> Result<(), String> {
    spawn_with(id, log, &[])
}

/// [`spawn`] with `env` added to the launcher's environment (and so the helper's): the host tests
/// pass the tool search path and the timing overrides here instead of changing the test process's.
fn spawn_with(id: &str, log: &Path, env: &[(&str, &str)]) -> Result<(), String> {
    if !super::youtube::well_formed(id) {
        return Err("not a YouTube id".to_string());
    }
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(LAUNCHER)
        .arg("trailer-launcher")
        .arg(SCRIPT)
        .arg(id)
        .arg(log)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("could not run /bin/sh: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the launcher exited with {status}"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::webos::youtube::launch_payload;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    const ID: &str = "PLl99DlL6b4";
    // the television's own reply shapes (webOS 3.9.3)
    const FRONT: &str = r#"{"appId":"youtube.leanback.v4","returnValue":true,"windowId":"","processId":""}"#;
    const HOME: &str = r#"{"appId":"com.webos.app.home","returnValue":true,"windowId":"","processId":""}"#;
    const OK: &str = r#"{"returnValue":true}"#;

    /// A scratch directory holding fake tools on a VIRTUAL clock: `clock` is the uptime file the
    /// script reads (`SN_TRAILER_TEST_UPTIME`), the fake `date +%s` reads it too, and the fake
    /// `sleep N` advances it, so the helper's timeline is exact and costs no wall time. The fake
    /// `script` records `$SHELL` and runs its `-c` command; the fake `luna-send` appends
    /// `<clock> <args>` to `calls`, records which of fds 9 and 12 it can see in `fds`, and answers
    /// a foreground query from the next line of `fg` (the last line repeats), a launch with `launch`.
    struct Rig {
        dir: PathBuf,
    }

    static NEXT: AtomicU32 = AtomicU32::new(0);

    impl Rig {
        fn new() -> Rig {
            let dir = std::env::temp_dir().join(format!("sn-trailer-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("bin")).unwrap();
            let d = dir.display();
            let tool = |name: &str, body: String| {
                let path = dir.join("bin").join(name);
                fs::write(&path, format!("#!/bin/sh\nD='{d}'\nclock_now() {{\n  read c r < \"$D/clock\"\n  echo \"${{c%%.*}}\"\n}}\n{body}")).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            };
            tool("date", "case \"$1\" in +%s) clock_now ;; *) exec /bin/date \"$@\" ;; esac\n".into());
            // `realsleep` makes every fake sleep take half a second of real time too (the tests that
            // wait for another process to handle a signal: generous, so load cannot make them flake); `SN_TEST_REAL_SLEEP` in a helper's environment makes it a real, interruptible
            // sleep of the whole time (the test that interrupts a helper in its final wait)
            tool(
                "sleep",
                "[ -n \"$SN_TEST_REAL_SLEEP\" ] && echo $$ > \"$D/sleep.pid\"\necho \"$1\" >> \"$D/sleeps\"\nc=$(clock_now)\necho $((c + $1)) > \"$D/clock\"\n[ -n \"$SN_TEST_REAL_SLEEP\" ] && exec /bin/sleep \"$1\"\n[ -e \"$D/realsleep\" ] && /bin/sleep 0.5\nexit 0\n".into(),
            );
            tool("script", "echo \"$SHELL\" >> \"$D/shells\"\n[ \"$1\" = -q ] && shift\n[ \"$1\" = -c ] && shift\nexec /bin/sh -c \"$1\"\n".into());
            tool(
                "luna-send",
                concat!(
                    "echo \"$(clock_now) $*\" >> \"$D/calls\"\n",
                    "for n in 9 12; do [ -e /dev/fd/$n ] && echo \"$n\" >> \"$D/fds\"; done\n",
                    // a rival that takes the lock over while this helper works
                    "[ -e \"$D/steal\" ] && echo \"999999 0\" > \"$(cat \"$D/steal\")/pid\"\n",
                    "case \"$*\" in\n*getForegroundAppInfo*)\n",
                    "n=$(cat \"$D/fgn\" 2>/dev/null || echo 0)\nn=$((n + 1))\necho $n > \"$D/fgn\"\n",
                    "total=$(wc -l < \"$D/fg\")\n[ $n -gt $total ] && n=$total\nsed -n \"${n}p\" \"$D/fg\" ;;\n",
                    "*) cat \"$D/launch\" ;;\nesac\nexit 0\n"
                )
                .into(),
            );
            // the television's setsid, as far as the launcher needs it: run the rest in the foreground
            tool("setsid", "echo \"$#\" >> \"$D/setsid\"\nexec \"$@\"\n".into());
            fs::write(dir.join("clock"), "0").unwrap();
            fs::write(dir.join("launch"), format!("{OK}\n")).unwrap();
            let rig = Rig { dir };
            rig.foreground(&[FRONT]);
            rig
        }

        fn foreground(&self, replies: &[&str]) {
            fs::write(self.dir.join("fg"), replies.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
        }

        fn set_clock(&self, text: &str) {
            fs::write(self.dir.join("clock"), text).unwrap();
        }

        fn real_sleeps(&self) {
            fs::write(self.dir.join("realsleep"), "").unwrap();
        }

        fn log_path(&self) -> PathBuf {
            self.dir.join("trailer-helper.log")
        }

        fn lock_path(&self) -> PathBuf {
            self.dir.join("trailer-helper.log.lock")
        }

        /// A lock directory whose pid file holds `content`.
        fn lock_with(&self, content: &str) {
            fs::create_dir(self.lock_path()).unwrap();
            fs::write(self.lock_path().join("pid"), content).unwrap();
        }

        /// The tool path: the fakes first, then the host's own (`mkdir`, `rm`, `tr`, `wc`, `sed`...).
        fn test_path(&self) -> String {
            format!("{}:{}", self.dir.join("bin").display(), std::env::var("PATH").unwrap_or_default())
        }

        fn uptime_file(&self) -> String {
            self.dir.join("clock").display().to_string()
        }

        /// The environment the helper needs to run on the fakes.
        fn env(&self) -> Vec<(String, String)> {
            vec![("SN_TRAILER_TEST_PATH".into(), self.test_path()), ("SN_TRAILER_TEST_UPTIME".into(), self.uptime_file())]
        }

        /// Run the helper on the host's `sh`, as the launcher would, with `env` on top.
        fn run(&self, id: &str, env: &[(&str, &str)]) -> std::process::ExitStatus {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(SCRIPT).arg("trailer-helper").arg(id).arg(self.log_path()).envs(self.env());
            for (k, v) in env {
                cmd.env(k, v);
            }
            cmd.stdin(Stdio::null()).status().expect("run sh")
        }

        /// Every luna-send call: (virtual second, arguments).
        fn calls(&self) -> Vec<(u64, String)> {
            let text = fs::read_to_string(self.dir.join("calls")).unwrap_or_default();
            text.lines()
                .map(|l| {
                    let (t, rest) = l.split_once(' ').unwrap();
                    (t.parse().unwrap(), rest.to_string())
                })
                .collect()
        }

        fn launches(&self) -> Vec<(u64, String)> {
            self.calls().into_iter().filter(|(_, a)| a.contains("applicationManager/launch")).collect()
        }

        fn foreground_calls(&self) -> Vec<u64> {
            self.calls().into_iter().filter(|(_, a)| a.contains("getForegroundAppInfo")).map(|(t, _)| t).collect()
        }

        fn log(&self) -> String {
            fs::read_to_string(self.log_path()).unwrap_or_default()
        }

        fn file(&self, name: &str) -> String {
            fs::read_to_string(self.dir.join(name)).unwrap_or_default()
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A process standing in for another running helper: a shell whose command line carries the
    /// name `trailer-helper`, started in the background of a second shell that exits, so it is
    /// orphaned (init reaps it) and `kill -0` tells dead from alive. Killed when dropped.
    struct Holder {
        pid: u32,
    }

    impl Holder {
        fn start(ignore_term: bool) -> Holder {
            let body = if ignore_term { "trap '' TERM; while :; do sleep 1; done" } else { "while :; do sleep 1; done" };
            let out = Command::new("sh")
                .arg("-c")
                .arg("sh -c \"$1\" trailer-helper < /dev/null > /dev/null 2>&1 & echo $!")
                .arg("outer")
                .arg(body)
                .output()
                .expect("start the stand-in helper");
            let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().expect("its pid");
            Holder { pid }
        }

        fn alive(&self) -> bool {
            Command::new("kill").args(["-0", &self.pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
        }
    }

    impl Drop for Holder {
        fn drop(&mut self) {
            let _ = Command::new("kill").args(["-9", &self.pid.to_string()]).stderr(Stdio::null()).status();
        }
    }

    /// The JSON the launch call carried: the last argument of the recorded luna-send line.
    fn payload_of(args: &str) -> serde_json::Value {
        let json = args.split_once(" luna://com.webos.applicationManager/launch ").expect("a launch call").1;
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"))
    }

    fn expected_payload() -> serde_json::Value {
        serde_json::from_str(&launch_payload(ID).unwrap()).unwrap()
    }

    #[test]
    fn youtube_coming_to_the_front_gets_exactly_one_launch_with_the_exact_payload() {
        let rig = Rig::new();
        rig.foreground(&[HOME, HOME, FRONT]);
        // the environment's SHELL must not reach `script`'s command
        assert!(rig.run(ID, &[("SHELL", "/bin/false")]).success());
        let launches = rig.launches();
        assert_eq!(launches.len(), 1, "{:?}", rig.calls());
        assert_eq!(payload_of(&launches[0].1), expected_payload());
        assert!(launches[0].1.starts_with("-n 1 -w 3000 luna://"), "{}", launches[0].1);
        // in front at 2 s; the later of 2 + 5 and 0 + 10 is 10
        assert_eq!(launches[0].0, 10);
        assert_eq!(rig.foreground_calls(), [0, 1, 2, 10], "polls at 0, 1 and 2, then one check before the launch");
        let log = rig.log();
        assert!(log.contains(&format!("start id={ID}")), "{log}");
        assert!(log.contains("YouTube in front after 2s"), "{log}");
        assert!(log.contains("sending the second launch"), "{log}");
        assert!(log.contains(&format!("second launch reply: {OK}")), "{log}");
        assert!(!rig.lock_path().exists(), "the lock is removed on exit");
        let shells = rig.file("shells");
        assert!(shells.lines().count() >= 5 && shells.lines().all(|l| l == "/bin/sh"), "script ran with SHELL: {shells:?}");
    }

    #[test]
    fn a_slow_youtube_waits_five_seconds_after_it_is_in_front() {
        let rig = Rig::new();
        rig.foreground(&[HOME; 9].iter().copied().chain([FRONT]).collect::<Vec<_>>());
        assert!(rig.run(ID, &[]).success());
        // in front at 9 s: 9 + 5 is later than 0 + 10
        assert_eq!(rig.launches().iter().map(|(t, _)| *t).collect::<Vec<_>>(), [14]);
    }

    #[test]
    fn youtube_never_in_front_means_no_launch_and_a_log_line() {
        let rig = Rig::new();
        rig.foreground(&[HOME]);
        assert!(rig.run(ID, &[]).success());
        assert!(rig.launches().is_empty());
        assert_eq!(rig.foreground_calls(), (0..=15).collect::<Vec<u64>>(), "one poll a second for 15 s");
        assert!(rig.log().contains("never came to the front within 15s"), "{}", rig.log());
        assert!(!rig.lock_path().exists());
    }

    #[test]
    fn leaving_youtube_before_the_second_launch_means_no_launch() {
        let rig = Rig::new();
        rig.foreground(&[HOME, FRONT, HOME]);
        assert!(rig.run(ID, &[]).success());
        assert!(rig.launches().is_empty(), "{:?}", rig.calls());
        assert!(rig.log().contains("left YouTube before the second launch"), "{}", rig.log());
        assert!(!rig.lock_path().exists());
    }

    #[test]
    fn a_silent_luna_send_is_never_in_front_not_a_hang() {
        let rig = Rig::new();
        rig.foreground(&[""]);
        assert!(rig.run(ID, &[]).success());
        assert!(rig.launches().is_empty());
    }

    #[test]
    fn the_timings_can_be_overridden_and_a_bad_value_falls_back() {
        let rig = Rig::new();
        rig.foreground(&[HOME]);
        rig.run(ID, &[("SN_TRAILER_POLL", "2"), ("SN_TRAILER_FRONT", "6")]);
        assert_eq!(rig.foreground_calls(), [0, 2, 4, 6]);
        let rig = Rig::new();
        rig.foreground(&[FRONT]);
        rig.run(ID, &[("SN_TRAILER_SETTLE", "1"), ("SN_TRAILER_REPLAY", "3")]);
        assert_eq!(rig.launches().iter().map(|(t, _)| *t).collect::<Vec<_>>(), [3]);
        // garbage (not a number, a leading zero) means the default: one poll a second
        for bad in ["x", "08"] {
            let rig = Rig::new();
            rig.foreground(&[HOME]);
            rig.run(ID, &[("SN_TRAILER_POLL", bad), ("SN_TRAILER_FRONT", "2")]);
            assert_eq!(rig.foreground_calls(), [0, 1, 2], "{bad:?}");
        }
    }

    /// The clock is the integer part of /proc/uptime's first field (`1000.57 99.9` is 1000), and
    /// `date +%s` where that file cannot be read.
    #[test]
    fn the_clock_is_the_uptime_and_falls_back_to_date() {
        let rig = Rig::new();
        rig.set_clock("1000.57 99.9");
        let holder = Holder::start(false);
        // a stamp 900 s old is stale however alive its pid: no TERM
        rig.lock_with(&format!("{} 100\n", holder.pid));
        assert!(rig.run(ID, &[]).success());
        assert!(holder.alive(), "an old lock's holder is left alone");
        assert_eq!(rig.launches().len(), 1);
        assert_eq!(rig.launches()[0].0, 1010, "10 s after the start, read from the uptime file");
        // no uptime file: the same timeline from the fake `date +%s`
        let rig = Rig::new();
        rig.foreground(&[HOME, FRONT]);
        assert!(rig.run(ID, &[("SN_TRAILER_TEST_UPTIME", "/nonexistent/uptime")]).success());
        assert_eq!(rig.launches().iter().map(|(t, _)| *t).collect::<Vec<_>>(), [10]);
        // an odd uptime file falls back too
        let rig = Rig::new();
        fs::write(rig.dir.join("uptime-odd"), "not a number\n").unwrap();
        let odd = rig.dir.join("uptime-odd").display().to_string();
        assert!(rig.run(ID, &[("SN_TRAILER_TEST_UPTIME", &odd)]).success());
        assert_eq!(rig.launches().len(), 1);
    }

    /// The newest helper wins: the live, fresh holder is sent TERM and gone, and the new helper
    /// runs and sends its launch.
    #[test]
    fn a_live_fresh_helper_is_replaced_by_the_newest() {
        let rig = Rig::new();
        rig.real_sleeps();
        let holder = Holder::start(false);
        assert!(holder.alive());
        rig.lock_with(&format!("{} 0\n", holder.pid));
        assert!(rig.run(ID, &[]).success());
        assert!(!holder.alive(), "the old helper was terminated");
        assert_eq!(rig.launches().len(), 1, "{:?}\n{}", rig.calls(), rig.log());
        assert_eq!(payload_of(&rig.launches()[0].1), expected_payload());
        assert!(rig.log().contains(&format!("replaced the running trailer helper ({})", holder.pid)), "{}", rig.log());
        assert!(!rig.lock_path().exists(), "the new helper's own lock is removed at the end");
    }

    /// The window the audit named: an older helper sitting in its long final wait must still be
    /// replaced within the newcomer's 5 s. Its wait is a background sleep it `wait`s for, which TERM
    /// interrupts at once; the trap also takes the sleep with it, so none stays behind.
    #[test]
    fn a_helper_in_its_final_wait_is_replaced_and_leaves_no_sleep_behind() {
        // under 120, because the fake sleep also moves the virtual clock by it and the old helper's
        // lock must still be fresh for the newcomer
        let secs = "75".to_string();
        let rig = Rig::new();
        rig.real_sleeps();
        // the OLD helper: YouTube in front at once, then a long final wait that really sleeps; started
        // in the background of a shell that exits, so init reaps it when it dies
        let out = Command::new("sh")
            .arg("-c")
            .arg("sh -c \"$1\" trailer-helper \"$2\" \"$3\" < /dev/null > /dev/null 2>&1 & echo $!")
            .arg("outer")
            .arg(SCRIPT)
            .arg(ID)
            .arg(rig.log_path())
            .envs(rig.env())
            .env("SN_TRAILER_SETTLE", &secs)
            .env("SN_TRAILER_REPLAY", &secs)
            .env("SN_TEST_REAL_SLEEP", "1")
            .output()
            .unwrap();
        let old = Holder { pid: String::from_utf8_lossy(&out.stdout).trim().parse().expect("its pid") };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !rig.file("sleeps").lines().any(|l| l == secs) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(rig.file("sleeps").lines().any(|l| l == secs), "the old helper is in its final wait: {}", rig.log());
        assert!(old.alive());
        let sleep = Holder { pid: rig.file("sleep.pid").trim().parse().expect("the sleep's pid") };
        assert!(sleep.alive(), "the old helper's final wait is a real sleep");
        // the NEWEST helper: no waits of its own
        assert!(rig.run(ID, &[("SN_TRAILER_SETTLE", "0"), ("SN_TRAILER_REPLAY", "0")]).success());
        assert!(!old.alive(), "the old helper went within the newcomer's 5 s");
        assert_eq!(rig.launches().len(), 1, "only the newest helper launches: {:?}\n{}", rig.calls(), rig.log());
        assert!(rig.log().contains("replaced the running trailer helper"), "{}", rig.log());
        // and the sleep it was in (the shim records its pid) went with it
        assert!(!sleep.alive(), "a sleep was left behind");
    }

    /// A holder that will not go: the newcomer gives up after its 5 s and leaves the lock alone.
    #[test]
    fn a_holder_that_ignores_term_keeps_the_lock_and_the_newcomer_exits() {
        let rig = Rig::new();
        rig.real_sleeps();
        let holder = Holder::start(true);
        rig.lock_with(&format!("{} 0\n", holder.pid));
        assert!(rig.run(ID, &[]).success());
        assert!(holder.alive());
        assert!(rig.calls().is_empty(), "{:?}", rig.calls());
        assert!(rig.lock_path().join("pid").exists());
        let log = rig.log();
        assert!(log.contains("replacing") && log.contains("another trailer helper is running"), "{log}");
        assert_eq!(rig.file("sleeps").lines().count(), 5, "it waited 5 s, in five 1 s sleeps");
    }

    /// The old helper's exit trap must not delete the lock of the helper that replaced it.
    #[test]
    fn a_helper_removes_only_its_own_lock() {
        let rig = Rig::new();
        fs::write(rig.dir.join("steal"), rig.lock_path().display().to_string()).unwrap();
        assert!(rig.run(ID, &[]).success());
        assert_eq!(fs::read_to_string(rig.lock_path().join("pid")).unwrap().trim(), "999999 0", "the other owner's lock stays");
    }

    #[test]
    fn a_lock_whose_pid_is_dead_is_stale_and_taken_over() {
        let rig = Rig::new();
        let mut gone = Command::new("true").spawn().unwrap();
        let pid = gone.id();
        gone.wait().unwrap();
        rig.lock_with(&format!("{pid} 0\n"));
        assert!(rig.run(ID, &[]).success());
        assert_eq!(rig.launches().len(), 1, "{:?}", rig.calls());
        assert!(!rig.lock_path().exists(), "its own lock is removed at the end");
    }

    /// Fresh needs a valid stamp that is not in the future and under two minutes old; anything
    /// else is stale, and a stale lock's live holder is neither waited for nor sent a TERM.
    #[test]
    fn a_missing_or_odd_or_future_stamp_is_stale() {
        for content in ["{pid}\n", "{pid} \n", "{pid} soon\n", "{pid} 05\n", "{pid} 5000\n", "{pid} -3\n"] {
            let rig = Rig::new();
            rig.set_clock("1000");
            let holder = Holder::start(false);
            rig.lock_with(&content.replace("{pid}", &holder.pid.to_string()));
            assert!(rig.run(ID, &[]).success());
            assert!(holder.alive(), "{content:?}: a lock without a fresh stamp is stale, its holder is left alone");
            assert_eq!(rig.launches().len(), 1, "{content:?}: {:?}", rig.calls());
            assert!(!rig.log().contains("replacing"), "{content:?}");
        }
    }

    /// With /proc, a live pid whose command line is not a trailer helper is never sent a TERM (the
    /// pid was reused). The test process itself is that pid, so a wrong TERM would end the run.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_reused_pid_that_is_not_a_helper_is_left_alone() {
        let rig = Rig::new();
        rig.lock_with(&format!("{} 0\n", std::process::id()));
        assert!(rig.run(ID, &[]).success());
        assert_eq!(rig.launches().len(), 1, "{:?}\n{}", rig.calls(), rig.log());
        assert!(!rig.log().contains("replacing"));
    }

    #[test]
    fn the_log_is_truncated_at_each_start() {
        let rig = Rig::new();
        fs::write(rig.log_path(), "old line from the last run\n").unwrap();
        assert!(rig.run(ID, &[]).success());
        assert!(!rig.log().contains("old line"), "{}", rig.log());
    }

    /// The log path is a symlink someone left in the shared runtime directory: it is removed, never
    /// written through.
    #[test]
    fn a_symlinked_log_is_replaced_not_followed() {
        use std::os::unix::fs::symlink;
        let rig = Rig::new();
        let victim = rig.dir.join("victim");
        fs::write(&victim, "precious\n").unwrap();
        symlink(&victim, rig.log_path()).unwrap();
        assert!(rig.run(ID, &[]).success());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n", "the target is untouched");
        assert!(!fs::symlink_metadata(rig.log_path()).unwrap().file_type().is_symlink(), "now a plain file");
        assert!(rig.log().contains("start id="), "{}", rig.log());
        // the same for the early refusal path, which appends without truncating
        let rig = Rig::new();
        let victim = rig.dir.join("victim");
        fs::write(&victim, "precious\n").unwrap();
        symlink(&victim, rig.log_path()).unwrap();
        assert_eq!(rig.run("bad id!", &[]).code(), Some(2));
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n");
    }

    /// The lock path is a symlink to a directory that looks like a live helper's lock: it is
    /// removed (the target untouched), and the helper runs.
    #[test]
    fn a_symlinked_lock_is_replaced_not_followed() {
        use std::os::unix::fs::symlink;
        let rig = Rig::new();
        let holder = Holder::start(false);
        let target = rig.dir.join("elsewhere");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("pid"), format!("{} 0\n", holder.pid)).unwrap();
        symlink(&target, rig.lock_path()).unwrap();
        assert!(rig.run(ID, &[]).success());
        assert!(holder.alive(), "the symlink's target was not read as a lock");
        assert_eq!(fs::read_to_string(target.join("pid")).unwrap(), format!("{} 0\n", holder.pid), "nor written");
        assert_eq!(rig.launches().len(), 1);
        assert!(!rig.lock_path().exists());
    }

    #[test]
    fn a_malformed_id_sends_nothing_in_the_script_either() {
        for bad in ["", "short", "has space1", "x\",\"id\":\"evil", "a'b;rm -rf /", "abcdefghij01234567890", "v=abcdef", "$(id)abcdef"] {
            let rig = Rig::new();
            assert_eq!(rig.run(bad, &[]).code(), Some(2), "{bad:?}");
            assert!(rig.calls().is_empty(), "{bad:?}");
            assert!(rig.log().contains("not a YouTube id"), "{bad:?}");
            assert!(!rig.lock_path().exists(), "{bad:?}");
        }
    }

    #[test]
    fn spawn_refuses_a_malformed_id_and_starts_nothing() {
        let rig = Rig::new();
        for bad in ["", "short", "x\",\"id\":\"evil", "abcdefghij01234567890"] {
            assert_eq!(spawn(bad, &rig.log_path()), Err("not a YouTube id".to_string()), "{bad:?}");
        }
        assert!(!rig.log_path().exists());
        assert!(!rig.dir.join("setsid").exists());
    }

    fn spawn_on(rig: &Rig) -> Result<(), String> {
        let env = rig.env();
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        spawn_with(ID, &rig.log_path(), &pairs)
    }

    /// The launcher: the helper runs detached (under `setsid`, which the rig fakes by running its
    /// arguments), `spawn` returns at once, and the helper still finishes its work afterwards.
    #[test]
    fn spawn_starts_the_helper_under_setsid_and_returns() {
        let rig = Rig::new();
        rig.foreground(&[FRONT]);
        assert_eq!(spawn_on(&rig), Ok(()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while rig.launches().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(rig.launches().len(), 1, "the helper ran on its own: {:?}\n{}", rig.calls(), rig.log());
        // setsid got the shell, -c, the script, the name, the id and the log path
        assert_eq!(rig.file("setsid").trim(), "6");
        assert_eq!(payload_of(&rig.launches()[0].1), expected_payload());
    }

    /// No `setsid` on the tool path: the launcher says so (exit 3) and no helper starts.
    #[test]
    fn spawn_reports_a_television_without_setsid() {
        let rig = Rig::new();
        let empty = rig.dir.join("empty");
        fs::create_dir(&empty).unwrap();
        let error = spawn_with(ID, &rig.log_path(), &[("SN_TRAILER_TEST_PATH", empty.to_str().unwrap())]).expect_err("no setsid, no helper");
        assert!(error.contains("launcher exited"), "{error}");
        assert!(!rig.log_path().exists());
    }

    /// Descriptors the app left open (without CLOEXEC) must not reach the helper. The wrapper opens
    /// fds 9 and 12 without CLOEXEC, as the app's display and bus descriptors may be, and then
    /// runs the given shell command; the fake `luna-send` reports which of them it still sees. The
    /// control run (the script without the launcher) proves the probe sees an inherited fd.
    #[test]
    fn the_launcher_closes_inherited_descriptors_and_the_probe_sees_them_without_it() {
        // a shell that cannot redirect fd 12 (dash) cannot hold it open for the test either
        let wide = Command::new("/bin/sh").args(["-c", "exec 12> /dev/null"]).stderr(Stdio::null()).status().unwrap().success();
        let wrap = |rig: &Rig, args: &[&str]| {
            let marker = rig.dir.join("marker");
            Command::new("/bin/sh")
                .arg("-c")
                .arg(if wide { "exec 9> \"$1\"; exec 12> \"$1\"; shift; exec /bin/sh -c \"$@\"" } else { "exec 9> \"$1\"; shift; exec /bin/sh -c \"$@\"" })
                .arg("wrapper")
                .arg(&marker)
                .args(args)
                .envs(rig.env())
                .stdin(Stdio::null())
                .status()
                .unwrap()
        };
        let launcher_args = |rig: &Rig| vec![LAUNCHER.to_string(), "trailer-launcher".into(), SCRIPT.into(), ID.into(), rig.log_path().display().to_string()];
        // control: the helper script straight under the wrapper keeps both fds
        let rig = Rig::new();
        let args = vec![SCRIPT.to_string(), "trailer-helper".into(), ID.into(), rig.log_path().display().to_string()];
        assert!(wrap(&rig, &args.iter().map(String::as_str).collect::<Vec<_>>()).success());
        let seen = rig.file("fds");
        assert!(seen.lines().any(|l| l == "9") && (!wide || seen.lines().any(|l| l == "12")), "the probe sees inherited fds: {seen:?}");
        // through the launcher: none
        let rig = Rig::new();
        let args = launcher_args(&rig);
        assert!(wrap(&rig, &args.iter().map(String::as_str).collect::<Vec<_>>()).success());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while rig.launches().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(rig.launches().len(), 1, "the helper ran: {:?}\n{}", rig.calls(), rig.log());
        assert_eq!(rig.file("fds"), "", "the helper sees none of the inherited descriptors");
    }
}
