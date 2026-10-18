//! **Stats for nerds** — the on-screen technical read-out, toggled from the player's `…` menu (its
//! last row) and from Settings' About section. Ported from PlxNative's `app::diagnostics`; the
//! design is that module's, and so are the reasons.
//!
//! # Why it exists, and why it ships in release builds
//!
//! It is a support channel. The app runs on televisions nobody here owns, and every other
//! diagnostic surface (the `/tmp` triggers, the event log over ssh) needs a rooted set or a
//! `devtriggers` build. A panel a stranger can open with the remote and photograph with a phone
//! needs neither, so it ships in RELEASE builds, opens no path, listens on no socket and reads no
//! trigger. It is a product feature, not a debug surface.
//!
//! # The photograph is the output format
//!
//! * **Complete in a phone photo.** The dense `size::DIAGNOSTIC` face on the opaque panel ground;
//!   no value is elided (`widgets::FieldList` wraps instead). Severity is a WORD in the value
//!   (capitals: `REFUSED`, `FROZEN`, `NO WINDOW`) as well as the fault tint, because a phone
//!   camera chroma-subsamples the colour away.
//! * **Not the screen.** A wide, shallow card in the overscan-safe top-left, above the transport,
//!   with two fixed columns: stream/output on the left, delivery/control on the right. A mode
//!   (HLS, progressive, torrent) changes values in place, never the row set, so two photographs
//!   compare by y-coordinate. It never scrolls.
//! * **Still.** Values are sampled every [`SAMPLE_MS`] and held, not read per frame: a number that
//!   changes between viewfinder and shutter cannot be trusted, and re-rendering ~20 volatile
//!   strings a frame would thrash the whole-string glyph cache. The sample is also the only
//!   repaint the panel asks for: off, it costs nothing; on and idle, the app presents at 2 Hz.
//!
//! # It takes no keys
//!
//! It is turned off the way it was turned on, so every transport key keeps working under it —
//! watching Feed and Frames move as you press play is how a wedged seek is told from a wedged
//! load. It is drawn after the whole tree, but **skipped** (not reordered) while any menu is up:
//! the `…` menu carries its own off-switch, and a panel over it would hide it ([`visible`]).
//!
//! # What may never appear on it
//!
//! A photograph lands in a public issue thread and cannot be redacted after the fact, so the
//! panel is a strict subset of the event log, structurally:
//!
//! * **No URL or path.** An addon puts debrid keys and its configuration in the stream URL's path
//!   and query. The Source row is decomposed by [`source_host`] into the server's KIND (this TV,
//!   the local network, a bare public address) or, for a named public host, its registrable domain
//!   only (`real-debrid.com`, never the subdomain, which can carry a user id); the transport and
//!   container are read from the scheme and the extension. The address itself never reaches a row.
//! * **No credential, no identity.** No header, no account, no title of what plays: only its
//!   technical shape. Server text that does reach a row (the addon's name, an HLS failure) goes
//!   through `sn_base::eventlog::scrub::scrub_local` first.
//!
//! Before anything has played (`never_played`) the pipeline rows would all read zero and look
//! like a broken app, so the panel shows the device card instead: what the set is, its firmware,
//! what its decoder table claims and whether it was read at all, and its Dolby Vision answer.

use std::ffi::CString;
use std::sync::atomic::Ordering::Relaxed;

use sn_media::player::video_geometry::AspectMode;
use sn_media::player::{self, Diag, PlaybackState, DIAG_READOUT_ON as ON};
use sn_media::stream::ProbedStreams;
use sn_platform::i18n::msg;
use sn_ui::label::Label;
use sn_ui::widgets::{Field, FieldList, FIELD_COL_W, FIELD_ROW_H};
use sn_ui::label::HAlign;
use sn_ui::{theme, Env, Painter, Rect, View};

/// Is the read-out on? Not persisted: a diagnostic overlay that survives a restart is one a user
/// can strand themselves with.
pub fn enabled() -> bool {
    ON.load(Relaxed)
}

/// Flip it. The next frame's [`Diagnostics::update`] sees the rising edge and samples at once.
pub fn toggle() {
    ON.fetch_xor(true, Relaxed);
    sn_machine::idle::invalidate();
}

/// Switch it off: a playback ending takes it with it, so it never survives into the next one.
pub fn close() {
    if ON.swap(false, Relaxed) {
        sn_machine::idle::invalidate();
    }
}

/// Is the panel drawn this frame? On, and no menu over the page: a menu (the player's `…` menu
/// carries the off-switch) is never covered by it.
pub fn visible(pages: &sn_ui::dispatch::Dispatcher<sn_screens::registry::Bundle>) -> bool {
    enabled() && pages.nav.modals.surfaces.is_empty()
}

/// How often the values are re-read. **Not a render rate**: the panel draws every presented frame
/// from the held snapshot.
pub const SAMPLE_MS: u32 = 500;
/// The two columns' fixed row counts.
const LEFT_ROWS: usize = 9;
const RIGHT_ROWS: usize = 9;
/// A still picture this long while playing is a stall worth counting.
const STALL_COUNT_MS: u32 = 2_000;
/// …and this long, the verdict line says so.
const STALL_MS: u32 = 8_000;
/// A fed lead larger than this is one lane starving, not ordinary interleave.
const SKEW_FAULT_MS: i64 = 3_000;

/// The previous sample's totals, which turn two totals into a rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Prev {
    fed_v: i64,
    fed_a: i64,
    net_rx: i64,
    /// Loop ms of the sample; 0 = no previous sample.
    at: u32,
}

/// The stalls this playback, counted at the sample cadence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Stalls {
    /// The `load_at` stamp of the session counted: a new one restarts the count.
    session: u32,
    count: u32,
    frozen: bool,
}

/// What a sample reads beyond the media layer's [`Diag`]: the session's facts and the viewer's.
pub struct Facts<'a> {
    pub d: &'a Diag,
    pub state: PlaybackState,
    pub paused: bool,
    pub url: &'a str,
    pub addon: &'a str,
    pub probe: Option<&'a ProbedStreams>,
    pub audio_track: i32,
    /// The failure read-out's reason while the state is Error, else "".
    pub error: &'a str,
    /// The last HLS failure, as text.
    pub hls_error: Option<String>,
    pub aspect: AspectMode,
    /// When the panel was switched on (loop ms): a frame clock older than that was not observed.
    pub on_since: u32,
}

/// The read-out's own state: one `App` field, main-thread only.
#[derive(Default)]
pub struct Diagnostics {
    next_sample: u32,
    was_on: bool,
    on_since: u32,
    prev: Prev,
    stalls: Stalls,
    /// The two header lines (identity, verdict) and their wrapped form.
    head: [String; 2],
    head_lines: [Vec<String>; 2],
    columns: [Vec<Field>; 2],
    /// The device card ([`device_rows`]), while nothing has played.
    rows: Vec<Field>,
    idle: bool,
    /// The last minute of buffer, queue and network samples ([`Sweep`]) and the lanes' held values.
    sweep: Sweep,
    chart_vals: [String; 4],
}

impl Diagnostics {
    /// Is a sample due? The rising edge of the switch is one at once; then every [`SAMPLE_MS`].
    /// Off, never (and nothing is repainted).
    fn due(&mut self, on: bool, now: u32) -> bool {
        let woke = on && !self.was_on;
        self.was_on = on;
        if woke {
            self.on_since = now;
            self.prev = Prev::default();
            // cells from before the switch-off would sit at x-positions nobody watched
            self.sweep = Sweep::default();
        }
        if !on || (!woke && now.wrapping_sub(self.next_sample) > u32::MAX / 2) {
            return false;
        }
        self.next_sample = now.wrapping_add(SAMPLE_MS);
        true
    }

    /// Re-sample if the hold has expired. Called once a frame, after the playback pump.
    pub fn update(&mut self, playback: &super::playback::Playback, now: u32) {
        if !self.due(enabled(), now) {
            return;
        }
        // ONE read feeding the whole panel: a row must not report "no frames" beside a position
        // taken a moment later
        let d = player::diag();
        let ps = &playback.player.session;
        let state = player::state(ps);
        let f = Facts {
            d: &d,
            state,
            paused: player::TX.paused.load(Relaxed),
            url: ps.url(),
            addon: playback.addon(),
            probe: ps.probed(),
            audio_track: ps.audio_track(),
            error: if state == PlaybackState::Error { player::error_reason(ps) } else { "" },
            hls_error: sn_media::hls::last_failure().map(|e| e.to_string()),
            aspect: player::aspect_mode(),
            on_since: self.on_since,
        };
        self.sample(&f, now);
        sn_machine::idle::invalidate();
    }

