//! How much media the player waits for before the clock runs, and how big the background buffer
//! may grow. Pure policy plus one process-wide meter; the pump and `engine::try_prime` act on it.
//!
//! **The meter** reads how fast the demuxer extends the queued tail, in media seconds per wall
//! second (`1.0` = the network keeps up with playback exactly). It only samples while the demuxer
//! is free to read: a lane at its cap, a finished producer or a seek say nothing about the network.
//! It is process-wide because a Reload seek builds a new engine on the same connection.
//!
//! **The fill target** is the runway a start, a seek, a resume or a rebuffer waits for. A network
//! that keeps up needs only [`MIN_FILL_NS`]. A slower one drains the runway at `1 - rate` per
//! second of play, so a wait of `B / rate` buys `B / (1 - rate)` seconds of play: the ratio is
//! fixed by the network, and only the length of each stretch can be chosen. The target aims at
//! [`PLAY_GOAL_S`] of play per stretch without a single wait passing [`MAX_WAIT_S`]. A rebuffer that
//! comes soon after the last one raises a floor under the target; long smooth play lowers it again.

use std::sync::Mutex;
use std::time::Instant;

pub const MIN_FILL_NS: i64 = 4_000_000_000;
pub const UNKNOWN_FILL_NS: i64 = 6_000_000_000;
pub const MAX_FILL_NS: i64 = 30_000_000_000;
const PLAY_GOAL_S: f64 = 60.0;
const MAX_WAIT_S: f64 = 30.0;

/// The runway under which a running clock is stopped to refill.
pub const RUNWAY_LOW_MS: i64 = 1_000;

const WINDOW_MS: u64 = 1_000;
const ALPHA: f64 = 0.25;
const STALE_MS: u64 = 120_000;
/// A tail that moves more than this in one window was repositioned (a seek), not downloaded.
const JUMP_NS: i64 = 30_000_000_000;

/// A rebuffer this soon after the clock last started doubles the floor, when the network has not
/// been measured. A measured one predicts the run instead ([`expected_run_ms`]).
const RELAPSE_MS: u64 = 60_000;
/// This long running without a rebuffer resets the floor.
const SETTLE_MS: u64 = 180_000;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Window {
    t0: u64,
    tail0: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillRate {
    window: Option<Window>,
    ewma: Option<f64>,
    last_sample_ms: u64,
}

impl FillRate {
    pub const fn new() -> Self {
        Self { window: None, ewma: None, last_sample_ms: 0 }
    }

    /// One pump tick: the queued tail (content ns, `-1` when none) and whether the demuxer could
    /// read freely since the last tick.
    pub fn observe(&mut self, now_ms: u64, tail_ns: i64, free: bool) {
        if !free || tail_ns < 0 {
            self.window = None;
            return;
        }
        let Some(w) = self.window else {
            self.window = Some(Window { t0: now_ms, tail0: tail_ns });
            return;
        };
        let dt = now_ms.saturating_sub(w.t0);
        if dt < WINDOW_MS {
            return;
        }
        self.window = Some(Window { t0: now_ms, tail0: tail_ns });
        let dtail = tail_ns - w.tail0;
        if !(0..=JUMP_NS).contains(&dtail) {
            return;
        }
        let r = dtail as f64 / (dt as f64 * 1e6);
        self.ewma = Some(self.ewma.map_or(r, |e| e + ALPHA * (r - e)));
        self.last_sample_ms = now_ms;
    }

    /// Media seconds the network delivers per wall second, if measured in the last two minutes.
    pub fn rate(&self, now_ms: u64) -> Option<f64> {
        self.ewma.filter(|_| now_ms.saturating_sub(self.last_sample_ms) <= STALE_MS)
    }
}

/// The runway to wait for at `rate` (see the module doc), before the relapse floor.
pub fn fill_target_ns(rate: Option<f64>) -> i64 {
    let Some(r) = rate.filter(|r| r.is_finite()) else {
        return UNKNOWN_FILL_NS;
    };
    if r >= 1.0 {
        return MIN_FILL_NS;
    }
    let s = (PLAY_GOAL_S * (1.0 - r)).min(MAX_WAIT_S * r.max(0.0));
    ((s * 1e9) as i64).clamp(MIN_FILL_NS, MAX_FILL_NS)
}

/// How long a clock started on `target_ns` of runway should play before the runway is down to
/// [`RUNWAY_LOW_MS`], when the network delivers `rate` (`None` when it keeps up or is unmeasured).
pub fn expected_run_ms(target_ns: i64, rate: Option<f64>) -> Option<u64> {
    let r = rate.filter(|r| r.is_finite() && *r < 1.0)?.max(0.0);
    let drain_ns = (target_ns - RUNWAY_LOW_MS * 1_000_000).max(0) as f64;
    Some((drain_ns / (1.0 - r) / 1e6) as u64)
}

/// The longest floor a network at `rate` may impose: one wait of [`MAX_WAIT_S`] at most.
fn floor_cap_ns(rate: Option<f64>) -> i64 {
    rate.filter(|r| r.is_finite()).map_or(MAX_FILL_NS, |r| {
        ((MAX_WAIT_S * r.max(0.0) * 1e9) as i64).clamp(MIN_FILL_NS, MAX_FILL_NS)
    })
}

/// Whether a waiting clock may start: the runway reached the target, or nothing more can arrive
/// (the producer finished) or fit (a lane is at its cap). A runway nobody has published (`None`)
/// does not hold the clock: the decoder's own prime depth is then the only wait, as before.
pub fn fill_ready(runway_ns: Option<i64>, target_ns: i64, producer_done: bool, lanes_full: bool) -> bool {
    producer_done || lanes_full || runway_ns.is_none_or(|r| r >= target_ns)
}

/// The floor the relapse rule keeps under the target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Relapse {
    floor_ns: i64,
    started_ms: Option<u64>,
    expected_run_ms: Option<u64>,
}

impl Relapse {
    pub const fn new() -> Self {
        Self { floor_ns: 0, started_ms: None, expected_run_ms: None }
    }

    pub fn floor_ns(&self) -> i64 {
        self.floor_ns
    }

    /// The clock started (or resumed) at `now_ms`, expected to play `expected_run_ms` before its
    /// runway is low (`None`: not predictable).
    pub fn started(&mut self, now_ms: u64, expected_run_ms: Option<u64>) {
        self.started_ms = Some(now_ms);
        self.expected_run_ms = expected_run_ms;
    }

    /// The runway ran out at `now_ms`: a relapse doubles the floor. A run-out the network speed
    /// predicted is not one; a run shorter than half the prediction (the network got slower) is.
    pub fn ran_out(&mut self, now_ms: u64, target_ns: i64) {
        let soon_ms = self.expected_run_ms.map_or(RELAPSE_MS, |e| e / 2);
        let soon = self.started_ms.is_some_and(|t| now_ms.saturating_sub(t) < soon_ms);
        self.floor_ns = if soon {
            self.floor_ns.max(target_ns).saturating_mul(2).min(MAX_FILL_NS)
        } else {
            0
        };
        self.started_ms = None;
    }

    /// A pump tick while running: long smooth play forgets the floor.
    pub fn tick(&mut self, now_ms: u64) {
        if self.started_ms.is_some_and(|t| now_ms.saturating_sub(t) >= SETTLE_MS) {
            self.floor_ns = 0;
        }
    }
}

/// The video lane's byte cap for a new session: the base, raised toward `ceiling` by up to a third
/// of the memory the kernel reports as available. A 4K stream needs ~3 MB per second of runway, and
/// the TV this runs on has ~200 MB available with swap already in use, so memory, not the wanted
/// seconds, is the bound. A third leaves room for the pre-audio headroom granted on top of the cap.
pub fn video_queue_cap(base: i64, ceiling: i64, mem_available: Option<i64>) -> i64 {
    match mem_available {
        Some(m) if m > 0 => (m / 3).clamp(base, ceiling.max(base)),
        _ => base,
    }
}