    /// Build the held snapshot from one read.
    fn sample(&mut self, f: &Facts<'_>, now: u32) {
        self.head = [identity_line(), verdict_line(f, now)];
        self.idle = never_played(f.d, f.state);
        self.head_lines = wrap_header(&self.head, self.panel_width() - 2.0 * PAD);
        if self.idle {
            self.rows = device_rows();
            self.columns = [Vec::new(), Vec::new()];
        } else {
            self.stalls = count_stall(self.stalls, f, now);
            self.columns = [stream_rows(f), delivery_rows(f, self.prev, self.stalls, now)];
            self.rows = Vec::new();
            self.sweep.record(f.d, self.prev, now);
            self.chart_vals = self.sweep.values(f.d);
        }
        self.prev = Prev { fed_v: f.d.fed_v, fed_a: f.d.fed_a, net_rx: f.d.net_rx, at: now.max(1) };
    }
}

/// Nothing has been asked to play this session: `Idle` and no Load ever sent. A playback that
/// ended or failed also rests at `Idle`, and there the pipeline rows are the post-mortem.
fn never_played(d: &Diag, st: PlaybackState) -> bool {
    st == PlaybackState::Idle && d.load_at == 0
}

fn since(at: u32, now: u32) -> u32 {
    if at == 0 || now < at {
        0
    } else {
        now - at
    }
}

/// How long the picture has been still while it should move, if it is. Measured from the later
/// of the last frame and the panel's switch-on (a frame clock is only kept while it is on).
fn frozen_ms(f: &Facts<'_>, now: u32) -> Option<u32> {
    let d = f.d;
    let moving = f.state == PlaybackState::Playing && !f.paused && d.seen_frame;
    let still = since(d.frame_at.max(f.on_since), now);
    (moving && still >= STALL_COUNT_MS).then_some(still)
}

/// One more stall on the edge into a freeze; a new session starts the count again.
fn count_stall(mut s: Stalls, f: &Facts<'_>, now: u32) -> Stalls {
    if s.session != f.d.load_at {
        s = Stalls { session: f.d.load_at, ..Stalls::default() };
    }
    let frozen = frozen_ms(f, now).is_some();
    if frozen && !s.frozen {
        s.count += 1;
    }
    s.frozen = frozen;
    s
}

// ---- the header -------------------------------------------------------------------------------

/// Who this build is, on what set and screen.
fn identity_line() -> String {
    let w = sn_platform::tv::device::info();
    let os = if w.major == 0 { msg::browse_diagnostics_unknown_os().to_string() } else { format!("webOS {} · api {}", w.release, w.api) };
    let (_, _, vw, vh) = sn_base::surface::viewport();
    let build = if cfg!(feature = "devtriggers") { msg::app_stats_build_dev() } else { msg::app_stats_build_release() };
    msg::app_stats_header(build, sn_platform::devcaps::dv::capability().compact_display(), &vh.to_string(), &os, crate::VERSION, &vw.to_string())
}

/// What the pipeline thinks it is doing, with the failure's reason and a stall the state hides.
fn verdict_line(f: &Facts<'_>, now: u32) -> String {
    let s = match f.state {
        PlaybackState::Idle | PlaybackState::Paused => msg::app_stats_idle(),
        PlaybackState::Resolving => msg::app_stats_resolving(),
        PlaybackState::Connecting => msg::app_stats_connecting(),
        PlaybackState::Buffering => msg::app_stats_buffering(),
        PlaybackState::Seeking => msg::app_stats_seeking(),
        PlaybackState::Playing => msg::app_stats_playing(),
        PlaybackState::Error => msg::app_stats_error(),
    };
    if f.state == PlaybackState::Error {
        return match clean(f.error) {
            why if why.is_empty() => s.to_string(),
            why => format!("{s} — {why}"),
        };
    }
    if f.paused {
        return msg::app_stats_paused(s);
    }
    match frozen_ms(f, now) {
        Some(ms) if ms >= STALL_MS => msg::app_stats_stalled(i64::from(ms / 1000), s),
        _ => s.to_string(),
    }
}

// ---- the device card --------------------------------------------------------------------------

/// What this SET is: the rows a "it does not work on my TV" report needs before anything plays.
/// A model and a board are product identities shared by every unit LG built, not a household's.
fn device_rows() -> Vec<Field> {
    let hw = sn_platform::tv::device::device();
    let i = sn_platform::tv::device::info();
    let c = sn_platform::devcaps::caps();
    let set = hw.set_line();
    let firmware = match (i.name.as_str(), i.codename.as_str()) {
        ("", "") => msg::browse_diagnostics_unknown().to_string(),
        ("", cn) => cn.to_string(),
        (n, "") => n.to_string(),
        (n, cn) => format!("{n} · {cn}"),
    };
    let measured = sn_platform::devcaps::measured();
    let dv = sn_platform::devcaps::dv::probe();
    vec![
        Field::new(msg::app_stats_f_set(), if set.is_empty() { msg::app_stats_unknown_device().to_string() } else { set.clone() })
            .fault(set.is_empty()),
        Field::new(msg::app_stats_f_firmware(), firmware),
        Field::new(msg::app_stats_f_decoder(), decoder_summary(c, measured)).fault(!measured),
        Field::new(msg::app_stats_f_audio(), c.audio.clone()),
        Field::new(msg::app_stats_f_dolby_vision(), dv.full_state())
            .fault(dv.capability == sn_platform::devcaps::dv::DvCapability::Unknown),
    ]
}

/// `HEVC WxH · VP9 · [AV1 ·] device table | ASSUMED`: what the decoder table claims, and whether
/// it was read at all (an unreadable table falls back to an assumed profile).
fn decoder_summary(c: &sn_platform::devcaps::Caps, measured: bool) -> String {
    let hevc = if c.hevc { format!("HEVC {}x{}", c.hevc_max.0, c.hevc_max.1) } else { msg::app_stats_no_codec("HEVC") };
    let vp9 = if c.vp9 { "VP9".to_string() } else { msg::app_stats_no_codec("VP9") };
    let source = if measured { msg::app_stats_device_table() } else { msg::app_stats_assumed() };
    if c.av1 {
        format!("{hevc} · {vp9} · AV1 · {source}")
    } else {
        format!("{hevc} · {vp9} · {source}")
    }
}

// ---- the stream / output column ---------------------------------------------------------------

/// Server text made safe for a photograph.
fn clean(s: &str) -> String {
    sn_base::eventlog::scrub::scrub_local(s.trim())
}

/// Where the stream comes from, as a KIND — never the address. `None` for no parseable host.
///
/// The userinfo, port, path and query are dropped first (an addon's debrid key and configuration
/// live there); an IP literal is only ever its class; a named public host is cut to its
/// registrable domain, so a per-user subdomain never shows.
pub fn source_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    }
    .trim_end_matches('.')
    .to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    if host == "localhost" || host == "::1" || host.starts_with("127.") {
        return Some(msg::app_stats_this_tv().to_string());
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let local = ip.is_private() || ip.is_link_local();
        return Some(if local { msg::app_stats_local_network() } else { msg::app_stats_public_address() }.to_string());
    }
    if host.contains(':') {
        // IPv6: fc00::/7 unique-local and fe80::/10 link-local are the home network
        let local = host.starts_with("fc") || host.starts_with("fd") || host.starts_with("fe8");
        return Some(if local { msg::app_stats_local_network() } else { msg::app_stats_public_address() }.to_string());
    }
    if host.ends_with(".local") || host.ends_with(".lan") || host.ends_with(".home.arpa") || !host.contains('.') {
        return Some(msg::app_stats_local_network().to_string());
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    // ponytail: a two-letter country TLD under a short second level (co.uk, com.br) keeps three
    // labels; a full public-suffix list would be exact, and this is a read-out, not a cookie jar
    let keep = match labels.as_slice() {
        [.., sld, tld] if tld.len() == 2 && sld.len() <= 3 && labels.len() >= 3 => 3,
        _ => 2,
    };
    Some(labels[labels.len().saturating_sub(keep)..].join("."))
}

/// The addon and the server's kind.
fn source_line(addon: &str, url: &str) -> String {
    let addon = clean(addon);
    match (addon.is_empty(), source_host(url)) {
        (false, Some(h)) => format!("{addon} · {h}"),
        (false, None) => addon,
        (true, Some(h)) => h,
        (true, None) => "—".to_string(),
    }
}

/// Stremio's streaming server (the torrent engine) serves on 11470 and names the torrent by its
/// 40-hex info hash in the path.
fn is_streaming_server(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = path.split(['?', '#']).next().unwrap_or("");
    authority.ends_with(":11470") || path.split('/').any(|seg| seg.len() == 40 && seg.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// How the bytes come: HLS (live or not), a torrent through the streaming server, or one
/// progressive file, with the scheme.
fn transport_line(url: &str, dur_ns: i64) -> String {
    let scheme = url.split_once("://").map_or("", |(s, _)| s).to_ascii_uppercase();
    let kind = if sn_media::hls::is_playlist_url(url) {
        if dur_ns <= 0 {
            format!("HLS · {}", msg::app_stats_live())
        } else {
            "HLS".to_string()
        }
    } else if is_streaming_server(url) {
        msg::app_stats_torrent().to_string()
    } else {
        msg::app_stats_progressive().to_string()
    };
    if scheme.is_empty() {
        kind
    } else {
        format!("{kind} · {scheme}")
    }
}

/// The container as the URL's extension names it (`MKV`), else that it does not.
fn container_line(url: &str) -> String {
    let path = url.split_once("://").map_or(url, |(_, r)| r);
    let path = path.split(['?', '#']).next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((_, ext)) if (2..=5).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphanumeric()) && path.contains('/') => {
            ext.to_ascii_uppercase()
        }
        _ => msg::app_stats_not_in_url().to_string(),
    }
}

/// The codec's journey: what the file is → what the Load declared to the TV.
fn chain(src: &str, payload: &str) -> String {
    let src = if src.is_empty() { "—" } else { src };
    format!("{src} → {payload}")
}

fn stream_rows(f: &Facts<'_>) -> Vec<Field> {
    let d = f.d;
    let video = f.probe.and_then(|p| p.video.as_ref());
    let audio = f.probe.and_then(|p| usize::try_from(f.audio_track).ok().and_then(|i| p.audio.get(i)).or(p.audio.first()));
    let mut v = Vec::with_capacity(LEFT_ROWS);
    v.push(Field::new(msg::app_stats_f_source(), source_line(f.addon, f.url)));
    v.push(Field::new(msg::app_stats_f_transport(), transport_line(f.url, d.dur_ns)));
    v.push(Field::new(msg::app_stats_f_container(), container_line(f.url)));

    let mut vline = chain(video.map_or("", |p| p.codec.as_str()), d.load_v_str());
    if let Some(dv) = video.map(|p| &p.dovi).filter(|dv| dv.present) {
        vline.push_str(&format!(" · Dolby Vision P{}.{}", dv.profile, dv.bl_compat));
    }
    v.push(Field::new(msg::app_stats_f_video(), vline));

    let src_a = audio.map(|a| if a.channels > 0 { format!("{} {}ch", a.codec, a.channels) } else { a.codec.clone() }).unwrap_or_default();
    let mut aline = chain(&src_a, d.load_a_str());
    // Atmos is declared (`contents.immersive`) only over an E-AC-3 ("AC3 PLUS", code 2) feed
    if audio.is_some_and(|a| a.atmos) && d.load_a == 2 {
        aline.push_str(" · Dolby Atmos");
    }
    v.push(Field::new(msg::app_stats_f_audio(), aline).fault(d.load_a == 0 && d.load_v != 0));

    // the frame rate is a CLAIM, and the row says whose: the pipeline's sourceInfo, else the file
    let probe_fps = video.map_or(0.0, |p| p.fps);
    let (fps_milli, fps_src) = if d.video_fps_milli > 0 {
        (d.video_fps_milli, msg::app_stats_pipeline())
    } else if probe_fps > 0.0 {
        ((probe_fps * 1000.0).round() as i64, msg::app_stats_declared())
    } else {
        (0, "")
    };
    let picture = match (d.video_w, d.video_h) {
        (0, _) | (_, 0) => msg::app_stats_not_opened().to_string(),
        (w, h) if fps_milli > 0 => msg::app_stats_fps(&fps_str(fps_milli), &h.to_string(), fps_src, &w.to_string()),
        (w, h) => msg::app_stats_fps_unknown(&h.to_string(), &w.to_string()),
    };
    // the viewer's aspect mode closes the Picture row: how the picture is placed on the screen
    let mode = sn_screens::player::tracks::aspect_labels()[usize::from(f.aspect.index())];
    v.push(
        Field::new(msg::app_stats_f_picture(), format!("{picture} · {mode}"))
            .fault(d.load_completed && (d.video_w == 0 || d.video_h == 0)),
    );

    let dur = if d.dur_ns > 0 { sn_ui::fmt::clock(d.dur_ns / 1_000_000) } else { msg::app_stats_live().to_string() };
    v.push(Field::new(msg::app_stats_f_timeline(), format!("{} / {dur}", sn_ui::fmt::clock(d.pos_ns / 1_000_000))));
    v.push(Field::new(msg::app_stats_f_sync(), skew(d)).fault(skew_bad(d)));
    let (plane, bad) = plane_line(d);
    v.push(Field::new(msg::app_stats_f_plane(), plane).fault(bad));
    debug_assert_eq!(v.len(), LEFT_ROWS);
    v
}

/// `23.976`, `25`: three places with the trailing zeros dropped, in the UI's numerals.
fn fps_str(fps_milli: i64) -> String {
    if fps_milli % 1000 == 0 {
        sn_platform::i18n::current().number(fps_milli / 1000)
    } else {
        sn_platform::i18n::current().decimal(fps_milli, 3).trim_end_matches('0').to_string()
    }
}

/// The FEED lead: highest fed video PTS minus highest fed audio PTS — how far apart the lanes
/// were handed over, not lip-sync (the pipeline's audio-master clock aligns presentation).
fn skew(d: &Diag) -> String {
    if d.fed_v_pts == 0 && d.fed_a_pts == 0 {
        return "—".to_string();
    }
    let ms = (d.fed_v_pts - d.fed_a_pts) / 1_000_000;
    msg::core_seconds(&sn_ui::fmt::signed_decimal(ms as f64 / 1000.0, 1))
}

fn skew_bad(d: &Diag) -> bool {
    (d.fed_v_pts != 0 || d.fed_a_pts != 0) && ((d.fed_v_pts - d.fed_a_pts) / 1_000_000).abs() > SKEW_FAULT_MS
}

/// The video plane's state in one sentence, and whether it is a fault. The compositor's window
/// id is reduced to whether there is one.
fn plane_line(d: &Diag) -> (String, bool) {
    match d.vp_mode {
        player::VP_EXPORTED => {
            let win = if d.window_id.is_empty() { msg::app_stats_no_window() } else { msg::app_stats_window_ready() };
            let placed = match d.place_rv {
                i32::MIN => msg::app_stats_not_placed().to_string(),
                0 => msg::app_stats_placement_failed().to_string(),
                rv => msg::app_stats_placed(&d.placed_h.to_string(), &rv.to_string(), &d.placed_w.to_string()),
            };
            (
                format!("{} · {win} · {placed}", msg::app_stats_exported_window()),
                d.window_id.is_empty() || d.place_rv == i32::MIN || d.place_rv == 0,
            )
        }
        player::VP_ACB => {
            let stage = match (d.acb_ok, d.stage) {
                (false, _) => msg::app_stats_unavailable(),
                (true, 0) => msg::app_stats_unbound(),
                (true, 1) => msg::app_stats_binding(),
                _ => msg::app_stats_streaming(),
            };
            (format!("ACB · {stage}"), !d.acb_ok)
        }
        _ => (msg::app_stats_no_video_path().to_string(), true),
    }
}