/// `MemAvailable` from `/proc/meminfo`, in bytes.
pub fn mem_available() -> Option<i64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_mem_available(&text)
}

fn parse_mem_available(text: &str) -> Option<i64> {
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: i64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kb.checked_mul(1024)
}

/// A running clock is not stopped to refill sooner than this after it started: a start the cap or
/// the end of the file released below the low mark must not turn into Pause/Play every tick.
const MIN_RUN_MS: u64 = 2_000;
/// A refill Pause the sink refused is not asked again sooner than this.
const REFUSED_BACKOFF_MS: u64 = 1_000;

/// The policy for the item playing now. A new item (another URL) starts it afresh; a reload of the
/// same item (a seek, an audio switch) keeps it, since the connection and the network are the same.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Policy {
    item: Option<u64>,
    /// A trailer preview is not metered and never waits or refills: it is a muted glimpse.
    metered: bool,
    rate: FillRate,
    relapse: Relapse,
    refused_ms: Option<u64>,
}

impl Policy {
    const fn new() -> Self {
        Self { item: None, metered: true, rate: FillRate::new(), relapse: Relapse::new(), refused_ms: None }
    }

    fn begin_item(&mut self, item: u64, metered: bool) {
        if self.item != Some(item) {
            *self = Self::new();
            self.item = Some(item);
        }
        self.metered = metered;
    }

    fn target_ns(&self, now_ms: u64) -> i64 {
        if !self.metered {
            return 0;
        }
        let rate = self.rate.rate(now_ms);
        fill_target_ns(rate).max(self.relapse.floor_ns().min(floor_cap_ns(rate)))
    }

    fn may_refill(&self, now_ms: u64) -> bool {
        self.metered
            && self.relapse.started_ms.is_some_and(|t| now_ms.saturating_sub(t) >= MIN_RUN_MS)
            && self.refused_ms.is_none_or(|t| now_ms.saturating_sub(t) >= REFUSED_BACKOFF_MS)
    }
}

static POLICY: Mutex<Policy> = Mutex::new(Policy::new());

fn now_ms() -> u64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    // Tests backdate a start by a few seconds: their clock begins well past zero.
    const EPOCH_MS: u64 = if cfg!(test) { 1_000_000 } else { 0 };
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64 + EPOCH_MS
}

fn policy() -> std::sync::MutexGuard<'static, Policy> {
    POLICY.lock().unwrap_or_else(|e| e.into_inner())
}

/// A session starts: `url` names the item, `metered` is false for a trailer preview.
pub fn begin_item(url: &str, metered: bool) {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    policy().begin_item(h.finish(), metered);
}

pub fn observe(tail_ns: i64, free: bool) {
    let now = now_ms();
    let mut p = policy();
    if !p.metered {
        return;
    }
    p.rate.observe(now, tail_ns, free);
    p.relapse.tick(now);
}

pub fn rate() -> Option<f64> {
    policy().rate.rate(now_ms())
}

/// The runway a waiting clock needs now (`0` for a preview: the decoder's prime is the only wait).
pub fn target_ns() -> i64 {
    policy().target_ns(now_ms())
}

/// Whether a running clock may be stopped to refill now.
pub fn may_refill() -> bool {
    policy().may_refill(now_ms())
}

/// The meter for a log line: `0.65x`, or `unmeasured`.
pub fn rate_text(rate: Option<f64>) -> String {
    rate.map_or_else(|| "unmeasured".to_owned(), |r| format!("{r:.2}x"))
}

pub fn note_started() {
    let now = now_ms();
    let mut p = policy();
    let expected = expected_run_ms(p.target_ns(now), p.rate.rate(now));
    p.relapse.started(now, expected);
    p.refused_ms = None;
}

pub fn note_ran_out(target_ns: i64) {
    policy().relapse.ran_out(now_ms(), target_ns);
}

pub fn note_refill_refused() {
    policy().refused_ms = Some(now_ms());
}

/// Tests: forget the policy, as a fresh process would hold it.
#[cfg(test)]
pub fn reset_for_test() {
    *policy() = Policy::new();
}