// ---- the delivery / control column ------------------------------------------------------------

fn delivery_rows(f: &Facts<'_>, prev: Prev, stalls: Stalls, now: u32) -> Vec<Field> {
    let d = f.d;
    let mut v = Vec::with_capacity(RIGHT_ROWS);
    let transfer = match d.http_status {
        0 => msg::app_stats_no_connection().to_string(),
        st => msg::app_stats_received(&sn_ui::fmt::bytes(d.net_rx, (2, 1, 0)), &st.to_string()),
    };
    v.push(Field::new(msg::app_stats_f_transfer(), transfer).fault(d.http_status != 0 && !(200..300).contains(&d.http_status)));
    v.push(Field::new(msg::app_stats_f_throughput(), throughput(d, prev, now)));

    let load = if d.load_failed {
        msg::app_stats_refused()
    } else if d.load_completed {
        msg::app_stats_completed()
    } else {
        msg::app_stats_waiting()
    };
    let events = match (d.cb_count, d.cb_err) {
        (0, _) => msg::app_stats_no_callbacks().to_string(),
        (n, 0) => msg::app_stats_callbacks(i64::from(n)),
        (n, e) => msg::app_stats_callback_error(&d.cb_err_at.to_string(), &e.to_string(), i64::from(n)),
    };
    v.push(
        Field::new(msg::app_stats_f_load(), format!("{load} · {events}"))
            .fault(d.load_failed || d.cb_err != 0 || (d.cb_count == 0 && d.load_completed)),
    );

    let fed = if d.pushed_any { fed_rate(d, prev, now) } else { msg::app_stats_nothing_demuxed().to_string() };
    v.push(
        Field::new(msg::app_stats_f_feed(), format!("{fed} · {}", feed_state(d)))
            .fault(!d.pushed_any && d.load_completed || (d.fed_v == 0 && d.load_completed) || d.feed_is_fault()),
    );
    let (cv, ca) = player::aq_caps();
    let mb = |b: i64, scale: i16| sn_ui::fmt::decimal(b as f64 / f64::from(1u32 << 20), scale);
    v.push(Field::new(msg::app_stats_f_queues(), msg::app_stats_queues(&mb(d.aq_audio, 2), &mb(ca, 1), &mb(cv, 1), &mb(d.aq_video, 1))));
    let (buffer, low) = buffer_text(d.playable_buffer_ms);
    v.push(Field::new(msg::app_stats_f_buffer(), buffer).fault(low));
    v.push(Field::new(msg::app_stats_f_frames(), frames_str(f, now)).fault(!d.seen_frame && d.load_completed && since(d.load_at, now) > STALL_MS));
    let frozen = frozen_ms(f, now);
    let stall = match frozen {
        Some(ms) => msg::app_stats_stalls_now(i64::from(stalls.count), &(ms / 1000).to_string()),
        None => msg::app_stats_stalls(i64::from(stalls.count)),
    };
    v.push(Field::new(msg::app_stats_f_stalls(), stall).fault(frozen.is_some()));
    let error = if f.state == PlaybackState::Error {
        Some(clean(f.error))
    } else {
        f.hls_error.as_deref().map(clean)
    }
    .filter(|e| !e.is_empty());
    v.push(Field::new(msg::app_stats_f_error(), error.clone().unwrap_or_else(|| msg::app_stats_no_error().to_string())).fault(error.is_some()));
    debug_assert_eq!(v.len(), RIGHT_ROWS);
    v
}

/// Network throughput: the last sample's, and the mean since the Load, localized bit rates.
fn throughput(d: &Diag, prev: Prev, now: u32) -> String {
    let avg_kbps = kbps(d.net_rx, since(d.load_at, now));
    msg::app_stats_rate(&show_kbps(avg_kbps), &show_kbps(now_kbps(d, prev, now)))
}

/// Bytes over milliseconds as kbit/s; `None` for no interval.
fn kbps(bytes: i64, ms: u32) -> Option<i64> {
    (ms > 0).then(|| bytes.max(0) * 8 / i64::from(ms))
}

/// The rate since the previous sample. A counter that went DOWN was reset by a reload inside the
/// interval, and what it holds now is still bytes received in it.
fn now_kbps(d: &Diag, prev: Prev, now: u32) -> Option<i64> {
    if prev.at == 0 || now <= prev.at {
        return None;
    }
    let bytes = if d.net_rx >= prev.net_rx { d.net_rx - prev.net_rx } else { d.net_rx };
    kbps(bytes, now - prev.at)
}

fn show_kbps(k: Option<i64>) -> String {
    k.map_or_else(|| "—".to_string(), sn_ui::fmt::bitrate)
}

/// Below this much playable stream ahead, the reserve is LOW: one hiccup of the source from a
/// stall. Two seconds is the pipeline's own audio lead-in, the least it starts with.
const BUFFER_LOW_MS: i64 = 2_000;

/// The Buffer row and lane value, and whether it is a fault: `LOW` and `EMPTY` in words.
fn buffer_text(ms: Option<i64>) -> (String, bool) {
    match ms {
        None => (msg::app_stats_not_yet().to_string(), false),
        Some(ms) if ms <= 0 => (msg::app_stats_buffer_empty().to_string(), true),
        Some(ms) => {
            let secs = sn_ui::fmt::decimal(ms as f64 / 1000.0, 1);
            if ms < BUFFER_LOW_MS {
                (msg::app_stats_buffer_low(&secs), true)
            } else {
                (msg::app_stats_buffer_ahead(&secs), false)
            }
        }
    }
}

/// Access units per second per lane since the previous sample; `—` before there is one.
fn fed_rate(d: &Diag, prev: Prev, now: u32) -> String {
    if prev.at == 0 || now <= prev.at {
        return "—".to_string();
    }
    let dt = f64::from(now - prev.at) / 1000.0;
    let per = |n: i64, p: i64| ((n - p).max(0) as f64 / dt).round() as i64;
    let l = sn_platform::i18n::current();
    msg::app_stats_fed_rate(&l.number(per(d.fed_a, prev.fed_a)), &l.number(per(d.fed_v, prev.fed_v)))
}

/// The video feeder's state in the UI language. Only an outright refusal is a fault.
fn feed_state(d: &Diag) -> &'static str {
    match d.feed_state {
        1 => msg::app_stats_feed_accepting(),
        2 => msg::app_stats_feed_full(),
        3 => msg::app_stats_refused(),
        4 => msg::app_stats_feed_holding(),
        5 => msg::app_stats_feed_empty(),
        _ => msg::app_stats_feed_none(),
    }
}

/// The frame count with its clock: `frames` is seek-scoped, so "0" has three meanings. Paused,
/// a still count is the pause button, never "frozen".
fn frames_str(f: &Facts<'_>, now: u32) -> String {
    let d = f.d;
    if f.paused {
        return if d.frames == 0 && !d.seen_frame { msg::app_stats_none_yet().to_string() } else { d.frames.to_string() };
    }
    let stuck = since(d.frame_at.max(d.load_at).max(f.on_since), now) / 1000;
    match (d.frames, d.seen_frame) {
        (0, false) if d.load_completed => msg::app_stats_frames_none(&stuck.to_string()),
        (0, false) => msg::app_stats_none_yet().to_string(),
        (0, true) => msg::app_stats_since_seek().to_string(),
        (n, _) if f.state == PlaybackState::Playing && stuck >= STALL_MS / 1000 => {
            msg::app_stats_frames_frozen(&n.to_string(), &stuck.to_string())
        }
        (n, _) => n.to_string(),
    }
}

// ---- the sweep chart ---------------------------------------------------------------------------

/// The chart's cells: one a sample, so a minute at the 2 Hz [`SAMPLE_MS`].
const HIST_N: usize = 120;

/// One sample of the chart. `-1` is "not observed", which draws nothing — distinct from a real 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    buffer_ms: i64,
    /// Queue fill against its cap, per mille: video, audio.
    queue: [i64; 2],
    kbps: i64,
}

const NO_CELL: Cell = Cell { buffer_ms: -1, queue: [-1, -1], kbps: -1 };

/// The old fork's sweep, minus Plex's quality-budget lane. **Cells are fixed x-coordinates**: a new
/// sample overwrites the cell under the cursor and nothing shifts left, so two phone photographs
/// of the panel compare cell for cell, and the cursor says which side is new. A new session (its
/// `load_at`) or the panel switching on clears it.
#[derive(Default)]
struct Sweep {
    cells: Vec<Cell>,
    /// The next cell to overwrite — the cursor's x.
    head: usize,
    session: u32,
}

impl Sweep {
    fn record(&mut self, d: &Diag, prev: Prev, now: u32) {
        if self.cells.len() != HIST_N || self.session != d.load_at {
            *self = Sweep { cells: vec![NO_CELL; HIST_N], head: 0, session: d.load_at };
        }
        let (cv, ca) = player::aq_caps();
        let fill = |b: i64, cap: i64| if cap > 0 { (b.max(0) * 1000 / cap).min(1000) } else { -1 };
        self.cells[self.head] = Cell {
            buffer_ms: d.playable_buffer_ms.map_or(-1, |ms| ms.max(0)),
            queue: [fill(d.aq_video, cv), fill(d.aq_audio, ca)],
            kbps: now_kbps(d, prev, now).unwrap_or(-1),
        };
        self.head = (self.head + 1) % HIST_N;
    }

    fn latest(&self) -> Cell {
        self.cells.get((self.head + HIST_N - 1) % HIST_N).copied().unwrap_or(NO_CELL)
    }

    fn observed(&self) -> impl Iterator<Item = Cell> + '_ {
        self.cells.iter().copied().filter(|c| *c != NO_CELL)
    }

    /// The mean over the window's observed cells. Bursty segmented transfers spend real cells at
    /// zero and those belong in it; a percentile would claim link capacity nobody measured.
    fn mean_kbps(&self) -> Option<i64> {
        let (sum, n) = self.observed().filter(|c| c.kbps >= 0).fold((0i64, 0i64), |(s, n), c| (s.saturating_add(c.kbps), n + 1));
        (n > 0).then(|| sum / n)
    }

    /// The four lanes' values, held with the sample: buffer, video queue, audio queue, network.
    fn values(&self, d: &Diag) -> [String; 4] {
        let latest = self.latest();
        let pct = |pm: i64| if pm < 0 { "—".to_string() } else { msg::core_percent(&sn_platform::i18n::current().number(pm / 10)) };
        let now = if latest.kbps < 0 { None } else { Some(latest.kbps) };
        [
            buffer_text(d.playable_buffer_ms).0,
            pct(latest.queue[0]),
            pct(latest.queue[1]),
            msg::app_stats_chart_mean(&show_kbps(self.mean_kbps()), &show_kbps(now)),
        ]
    }
}

/// The buffer lane's scale: a FIXED 20 s so two photographs compare by height, stretched in 10 s
/// steps only when the window holds more.
fn buffer_scale_ms(sweep: &Sweep) -> i64 {
    const FLOOR_MS: i64 = 20_000;
    let peak = sweep.observed().map(|c| c.buffer_ms).max().unwrap_or(0);
    FLOOR_MS.max((peak + 9_999) / 10_000 * 10_000)
}

// ---- drawing ----------------------------------------------------------------------------------

const MARGIN_Y: f32 = 60.0;
const PAD: f32 = 24.0;
const COL_GAP: f32 = 32.0;
/// The minimum header: title, one identity line, one verdict line, the column headings.
const HEAD_H: f32 = 118.0;
const HEAD_LINE_H: f32 = 24.0;
const HEAD_IDENTITY_Y: f32 = 38.0;
const PANEL_W: f32 = 2.0 * FIELD_COL_W + COL_GAP + 2.0 * PAD;
/// The chart's lanes (buffer, video queue, audio queue, network) and their heights. The buffer
/// lane is the tallest: it is the one read at a glance.
const LANE_H: [f32; 4] = [56.0, 30.0, 30.0, 44.0];
/// Air between the columns and the chart, and between two lanes.
const CHART_GAP: f32 = 12.0;
const LANE_GAP: f32 = 6.0;
const CHART_H: f32 = CHART_GAP + LANE_H[0] + LANE_H[1] + LANE_H[2] + LANE_H[3] + 3.0 * LANE_GAP;
/// The lane values' column, right of the cells.
const CHART_VALUE_W: f32 = 420.0;

/// Identity and verdict stay complete: wrapped, never elided, at the sampling boundary.
fn wrap_header(head: &[String; 2], width: f32) -> [Vec<String>; 2] {
    [sn_ui::widgets::diagnostic_lines(&head[0], width, false), sn_ui::widgets::diagnostic_lines(&head[1], width, true)]
}

struct HeaderLayout {
    verdict_y: f32,
    sections_y: f32,
    fields_y: f32,
}

impl HeaderLayout {
    fn new(lines: &[Vec<String>; 2]) -> Self {
        let identity_h = lines[0].len().max(1) as f32 * HEAD_LINE_H;
        let verdict_h = lines[1].len().max(1) as f32 * HEAD_LINE_H;
        let verdict_y = HEAD_IDENTITY_Y + identity_h;
        HeaderLayout { verdict_y, sections_y: verdict_y + verdict_h + 4.0, fields_y: HEAD_H + identity_h + verdict_h - 2.0 * HEAD_LINE_H }
    }
}

fn text(p: Painter, s: &str, sz: i32, ink: [f32; 4], bold: bool, r: Rect) {
    if let Ok(cs) = CString::new(s) {
        let l = Label::new(cs.as_ptr(), sz, ink);
        if bold { l.bold() } else { l }.draw(p, r);
    }
}

impl Diagnostics {
    fn panel_width(&self) -> f32 {
        if self.idle {
            FIELD_COL_W + 2.0 * PAD
        } else {
            PANEL_W
        }
    }

    /// The panel's box, sized to its content and clear of the transport, on the app's side margin
    /// (it is the overlay that must sit inside the overscan frame: its output is a photograph).
    pub fn panel_rect(&self) -> Rect {
        let head = HeaderLayout::new(&self.head_lines).fields_y;
        let lines = if self.idle {
            FieldList::wrapped_line_count(&self.rows)
        } else {
            FieldList::wrapped_line_count(&self.columns[0]).max(LEFT_ROWS).max(FieldList::wrapped_line_count(&self.columns[1]).max(RIGHT_ROWS))
        };
        let chart = if self.idle { 0.0 } else { CHART_H };
        Rect::new(sn_ui::consts::MARGIN_X, MARGIN_Y, self.panel_width(), head + FieldList::height(lines) + chart + PAD)
    }

    /// Draw the held snapshot. The caller decides whether it is [`visible`].
    pub fn draw(&self) {
        if self.head[0].is_empty() {
            return; // not sampled yet
        }
        let p = Painter::root();
        let e = Env::inert();
        let frame = self.panel_rect();
        // its own opaque ground: the player's UI plane is cleared transparent over the video
        p.rect(frame, 24.0, theme::PANEL_TOP, theme::PANEL_BOT, 0.0);
        let layout = HeaderLayout::new(&self.head_lines);
        let inner = frame.x + PAD;
        let iw = frame.w - 2.0 * PAD;
        text(p, msg::app_stats_title(), theme::size::BODY, theme::TEXT_PRIMARY, true, Rect::new(inner, frame.y + 10.0, iw, 30.0));
        let verdict_ink = if self.head[1].starts_with(msg::app_stats_error()) { theme::DANGER } else { theme::TEXT_PRIMARY };
        for (lines, y, ink, bold) in
            [(&self.head_lines[0], HEAD_IDENTITY_Y, theme::TEXT_TERTIARY, false), (&self.head_lines[1], layout.verdict_y, verdict_ink, true)]
        {
            for (i, line) in lines.iter().enumerate() {
                text(p, line, theme::size::DIAGNOSTIC, ink, bold, Rect::new(inner, frame.y + y + i as f32 * HEAD_LINE_H, iw, HEAD_LINE_H));
            }
        }
        let top = frame.y + layout.fields_y;
        let h = frame.h - layout.fields_y - PAD - if self.idle { 0.0 } else { CHART_H };
        let heading = |s: &str, x: f32| text(p, s, theme::size::DIAGNOSTIC, theme::TEXT_SECONDARY, true, Rect::new(x, frame.y + layout.sections_y, FIELD_COL_W, 24.0));
        if self.idle {
            heading(msg::app_stats_section_device(), inner);
            FieldList::new(&self.rows, Rect::new(inner, top, FIELD_COL_W, h)).draw(&e, p);
            return;
        }
        let right_x = inner + FIELD_COL_W + COL_GAP;
        heading(msg::app_stats_section_stream(), inner);
        heading(msg::app_stats_section_delivery(), right_x);
        FieldList::new(&self.columns[0], Rect::new(inner, top, FIELD_COL_W, h)).draw(&e, p);
        FieldList::new(&self.columns[1], Rect::new(right_x, top, FIELD_COL_W, h)).draw(&e, p);
        self.draw_chart(p, Rect::new(inner, top + h + CHART_GAP, iw, CHART_H - CHART_GAP));
    }