/// Tests: the clock started `ago_ms` before now.
#[cfg(test)]
pub fn backdate_start_for_test(ago_ms: u64) {
    let now = now_ms();
    policy().relapse.started(now - ago_ms, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(m: &mut FillRate, from_ms: u64, secs: u64, media_per_sec_ns: i64) -> u64 {
        let mut t = from_ms;
        let mut tail = 0;
        for _ in 0..=secs * 10 {
            m.observe(t, tail, true);
            t += 100;
            tail += media_per_sec_ns / 10;
        }
        t
    }

    #[test]
    fn the_meter_reads_media_seconds_per_wall_second() {
        let mut m = FillRate::new();
        let t = feed(&mut m, 0, 10, 650_000_000);
        let r = m.rate(t).expect("measured");
        assert!((r - 0.65).abs() < 0.02, "{r}");
    }

    #[test]
    fn a_blocked_demuxer_or_a_jump_is_not_a_sample() {
        let mut m = FillRate::new();
        m.observe(0, 0, true);
        m.observe(5_000, 1_000_000_000, false);
        m.observe(6_000, 1_000_000_000, true);
        assert_eq!(m.rate(6_000), None, "a window that saw the cap is dropped");
        m.observe(7_000, 400_000_000_000, true);
        assert_eq!(m.rate(7_000), None, "a seek's jump is not throughput");
        m.observe(8_000, 401_000_000_000, true);
        assert!((m.rate(8_000).unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(m.rate(8_000 + STALE_MS + 1), None, "an old measure is forgotten");
    }

    #[test]
    fn the_target_follows_the_network() {
        assert_eq!(fill_target_ns(None), UNKNOWN_FILL_NS);
        assert_eq!(fill_target_ns(Some(2.5)), MIN_FILL_NS, "a fast network starts at the minimum");
        assert_eq!(fill_target_ns(Some(1.0)), MIN_FILL_NS);
        assert_eq!(fill_target_ns(Some(0.97)), MIN_FILL_NS);
        let at_065 = fill_target_ns(Some(0.65));
        assert_eq!(at_065, 19_500_000_000, "a 30 s wait buys ~56 s of play");
        assert_eq!(fill_target_ns(Some(0.05)), MIN_FILL_NS, "a dead link never waits past the floor");
        assert!(fill_target_ns(Some(0.5)) <= MAX_FILL_NS);
        assert_eq!(fill_target_ns(Some(f64::NAN)), UNKNOWN_FILL_NS);
    }

    #[test]
    fn the_clock_starts_when_the_runway_is_ready_or_cannot_grow() {
        assert!(fill_ready(None, MIN_FILL_NS, false, false), "no published tail: the old prime only");
        assert!(!fill_ready(Some(3_999_999_999), MIN_FILL_NS, false, false));
        assert!(fill_ready(Some(MIN_FILL_NS), MIN_FILL_NS, false, false));
        assert!(fill_ready(Some(0), MIN_FILL_NS, true, false), "the end of the file");
        assert!(fill_ready(Some(0), MIN_FILL_NS, false, true), "a lane at its cap");
    }

    #[test]
    fn a_relapse_raises_the_floor_and_smooth_play_drops_it() {
        let mut r = Relapse::new();
        r.started(0, None);
        r.ran_out(20_000, MIN_FILL_NS);
        assert_eq!(r.floor_ns(), 2 * MIN_FILL_NS);
        r.started(30_000, None);
        r.ran_out(40_000, 2 * MIN_FILL_NS);
        assert_eq!(r.floor_ns(), 4 * MIN_FILL_NS);
        r.started(50_000, None);
        r.ran_out(55_000, MAX_FILL_NS);
        assert_eq!(r.floor_ns(), MAX_FILL_NS, "capped");
        r.started(60_000, None);
        r.tick(60_000 + SETTLE_MS);
        assert_eq!(r.floor_ns(), 0);
        r.started(0, None);
        r.ran_out(RELAPSE_MS + 1, MIN_FILL_NS);
        assert_eq!(r.floor_ns(), 0, "a run-out long after the start is not a relapse");
    }

    /// The audit's arithmetic: at 0.65x a 19.5 s runway plays ~53 s before it is low. That run-out
    /// is the plan, not a relapse; one at 20 s is (the network got slower).
    #[test]
    fn a_predicted_run_out_is_not_a_relapse_and_the_floor_never_waits_past_the_cap() {
        let target = fill_target_ns(Some(0.65));
        let expected = expected_run_ms(target, Some(0.65)).unwrap();
        assert!((52_000..54_000).contains(&expected), "{expected}");
        assert_eq!(expected_run_ms(target, Some(1.2)), None, "a network that keeps up predicts nothing");
        let mut r = Relapse::new();
        r.started(0, Some(expected));
        r.ran_out(expected, target);
        assert_eq!(r.floor_ns(), 0, "the planned run-out");
        r.started(100_000, Some(expected));
        r.ran_out(120_000, target);
        assert_eq!(r.floor_ns(), MAX_FILL_NS.min(2 * target));

        let mut p = Policy::new();
        p.relapse.floor_ns = MAX_FILL_NS;
        p.rate.ewma = Some(0.5);
        p.rate.last_sample_ms = 0;
        assert_eq!(p.target_ns(0), 15_000_000_000, "a 30 s floor at 0.5x would wait 60 s; capped to a 30 s wait");
    }

    #[test]
    fn the_video_cap_grows_with_free_memory_inside_its_bounds() {
        const MIB: i64 = 1024 * 1024;
        assert_eq!(video_queue_cap(48 * MIB, 96 * MIB, None), 48 * MIB);
        assert_eq!(video_queue_cap(48 * MIB, 96 * MIB, Some(210 * MIB)), 70 * MIB);
        assert_eq!(video_queue_cap(48 * MIB, 96 * MIB, Some(60 * MIB)), 48 * MIB);
        assert_eq!(video_queue_cap(48 * MIB, 96 * MIB, Some(4000 * MIB)), 96 * MIB);
    }

    #[test]
    fn a_new_item_starts_the_policy_afresh_and_a_reload_keeps_it() {
        let mut p = Policy::new();
        p.begin_item(1, true);
        p.relapse.started(0, None);
        p.relapse.ran_out(10_000, MIN_FILL_NS);
        assert_eq!(p.target_ns(10_000), 2 * MIN_FILL_NS);
        p.begin_item(1, true);
        assert_eq!(p.target_ns(10_000), 2 * MIN_FILL_NS, "a reload of the same item keeps its floor");
        p.begin_item(2, true);
        assert_eq!(p.target_ns(10_000), UNKNOWN_FILL_NS, "another item starts unmeasured");
        p.begin_item(3, false);
        assert_eq!(p.target_ns(10_000), 0, "a preview never waits for a runway");
    }

    #[test]
    fn a_refill_waits_for_a_minimum_run_and_backs_off_a_refusal() {
        let mut p = Policy::new();
        assert!(!p.may_refill(0), "a clock that never started has nothing to stop");
        p.relapse.started(1_000, None);
        assert!(!p.may_refill(1_000 + MIN_RUN_MS - 1));
        assert!(p.may_refill(1_000 + MIN_RUN_MS));
        p.refused_ms = Some(5_000);
        assert!(!p.may_refill(5_000 + REFUSED_BACKOFF_MS - 1));
        assert!(p.may_refill(5_000 + REFUSED_BACKOFF_MS));
        p.relapse.ran_out(9_000, MIN_FILL_NS);
        assert!(!p.may_refill(20_000), "a held clock refills once, not again until it restarts");
        p.begin_item(4, false);
        p.relapse.started(0, None);
        assert!(!p.may_refill(60_000), "a preview never refills");
    }

    #[test]
    fn meminfo_parses() {
        let text = "MemTotal:         881516 kB\nMemFree:           69280 kB\nMemAvailable:     207884 kB\n";
        assert_eq!(parse_mem_available(text), Some(207_884 * 1024));
        assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
    }
}