    /// The sweep: per lane a right-aligned key in the fields' gutter, one bar per cell, the held
    /// value on the right; one cursor across all lanes. Drawn from the held [`Sweep`], so it moves
    /// at the sample rate and asks for no frame of its own.
    fn draw_chart(&self, p: Painter, r: Rect) {
        let sw = &self.sweep;
        if sw.cells.len() != HIST_N {
            return;
        }
        let bx = r.x + sn_ui::widgets::FIELD_KEY_W + theme::space::SM;
        let value_x = r.x + r.w - CHART_VALUE_W;
        let bars_w = (value_x - theme::space::SM - bx).max(1.0);
        let bw = bars_w / HIST_N as f32;
        let bar = |x: f32, base: f32, h: f32, ink: [f32; 4]| p.rect(Rect::new(x, base - h, (bw - 1.0).max(1.0), h), 0.0, ink, ink, 0.0);
        let buffer_scale = buffer_scale_ms(sw) as f32;
        let net_peak = sw.observed().map(|c| c.kbps).max().unwrap_or(0).max(1) as f32;
        let mean = sw.mean_kbps();
        let keys = [msg::app_stats_f_buffer(), msg::app_stats_chart_queue_v(), msg::app_stats_chart_queue_a(), msg::app_stats_f_throughput()];
        let low_now = (0..BUFFER_LOW_MS).contains(&sw.latest().buffer_ms);
        let mut y = r.y;
        for (lane, lh) in LANE_H.into_iter().enumerate() {
            let base = y + lh;
            let mid = Rect::new(r.x, y + (lh - FIELD_ROW_H) / 2.0, sn_ui::widgets::FIELD_KEY_W, FIELD_ROW_H);
            if let Ok(cs) = CString::new(keys[lane]) {
                Label::new(cs.as_ptr(), theme::size::DIAGNOSTIC, theme::TEXT_TERTIARY).h(HAlign::Right).draw(p, mid);
            }
            let (ink, bold) = match lane {
                0 if low_now => (theme::DANGER, true),
                _ => (theme::TEXT_PRIMARY, false),
            };
            text(p, &self.chart_vals[lane], theme::size::DIAGNOSTIC, ink, bold, Rect::new(value_x, mid.y, CHART_VALUE_W, FIELD_ROW_H));
            if lane == 0 {
                // the low-water band: a bar that dips into it is a stall on its way
                let band = (BUFFER_LOW_MS as f32 / buffer_scale * lh).max(3.0);
                p.rect(Rect::new(bx, base - band, bars_w, band), 0.0, theme::DIAG_BUFFER_LOW_BAND, theme::DIAG_BUFFER_LOW_BAND, 0.0);
            }
            // the ground line: a run of zeros reads as data, not as blank card
            p.rect(Rect::new(bx, base - 1.0, bars_w, 1.0), 0.0, theme::HAIRLINE, theme::HAIRLINE, 0.0);
            for (k, c) in sw.cells.iter().enumerate() {
                // `k`, not `(head + k) % N`: cells are x-coordinates and never rotate
                let x = bx + k as f32 * bw;
                match lane {
                    0 if c.buffer_ms == 0 => bar(x, base, 3.0, theme::DIAG_BUFFER_LOW), // EMPTY is a mark, not a gap
                    0 if c.buffer_ms > 0 => {
                        let ink = if c.buffer_ms < BUFFER_LOW_MS { theme::DIAG_BUFFER_LOW } else { theme::DIAG_BUFFER_OK };
                        bar(x, base, (c.buffer_ms as f32 / buffer_scale).min(1.0) * lh, ink);
                    }
                    1 | 2 if c.queue[lane - 1] > 0 => bar(x, base, c.queue[lane - 1] as f32 / 1000.0 * lh, theme::DIAG_QUEUE_FILL),
                    3 if c.kbps > 0 => bar(x, base, c.kbps as f32 / net_peak * lh, theme::DIAG_NETWORK_ACTIVITY),
                    _ => {}
                }
            }
            if let (3, Some(m)) = (lane, mean) {
                let my = base - (m as f32 / net_peak).min(1.0) * lh;
                p.rect(Rect::new(bx, my - 1.0, bars_w, 2.0), 0.0, theme::DIAG_NETWORK_MEAN, theme::DIAG_NETWORK_MEAN, 0.0);
            }
            y = base + LANE_GAP;
        }
        // one cursor across every lane: the next cell to be overwritten
        let cx = bx + sw.head as f32 * bw;
        p.rect(Rect::new(cx, r.y, 2.0, y - LANE_GAP - r.y), 0.0, theme::DIAG_SWEEP_CURSOR, theme::DIAG_SWEEP_CURSOR, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_media::stream::{Dovi, ProbedAudio, ProbedVideo};
    use sn_platform::i18n::{language_on_this_thread_for_test, Preference, SHIPPED};

    fn probe() -> ProbedStreams {
        ProbedStreams {
            video: Some(ProbedVideo {
                codec: "hevc".into(),
                width: 3840,
                height: 2160,
                fps: 23.976,
                profile: -99,
                dovi: Dovi { present: true, profile: 8, bl_compat: 1, el_present: false },
            }),
            audio: vec![ProbedAudio { codec: "eac3".into(), channels: 6, ..ProbedAudio::default() }],
            ..ProbedStreams::default()
        }
    }

    /// A healthy playback a few seconds in.
    fn playing() -> Diag {
        Diag {
            vp_mode: player::VP_EXPORTED,
            window_id: "_Window_Id_1".into(),
            place_rv: 1,
            placed_w: 3840,
            placed_h: 2160,
            stage: 3,
            load_completed: true,
            cb_count: 42,
            pushed_any: true,
            fed_v: 480,
            fed_a: 620,
            frames: 400,
            seen_frame: true,
            aq_video: 12 << 20,
            aq_audio: 1 << 19,
            fed_v_pts: 21_000_000_000,
            fed_a_pts: 20_600_000_000,
            load_v: 2,
            load_a: 2,
            feed_state: 4,
            http_status: 206,
            net_rx: 48 << 20,
            playable_buffer_ms: Some(12_400),
            load_at: 1_000,
            frame_at: 19_900,
            video_w: 3840,
            video_h: 2160,
            video_fps_milli: 23_976,
            pos_ns: 20_000_000_000,
            dur_ns: 5_400_000_000_000,
            ..Diag::default()
        }
    }

    const URL: &str = "https://user:pw@abc123.download.real-debrid.com:8443/d/SECRETKEY/Film.2160p.mkv?token=hunter2";

    fn facts<'a>(d: &'a Diag, probe: Option<&'a ProbedStreams>) -> Facts<'a> {
        Facts {
            d,
            state: PlaybackState::Playing,
            paused: false,
            url: URL,
            addon: "Torrentio",
            probe,
            audio_track: 0,
            error: "",
            hls_error: None,
            aspect: AspectMode::Auto,
            on_since: 0,
        }
    }

    fn value<'a>(rows: &'a [Field], key: &str) -> &'a Field {
        rows.iter().find(|f| f.key == key).unwrap_or_else(|| panic!("no {key} row"))
    }

    #[test]
    fn the_source_row_is_the_addon_and_the_server_kind_never_the_url() {
        let _g = language_on_this_thread_for_test(Preference::En);
        assert_eq!(source_host(URL).as_deref(), Some("real-debrid.com"), "the registrable domain only");
        assert_eq!(source_host("http://127.0.0.1:11470/abc/0").as_deref(), Some("this TV"));
        assert_eq!(source_host("http://192.168.1.20:11470/x").as_deref(), Some("local network"));
        assert_eq!(source_host("http://nas.local/film.mkv").as_deref(), Some("local network"));
        assert_eq!(source_host("http://nas/film.mkv").as_deref(), Some("local network"), "a bare name is the LAN's");
        assert_eq!(source_host("https://203.0.113.9/x.mkv").as_deref(), Some("public address"), "an IP is never shown");
        assert_eq!(source_host("http://[fd00::5]:8080/x").as_deref(), Some("local network"));
        assert_eq!(source_host("https://cdn.example.co.uk/x").as_deref(), Some("example.co.uk"));
        assert_eq!(source_host(""), None);
        let line = source_line("Torrentio", URL);
        assert_eq!(line, "Torrentio · real-debrid.com");
        let rows = stream_rows(&facts(&playing(), None));
        let all: String = rows.iter().filter_map(|f| f.val.clone()).collect::<Vec<_>>().join("\n");
        for secret in ["SECRETKEY", "hunter2", "user", "pw@", "abc123", "8443", "://", "Film"] {
            assert!(!all.contains(secret), "{secret:?} reached the panel:\n{all}");
        }
    }

    #[test]
    fn the_transport_and_container_come_from_the_scheme_and_the_extension() {
        let _g = language_on_this_thread_for_test(Preference::En);
        assert_eq!(transport_line(URL, 1), "Progressive · HTTPS");
        assert_eq!(transport_line("https://tv.example/live/index.m3u8?k=1", 0), "HLS · live · HTTPS");
        assert_eq!(transport_line("https://cdn.example/vod/master.m3u8", 9), "HLS · HTTPS");
        let hash = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(transport_line(&format!("http://127.0.0.1:11470/{hash}/0"), 9), "Torrent via streaming server · HTTP");
        assert_eq!(container_line(URL), "MKV");
        assert_eq!(container_line("https://h.example/play/42"), "not in the URL");
        assert_eq!(container_line("https://h.example.com"), "not in the URL", "a host's dots are no extension");
    }

    #[test]
    fn a_healthy_playback_reads_without_a_fault() {
        let _g = language_on_this_thread_for_test(Preference::En);
        let d = playing();
        let p = probe();
        let f = facts(&d, Some(&p));
        let left = stream_rows(&f);
        let right = delivery_rows(&f, Prev { fed_v: 468, fed_a: 604, net_rx: 47 << 20, at: 19_500 }, Stalls::default(), 20_000);
        assert_eq!((left.len(), right.len()), (LEFT_ROWS, RIGHT_ROWS), "the schema is fixed");
        assert_eq!(value(&left, "Video").val.as_deref(), Some("hevc → H265 · Dolby Vision P8.1"));
        assert_eq!(value(&left, "Audio").val.as_deref(), Some("eac3 6ch → AC3 PLUS"));
        let mut joc = probe();
        joc.audio[0].atmos = true;
        assert_eq!(value(&stream_rows(&facts(&d, Some(&joc))), "Audio").val.as_deref(), Some("eac3 6ch → AC3 PLUS · Dolby Atmos"));
        assert_eq!(value(&left, "Picture").val.as_deref(), Some("3840×2160 · 23.976 fps (pipeline) · Auto"));
        assert_eq!(value(&left, "Position").val.as_deref(), Some("0:20 / 1:30:00"));
        assert_eq!(value(&left, "Feed V−A").val.as_deref(), Some("+0.4 s"));
        assert_eq!(value(&left, "Video plane").val.as_deref(), Some("exported window · window ready · placed 3840×2160 rv=1"));
        assert_eq!(value(&right, "Transfer").val.as_deref(), Some("48.0 MB · HTTP 206"));
        assert_eq!(value(&right, "Throughput").val.as_deref(), Some("16.8 Mbps now · 21.2 Mbps average"));
        assert_eq!(value(&right, "Feed").val.as_deref(), Some("24 fps · audio 32/s · holding ~1.6 s ahead"));
        assert_eq!(value(&right, "Buffer").val.as_deref(), Some("12.4 s ahead"));
        assert_eq!(value(&right, "Frames").val.as_deref(), Some("400"));
        assert_eq!(value(&right, "Stalls").val.as_deref(), Some("0 seen"));
        assert_eq!(value(&right, "Error").val.as_deref(), Some("none"));
        for row in left.iter().chain(&right) {
            assert_eq!(row.tone, sn_ui::widgets::Tone::Normal, "{}: {:?}", row.key, row.val);
        }
        assert_eq!(verdict_line(&f, 20_000), "Playing");
    }

    #[test]
    fn a_dead_playback_names_its_faults_in_words() {
        let _g = language_on_this_thread_for_test(Preference::En);
        let d = Diag {
            vp_mode: player::VP_EXPORTED,
            place_rv: i32::MIN,
            load_completed: true,
            load_failed: true,
            cb_err: 7,
            cb_err_at: 3,
            cb_count: 3,
            http_status: 403,
            load_v: 2,
            load_a: 0,
            load_at: 1_000,
            feed_state: 3,
            pushed_any: true,
            fed_v_pts: 9_000_000_000,
            fed_a_pts: 1_000_000_000,
            ..Diag::default()
        };
        let f = Facts { state: PlaybackState::Error, error: "The server refused the stream", ..facts(&d, None) };
        let left = stream_rows(&f);
        let right = delivery_rows(&f, Prev::default(), Stalls::default(), 20_000);
        let fault = |rows: &[Field], key: &str| {
            let r = value(rows, key);
            assert_eq!(r.tone, sn_ui::widgets::Tone::Fault, "{key} is a fault: {:?}", r.val);
            r.val.clone().unwrap()
        };
        assert!(fault(&left, "Video plane").contains("NO WINDOW · NOT PLACED"));
        assert!(fault(&left, "Picture").contains("NOT OPENED"));
        assert!(fault(&left, "Audio").contains("NONE"), "a video Load that sends no audio");
        assert_eq!(fault(&left, "Feed V−A"), "+8.0 s");
        assert_eq!(fault(&right, "Transfer"), "0 B · HTTP 403");
        assert_eq!(fault(&right, "Load"), "REFUSED · 3 events · ERROR 7 at #3");
        assert!(fault(&right, "Feed").ends_with("REFUSED"));
        assert!(fault(&right, "Frames").starts_with("NONE"));
        assert_eq!(fault(&right, "Error"), "The server refused the stream");
        assert_eq!(verdict_line(&f, 20_000), "Playback error — The server refused the stream");
    }

    #[test]
    fn a_frozen_picture_is_counted_once_and_a_pause_is_never_a_stall() {
        let _g = language_on_this_thread_for_test(Preference::En);
        let d = Diag { frame_at: 10_000, ..playing() };
        let f = facts(&d, None);
        let mut s = Stalls::default();
        for now in [10_500, 11_000, 12_500, 13_000, 19_000] {
            s = count_stall(s, &f, now);
        }
        assert_eq!(s.count, 1, "one freeze, however many samples see it");
        let right = delivery_rows(&f, Prev::default(), s, 19_000);
        assert_eq!(value(&right, "Stalls").val.as_deref(), Some("1 seen · STALLED NOW 9 s"));
        assert_eq!(value(&right, "Frames").val.as_deref(), Some("400 — FROZEN 9 s"));
        assert_eq!(verdict_line(&f, 19_000), "STALLED 9 s — Playing");
        let paused = Facts { paused: true, ..facts(&d, None) };
        assert_eq!(count_stall(Stalls::default(), &paused, 19_000).count, 0);
        assert_eq!(verdict_line(&paused, 19_000), "Playing (paused)");
        assert_eq!(frames_str(&paused, 19_000), "400");
        // a frame clock older than the switch-on was not observed: no freeze is inferred from it
        let fresh = Facts { on_since: 18_500, ..facts(&d, None) };
        assert!(frozen_ms(&fresh, 19_000).is_none());
        // a new session restarts the count
        let next = Diag { load_at: 30_000, frame_at: 30_050, ..playing() };
        assert_eq!(count_stall(s, &facts(&next, None), 30_100).count, 0);
    }

    #[test]
    fn the_panel_samples_at_two_hertz_and_holds_between() {
        let mut p = Diagnostics::default();
        assert!(!p.due(false, 0), "off: never");
        assert!(p.due(true, 100), "switched on: at once");
        assert_eq!(p.on_since, 100);
        assert!(!p.due(true, 116));
        assert!(!p.due(true, 599), "held for the whole period");
        assert!(p.due(true, 600));
        assert!(!p.due(true, 1_099));
        assert!(p.due(true, 1_100));
        assert!(!p.due(false, 1_700), "off again: nothing, and no repaint");
        assert!(p.due(true, 1_716), "on again: at once, whatever the old deadline");
        // the loop clock wraps without a stall
        let mut q = Diagnostics::default();
        assert!(q.due(true, u32::MAX - 100));
        assert!(!q.due(true, u32::MAX));
        assert!(q.due(true, 400));
    }

    #[test]
    fn nothing_played_shows_the_device_card() {
        assert!(never_played(&Diag::default(), PlaybackState::Idle));
        assert!(!never_played(&Diag { load_at: 5, ..Diag::default() }, PlaybackState::Idle), "an ended playback keeps its post-mortem");
        assert!(!never_played(&Diag::default(), PlaybackState::Connecting), "a start that has not loaded says where it waits");
        let _g = language_on_this_thread_for_test(Preference::En);
        let mut p = Diagnostics::default();
        let d = Diag::default();
        p.sample(&Facts { state: PlaybackState::Idle, url: "", addon: "", ..facts(&d, None) }, 0);
        assert!(p.idle);
        assert_eq!(p.rows.len(), 5);
        assert!(p.columns.iter().all(Vec::is_empty));
        assert!(p.panel_rect().w < PANEL_W, "the device card is one column");
    }

    /// Every row key fits its gutter, the headings and every composed fixture value fit their
    /// column on one line, in every shipped language; and the panel stays above the transport.
    #[test]
    fn every_string_fits_in_every_shipped_language() {
        use sn_machine::machine::Measure as _;
        use sn_ui::widgets::{FIELD_KEY_W, FIELD_VAL_SZ, FIELD_VAL_W};
        let m = sn_base::fontcov::advances::ShippedMeasure;
        let mut out = Vec::new();
        let d = playing();
        let p = probe();
        let dead = Diag { load_failed: true, cb_err: 1234, cb_err_at: 99, cb_count: 999, http_status: 503, place_rv: i32::MIN, ..playing() };
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let tag = language.tag();
            let f = facts(&d, Some(&p));
            let frozen = Diag { frame_at: 1_000, ..playing() };
            let mut rows = stream_rows(&f);
            rows.extend(delivery_rows(&f, Prev { fed_v: 1, fed_a: 1, net_rx: 1, at: 19_500 }, Stalls { count: 12, ..Stalls::default() }, 20_000));
            rows.extend(delivery_rows(&facts(&frozen, None), Prev::default(), Stalls { count: 12, ..Stalls::default() }, 99_000));
            rows.extend(stream_rows(&facts(&dead, None)));
            for aspect in AspectMode::ALL {
                rows.extend(stream_rows(&Facts { aspect, ..facts(&d, Some(&p)) }));
            }
            rows.extend(delivery_rows(&facts(&dead, None), Prev::default(), Stalls::default(), 99_000));
            rows.extend(device_rows());
            for r in &rows {
                if m.width_str(r.key, FIELD_VAL_SZ, false) > FIELD_KEY_W {
                    out.push(format!("{tag}: key {:?} overflows its gutter", r.key));
                }
                if let Some(v) = &r.val {
                    if m.width_str(v, FIELD_VAL_SZ, r.tone == sn_ui::widgets::Tone::Fault) > FIELD_VAL_W {
                        out.push(format!("{tag}: {} = {v:?} wraps", r.key));
                    }
                }
            }
            for h in [msg::app_stats_section_stream(), msg::app_stats_section_delivery(), msg::app_stats_section_device()] {
                if m.width_str(h, theme::size::DIAGNOSTIC, true) > FIELD_COL_W {
                    out.push(format!("{tag}: heading {h:?}"));
                }
            }
            if m.width_str(msg::app_stats_title(), theme::size::BODY, true) > PANEL_W - 2.0 * PAD {
                out.push(format!("{tag}: the title"));
            }
            for k in [msg::app_stats_f_buffer(), msg::app_stats_chart_queue_v(), msg::app_stats_chart_queue_a(), msg::app_stats_f_throughput()] {
                if m.width_str(k, FIELD_VAL_SZ, false) > FIELD_KEY_W {
                    out.push(format!("{tag}: chart key {k:?} overflows its gutter"));
                }
            }
            let mut sw = Sweep::default();
            sw.record(&d, Prev { net_rx: 1, at: 19_500, ..Prev::default() }, 20_000);
            let worst = [Some(123_400), Some(1_900), Some(0)].map(buffer_text).map(|b| b.0);
            for v in sw.values(&d).iter().chain(&worst) {
                if m.width_str(v, FIELD_VAL_SZ, true) > CHART_VALUE_W {
                    out.push(format!("{tag}: chart value {v:?} overflows"));
                }
            }
            let mut panel = Diagnostics::default();
            panel.sample(&f, 20_000);
            let r = panel.panel_rect();
            if r.y + r.h > sn_screens::player::CTRL_Y {
                out.push(format!("{tag}: the panel reaches the transport ({} > {})", r.y + r.h, sn_screens::player::CTRL_Y));
            }
            if r.x + r.w > sn_ui::consts::SCR_W - sn_ui::consts::MARGIN_X {
                out.push(format!("{tag}: the panel leaves the safe area"));
            }
        }
        assert!(out.is_empty(), "{}", out.join("\n"));
    }

    /// The chart: fixed x-cells under a moving cursor, a new session starting it again, the
    /// interval's rate from the byte counter, and LOW / EMPTY in words as well as colour.
    #[test]
    fn the_sweep_holds_a_minute_in_fixed_cells() {
        let _g = language_on_this_thread_for_test(Preference::En);
        let mut sw = Sweep::default();
        let d = playing();
        sw.record(&d, Prev::default(), 20_000);
        assert_eq!((sw.cells.len(), sw.head), (HIST_N, 1));
        assert_eq!(sw.cells[0].kbps, -1, "no previous sample: not observed, not zero");
        assert_eq!(sw.cells[0].buffer_ms, 12_400);
        let next = Diag { net_rx: d.net_rx + 1_000_000, ..playing() };
        sw.record(&next, Prev { net_rx: d.net_rx, at: 20_000, ..Prev::default() }, 20_500);
        assert_eq!(sw.cells[1].kbps, 16_000, "1 MB in half a second");
        assert_eq!(sw.cells[0].buffer_ms, 12_400, "an older cell does not move");
        for i in 0..HIST_N - 1 {
            sw.record(&d, Prev::default(), 21_000 + i as u32);
        }
        assert_eq!(sw.head, 1, "the cursor wraps and overwrites in place");
        assert_eq!(sw.mean_kbps(), Some(16_000), "unobserved cells stay out of the mean");
        sw.record(&Diag { load_at: 9_999, ..playing() }, Prev::default(), 90_000);
        assert_eq!((sw.head, sw.observed().count()), (1, 1), "a new session starts the sweep again");

        let low = Diag { playable_buffer_ms: Some(900), aq_video: 24 << 20, ..playing() };
        assert_eq!(buffer_text(low.playable_buffer_ms), ("0.9 s ahead · LOW".to_string(), true));
        assert_eq!(buffer_text(Some(0)), ("EMPTY — nothing ahead".to_string(), true));
        assert_eq!(buffer_text(None).1, false);
        sw.record(&low, Prev::default(), 90_500);
        assert_eq!(sw.values(&low)[0], "0.9 s ahead · LOW");
        assert_eq!(sw.values(&low)[1], "50%", "24 MB of the video queue's 48 MB cap");
        let f = facts(&low, None);
        assert_eq!(value(&delivery_rows(&f, Prev::default(), Stalls::default(), 20_000), "Buffer").tone, sn_ui::widgets::Tone::Fault);
    }

    #[test]
    fn the_aspect_names_match_the_media_modes() {
        assert_eq!(sn_screens::player::tracks::aspect_labels().len(), AspectMode::ALL.len());
    }
}
