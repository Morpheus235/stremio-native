//! **The artwork source** — `sn_ui::tex::Source` for the addons' posters, stills and backdrops,
//! the render cache's other half (`sn_ui::tex`'s module doc).
//!
//! A tile asks for `(url, w, h)` while it draws. This interns the request into a [`PosterKey`]
//! and, on a worker, produces the pixels that COVER the requested box, the cheapest way it can:
//!
//! 1. **the disk tier** (`sn_platform::imgcache`, [`ImgCacheDisk`]): the already SCALED pixels of
//!    an earlier visit, so a revisit or a reboot shows a poster with no fetch and no decode;
//! 2. else **the network**, for the smallest variant the host offers that still covers the box
//!    ([`sized_url`]: metahub, TMDB and Amazon name the size in the URL), asking for WebP first
//!    ([`crate::fetch::ART_ACCEPT`]) and following redirects; then the decode (JPEG, PNG, WebP)
//!    and the cover-scale, both on the worker, never on the frame; the scaled pixels go to disk.
//!
//! The frame thread hands the pixels to the cache ([`Art::drain`] → `tex::accept`); the upload is
//! the cache's, in the dispatcher's prepare pass.
//!
//! **Viewport first, as PlxNative's poster store does it.** A draw stamps its key with the frame;
//! the keys drawn most recently start first, at most [`VISIBLE_INFLIGHT_MAX`] at once, and a
//! prefetch (`warm`) may hold one more worker ([`PREFETCH_INFLIGHT_MAX`]), never a visible one's.
//! A key that has not been drawn for [`CANCEL_FRAMES`] (its row scrolled away) is cancelled: a
//! queued one is forgotten before it costs a request, one already out has its [`Cancel`] set so the
//! worker skips the decode. A prefetch nobody drew in [`WARM_KEEP_FRAMES`] is forgotten too.
//!
//! **Failures are told apart.** A transport failure, a timeout or a 5xx/429/408 is TRANSIENT: the
//! key parks for a bounded backoff (250 ms doubling to 8 s, [`RETRY_MIN_MS`]..[`RETRY_MAX_MS`]) and
//! the next DRAW after it is due asks again. Anything else (a 404, bytes that do not decode) has
//! SETTLED: [`Source::failed`](tex::Source::failed) says so, and the tile draws its fallback (the
//! title on a tinted card) instead of a skeleton. A first settled miss is asked once more after
//! [`SETTLED_RETRY_MS`] by a draw (the card back in view), a second settles for [`FAILED_MS`]; a key
//! that has failed transiently [`FALLBACK_AFTER_ATTEMPTS`] times in a row also reads as failed while
//! its backoff keeps asking. A key that has missed keeps reading failed through its retry, so the
//! fallback does not flicker back to a skeleton; the pixels replace it if they ever arrive. A key the render cache refused to keep resident cools for
//! [`REFUSED_MS`]; one it lost under pressure is asked again by the next draw that wants it
//! (cheaply, from the disk tier).
//!
//! **Bounded**: at most [`SLOTS_MAX`] keys are remembered; the least recently drawn is forgotten
//! (its texture freed on the next prepare), never one drawn in this frame or the last.
//!
//! Addons have no clearLogo, so `logo` answers nothing and `logo_failed` says settled.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use sn_base::task::OffFrame;
use sn_machine::machine::PosterKey;
use sn_stremio::sched::Cancel;
use sn_stremio::{Fetch, FetchError};
use sn_ui::tex::{self, Decoded, PosterError, PosterReady, Warm};

use crate::fetch::{NetFetch, Profile, ART_ACCEPT};

/// Fetches out at once for keys a draw asked for.
pub const VISIBLE_INFLIGHT_MAX: usize = 4;
/// Fetches out at once for prefetches: speculation never takes a visible key's worker.
pub const PREFETCH_INFLIGHT_MAX: usize = 1;
/// Keys remembered (each a poster's texture at most, ~375 KB at 250x375).
pub const SLOTS_MAX: usize = 160;
/// Frames without a draw after which a wanted key is cancelled (its row left the screen).
pub const CANCEL_FRAMES: u64 = 30;
/// Frames a prefetched key may wait for its first draw before it is forgotten.
pub const WARM_KEEP_FRAMES: u64 = 600;
/// The first retry of a transient failure; each further one doubles, up to [`RETRY_MAX_MS`].
pub const RETRY_MIN_MS: u64 = 250;
pub const RETRY_MAX_MS: u64 = 8_000;
/// How long a settled miss (a 404, an undecodable body) stays settled.
pub const FAILED_MS: u64 = 300_000;
/// A FIRST settled miss is asked once more this much later, by the next draw that wants it (the
/// card scrolled back into view): a CDN's passing 404 or a truncated body does not cost the
/// poster for [`FAILED_MS`]. A second miss settles for the full [`FAILED_MS`].
pub const SETTLED_RETRY_MS: u64 = 30_000;
/// Transient failures in a row after which [`Source::failed`](tex::Source::failed) answers
/// `true`: the tile shows its designed fallback while the backoff keeps asking, instead of a
/// skeleton for as long as the host keeps failing.
pub const FALLBACK_AFTER_ATTEMPTS: u32 = 3;
/// How long a key the render cache refused to keep resident cools down.
pub const REFUSED_MS: u64 = 15_000;

/// Which size of a host's variants a request box needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// A poster card (250x375 at 1080p).
    Card,
    /// A landscape still (420x236).
    Wide,
    /// A hero or a backdrop.
    Hero,
}

impl Variant {
    pub fn of(w: i32, h: i32) -> Variant {
        if w > 600 || h > 900 {
            Variant::Hero
        } else if w > 342 {
            Variant::Wide
        } else {
            Variant::Card
        }
    }
}

/// `url` asking its host for the smallest variant that still covers a `w`x`h` box, for the hosts
/// whose URLs name a size. Every other URL is returned untouched.
///
/// - `images.metahub.space/poster/{small|medium|large}/…`: cards and stills take `medium`, a hero
///   `large`;
/// - `image.tmdb.org/t/p/{w92|…|w1280|original}/…`: cards `w342`, stills `w500`, a hero `w1280`;
/// - Amazon's `…._V1_<ops>.jpg` (`m.media-amazon.com`, `ia.media-imdb.com`): the operations are
///   replaced by a width (`_V1_UX300_` for a card, `UX500` a still, `UX1280` a hero).
pub fn sized_url(url: &str, w: i32, h: i32) -> Cow<'_, str> {
    let variant = Variant::of(w, h);
    let Some((scheme, rest)) = url.split_once("://") else { return Cow::Borrowed(url) };
    if !(scheme == "https" || scheme == "http") || rest.contains('?') || rest.contains('#') {
        return Cow::Borrowed(url);
    }
    let Some((host, path)) = rest.split_once('/') else { return Cow::Borrowed(url) };
    let host = host.to_ascii_lowercase();
    let replaced = match host.as_str() {
        "images.metahub.space" => {
            let size = if variant == Variant::Hero { "large" } else { "medium" };
            replace_segment(path, "poster/", &["small", "medium", "large"], size)
        }
        "image.tmdb.org" => {
            let size = match variant {
                Variant::Card => "w342",
                Variant::Wide => "w500",
                Variant::Hero => "w1280",
            };
            replace_segment(path, "t/p/", &["w92", "w154", "w185", "w300", "w342", "w500", "w780", "w1280", "original"], size)
        }
        "m.media-amazon.com" | "ia.media-imdb.com" => amazon_width(
            path,
            match variant {
                Variant::Card => 300,
                Variant::Wide => 500,
                Variant::Hero => 1280,
            },
        ),
        _ => None,
    };
    match replaced {
        Some(path) => Cow::Owned(format!("{scheme}://{}/{path}", rest.split_once('/').map_or("", |(h, _)| h))),
        None => Cow::Borrowed(url),
    }
}

/// `path` with the segment after `prefix` (which must start the path) replaced by `size`, when that
/// segment is one of `known`.
fn replace_segment(path: &str, prefix: &str, known: &[&str], size: &str) -> Option<String> {
    let tail = path.strip_prefix(prefix)?;
    let (segment, rest) = tail.split_once('/')?;
    if !known.contains(&segment) || rest.is_empty() {
        return None;
    }
    Some(format!("{prefix}{size}/{rest}"))
}

/// An Amazon image path with its `._V1_…` operations replaced by `UX{width}`.
fn amazon_width(path: &str, width: u32) -> Option<String> {
    let at = path.rfind("._V1_")?;
    let after = &path[at + "._V1_".len()..];
    let dot = after.rfind('.')?;
    let (ops, ext) = (&after[..dot], &after[dot + 1..]);
    if ops.contains('/') || ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(format!("{}._V1_UX{width}_.{ext}", &path[..at]))
}

// ---------------------------------------------------------------------------------------------
// The worker's half: disk, fetch, decode, scale.

/// Where scaled pixels persist between visits and boots.
pub trait Disk: Send + Sync {
    fn read(&self, url: &str, w: u32, h: u32) -> Option<Vec<u8>>;
    fn write(&self, url: &str, w: u32, h: u32, bytes: &[u8]);
    fn remove(&self, url: &str, w: u32, h: u32);
}

/// The disk tier over `sn_platform::imgcache` (`classify_pixels`).
pub struct ImgCacheDisk;

impl Disk for ImgCacheDisk {
    fn read(&self, url: &str, w: u32, h: u32) -> Option<Vec<u8>> {
        let key = sn_platform::imgcache::classify_pixels(url, w, h)?;
        sn_platform::imgcache::read_at(sn_platform::imgcache::generation(), &key).map(|c| c.bytes)
    }
    fn write(&self, url: &str, w: u32, h: u32, bytes: &[u8]) {
        if let Some(key) = sn_platform::imgcache::classify_pixels(url, w, h) {
            sn_platform::imgcache::write_at(sn_platform::imgcache::generation(), &key, bytes);
        }
    }
    fn remove(&self, url: &str, w: u32, h: u32) {
        if let Some(key) = sn_platform::imgcache::classify_pixels(url, w, h) {
            sn_platform::imgcache::remove_at(sn_platform::imgcache::generation(), &key);
        }
    }
}

/// The disk form of scaled pixels: magic, width, height (u16 LE), channels (3 or 4), the rows.
/// An opaque image (every poster) drops its alpha, a quarter of the bytes.
const PX_MAGIC: &[u8; 6] = b"SNPX1\0";
const PX_HEAD: usize = PX_MAGIC.len() + 5;

pub fn encode_pixels(d: &Decoded) -> Vec<u8> {
    let opaque = d.rgba.chunks_exact(4).all(|p| p[3] == 255);
    let channels: u8 = if opaque { 3 } else { 4 };
    let mut out = Vec::with_capacity(PX_HEAD + d.rgba.len());
    out.extend_from_slice(PX_MAGIC);
    out.extend_from_slice(&d.w.to_le_bytes());
    out.extend_from_slice(&d.h.to_le_bytes());
    out.push(channels);
    if opaque {
        for p in d.rgba.chunks_exact(4) {
            out.extend_from_slice(&p[..3]);
        }
    } else {
        out.extend_from_slice(&d.rgba);
    }
    out
}

pub fn decode_pixels(b: &[u8]) -> Option<Decoded> {
    let head = b.get(..PX_HEAD)?;
    if &head[..PX_MAGIC.len()] != PX_MAGIC {
        return None;
    }
    let w = u16::from_le_bytes([head[6], head[7]]);
    let h = u16::from_le_bytes([head[8], head[9]]);
    let channels = usize::from(head[10]);
    let n = usize::from(w) * usize::from(h);
    let body = &b[PX_HEAD..];
    if w == 0 || h == 0 || !(channels == 3 || channels == 4) || body.len() != n * channels {
        return None;
    }
    let rgba: Box<[u8]> = if channels == 4 {
        body.into()
    } else {
        let mut out = Vec::with_capacity(n * 4);
        for p in body.chunks_exact(3) {
            out.extend_from_slice(p);
            out.push(255);
        }
        out.into_boxed_slice()
    };
    Some(Decoded { w, h, rgba })
}

/// Where a load's pixels came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Disk,
    Network,
}

/// A worker's answer for one key.
pub enum Outcome {
    Ready(Decoded, Origin),
    /// `transient`: the same request may well succeed later (no answer, a 5xx, a 429).
    Failed { transient: bool, error: PosterError },
    /// The key was cancelled (scrolled away) before the work was done.
    Cancelled,
}

/// May a failed fetch be asked again soon?
pub fn transient(e: &FetchError) -> bool {
    match e {
        FetchError::Transport(_) | FetchError::Cancelled => true,
        FetchError::Status(s) => *s >= 500 || *s == 429 || *s == 408,
        FetchError::Decode(_) => false,
    }
}

/// One key's pixels: the disk tier first, else the network (then written to disk).
pub fn load(fetch: &dyn Fetch, disk: &dyn Disk, url: &str, w: i32, h: i32, cancel: &Cancel) -> Outcome {
    let (dw, dh) = (w.max(0) as u32, h.max(0) as u32);
    if let Some(bytes) = disk.read(url, dw, dh) {
        match decode_pixels(&bytes) {
            Some(d) => return Outcome::Ready(d, Origin::Disk),
            None => disk.remove(url, dw, dh),
        }
    }
    if cancel.is_cancelled() {
        return Outcome::Cancelled;
    }
    let body = match fetch.get(url, &[]) {
        Ok(b) if !b.is_empty() => b,
        Ok(_) => return Outcome::Failed { transient: true, error: PosterError::Fetch },
        Err(FetchError::Cancelled) => return Outcome::Cancelled,
        Err(e) => return Outcome::Failed { transient: transient(&e), error: PosterError::Fetch },
    };
    if cancel.is_cancelled() {
        return Outcome::Cancelled;
    }
    match decode_cover(&body, w, h) {
        Ok(d) => {
            disk.write(url, dw, dh, &encode_pixels(&d));
            Outcome::Ready(d, Origin::Network)
        }
        Err(error) => Outcome::Failed { transient: false, error },
    }
}

/// Decode `body` and scale it down (never up) so it just covers a `w`x`h` box.
pub fn decode_cover(body: &[u8], w: i32, h: i32) -> Result<Decoded, PosterError> {
    let (iw, ih, rgba) = sn_gfx::img::img_decode_owned(body).ok_or(PosterError::Decode)?;
    let (tw, th) = cover_size(iw, ih, w, h);
    let rgba = if (tw, th) == (iw, ih) {
        rgba
    } else {
        let img = image::RgbaImage::from_raw(iw, ih, rgba).ok_or(PosterError::Decode)?;
        image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle).into_raw()
    };
    let (Ok(w16), Ok(h16)) = (u16::try_from(tw), u16::try_from(th)) else {
        return Err(PosterError::Decode);
    };
    Ok(Decoded { w: w16, h: h16, rgba: rgba.into_boxed_slice() })
}

/// The size an `iw`x`ih` image scales DOWN to so it covers `w`x`h` (the box's short side is
/// filled, the long side overhangs and is cropped by the tile). An image already inside the box,
/// or a box with no size, is left alone.
pub fn cover_size(iw: u32, ih: u32, w: i32, h: i32) -> (u32, u32) {
    if w <= 0 || h <= 0 || iw == 0 || ih == 0 {
        return (iw, ih);
    }
    let scale = (w as f64 / iw as f64).max(h as f64 / ih as f64);
    if scale >= 1.0 {
        return (iw, ih);
    }
    (((iw as f64 * scale).ceil() as u32).max(1), ((ih as f64 * scale).ceil() as u32).max(1))
}

// ---------------------------------------------------------------------------------------------
// The frame thread's half: interning, admission, the LRU.

/// One fetch to start.
pub struct Job {
    pub key: u32,
    pub url: String,
    pub w: i32,
    pub h: i32,
    pub cancel: Cancel,
}

/// What a worker posts back.
pub struct Fetched {
    pub key: u32,
    pub outcome: Outcome,
}

/// Starts one job. The product spawns a worker; a test records it.
pub trait Starter {
    fn start(&self, job: Job, tx: Sender<Fetched>) -> bool;
}

/// One small-stack worker per job, over `NetFetch` (`Profile::Art`, WebP-first `Accept`) and
/// the imgcache disk tier.
pub struct Workers;

impl Starter for Workers {
    fn start(&self, job: Job, tx: Sender<Fetched>) -> bool {
        sn_base::task::spawn_off_frame("art", move |off: &OffFrame| {
            let fetch = NetFetch::new(off, Profile::Art).own_headers(ART_ACCEPT).cancelled_by(job.cancel.clone());
            let outcome = load(&fetch, &ImgCacheDisk, &job.url, job.w, job.h, &job.cancel);
            let _ = tx.send(Fetched { key: job.key, outcome });
            sn_machine::present::wake_from_worker();
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Asked for, waiting for a worker.
    Wanted,
    /// Out on a worker; `prefetch` says which bound it counts against.
    Fetching { prefetch: bool },
    /// Handed to the cache.
    Ready,
    /// A transient failure, parked until then.
    Retry(Instant),
    /// A settled miss, until then.
    Failed(Instant),
    /// The cache refused residency, cooling until then.
    Refused(Instant),
}

struct Slot {
    /// The sized URL (what is fetched and what the disk tier is keyed by).
    url: String,
    w: i32,
    h: i32,
    state: State,
    /// The frame a draw last asked for it; 0 = only ever prefetched.
    drawn: u64,
    born: u64,
    attempts: u32,
    /// Settled misses since the pixels last arrived (the first is retried sooner).
    settled: u32,
    /// The key has missed (settled, or [`FALLBACK_AFTER_ATTEMPTS`] transient failures): it reads
    /// as failed until its pixels arrive, retries included.
    missed: bool,
    cancel: Cancel,
}

/// What the source has done, for the heartbeat and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub disk_hits: u64,
    pub fetched: u64,
    pub retries: u64,
    pub failed: u64,
    pub cancelled: u64,
}

#[derive(Default)]
struct Inner {
    by_req: HashMap<(String, i32, i32), u32>,
    slots: HashMap<u32, Slot>,
    next: u32,
    frame: u64,
    visible_out: usize,
    prefetch_out: usize,
    /// Keys forgotten while their pixels were with the cache: freed on the next prepare.
    freed: Vec<PosterKey>,
    stats: Stats,
}

impl Inner {
    fn recent(&self, s: &Slot) -> bool {
        s.drawn > 0 && s.drawn + CANCEL_FRAMES >= self.frame
    }

    /// Forget `key` (and free its texture if the cache holds it).
    fn forget(&mut self, key: u32) {
        if let Some(slot) = self.slots.remove(&key) {
            slot.cancel.cancel();
            self.by_req.retain(|_, k| *k != key);
            if slot.state == State::Ready {
                self.freed.push(PosterKey(key));
            }
        }
    }

    /// Forget the least recently drawn slot that is not out on a worker and was not drawn in
    /// this frame or the last. `false` when every slot is protected.
    fn evict_one(&mut self) -> bool {
        let frame = self.frame;
        let victim = self
            .slots
            .iter()
            .filter(|(_, s)| !matches!(s.state, State::Fetching { .. }) && s.drawn + 1 < frame.max(1))
            .min_by_key(|(k, s)| (s.drawn, s.born, **k))
            .map(|(k, _)| *k);
        match victim {
            Some(k) => {
                self.forget(k);
                true
            }
            None => false,
        }
    }
}

pub struct Art {
    inner: RefCell<Inner>,
    tx: Sender<Fetched>,
    rx: Receiver<Fetched>,
    starter: Box<dyn Starter>,
}

/// How a request touches its slot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Touch {
    Draw,
    Warm,
}

impl Art {
    pub fn new(starter: Box<dyn Starter>) -> Self {
        let (tx, rx) = channel();
        Art { inner: RefCell::new(Inner { next: 1, frame: 1, ..Inner::default() }), tx, rx, starter }
    }

    /// Install a leaked source as the render cache's (once, at boot, on the frame thread).
    pub fn install(self) -> &'static Art {
        let art: &'static Art = Box::leak(Box::new(self));
        tex::install(art);
        art
    }

    pub fn stats(&self) -> Stats {
        self.inner.borrow().stats
    }

    /// Once a frame, on the frame thread, before the present decision: advance the frame, fold
    /// every finished job into the cache, cancel what scrolled away, start what is wanted.
    pub fn drain(&self) {
        self.inner.borrow_mut().frame += 1;
        for done in self.rx.try_iter().collect::<Vec<_>>() {
            let accept = {
                let mut inner = self.inner.borrow_mut();
                let prefetch = match inner.slots.get(&done.key).map(|s| s.state) {
                    Some(State::Fetching { prefetch }) => Some(prefetch),
                    _ => None,
                };
                match prefetch {
                    Some(true) => inner.prefetch_out = inner.prefetch_out.saturating_sub(1),
                    Some(false) => inner.visible_out = inner.visible_out.saturating_sub(1),
                    // forgotten (or re-armed) while it was out: its bound was released then
                    None => continue,
                }
                let now = Instant::now();
                let Inner { slots, stats, .. } = &mut *inner;
                let slot = slots.get_mut(&done.key).expect("found above");
                match done.outcome {
                    Outcome::Ready(d, origin) => {
                        slot.state = State::Ready;
                        slot.attempts = 0;
                        slot.settled = 0;
                        slot.missed = false;
                        match origin {
                            Origin::Disk => stats.disk_hits += 1,
                            Origin::Network => stats.fetched += 1,
                        }
                        Some(Ok(d))
                    }
                    Outcome::Failed { transient: true, .. } => {
                        slot.attempts = slot.attempts.saturating_add(1);
                        let backoff = (RETRY_MIN_MS << (slot.attempts - 1).min(8)).min(RETRY_MAX_MS);
                        slot.state = State::Retry(now + Duration::from_millis(backoff));
                        slot.missed |= slot.attempts >= FALLBACK_AFTER_ATTEMPTS;
                        stats.retries += 1;
                        None
                    }
                    Outcome::Failed { transient: false, error } => {
                        slot.settled = slot.settled.saturating_add(1);
                        slot.missed = true;
                        let hold = if slot.settled == 1 { SETTLED_RETRY_MS } else { FAILED_MS };
                        slot.state = State::Failed(now + Duration::from_millis(hold));
                        stats.failed += 1;
                        Some(Err(error))
                    }
                    Outcome::Cancelled => {
                        slot.state = State::Wanted;
                        slot.cancel = Cancel::default();
                        stats.cancelled += 1;
                        None
                    }
                }
            };
            if let Some(result) = accept {
                tex::accept(PosterReady { key: PosterKey(done.key), result });
            }
        }
        self.sweep();
        self.pump();
    }

    /// Cancel what nobody is looking at any more.
    fn sweep(&self) {
        let mut inner = self.inner.borrow_mut();
        let frame = inner.frame;
        let mut gone = Vec::new();
        for (k, s) in &inner.slots {
            let stale_draw = s.drawn > 0 && s.drawn + CANCEL_FRAMES < frame;
            let stale_warm = s.drawn == 0 && s.born + WARM_KEEP_FRAMES < frame;
            match s.state {
                State::Wanted if stale_draw || stale_warm => gone.push(*k),
                State::Fetching { .. } if stale_draw || stale_warm => s.cancel.cancel(),
                _ => {}
            }
        }
        for k in gone {
            inner.forget(k);
            inner.stats.cancelled += 1;
        }
    }

    /// The keys forgotten while resident, for the frame thread to free (`tex::free`).
    pub fn take_freed(&self) -> Vec<PosterKey> {
        std::mem::take(&mut self.inner.borrow_mut().freed)
    }

    /// Start wanted keys: the most recently drawn first, within the visible bound; then one
    /// prefetch, oldest claim first.
    fn pump(&self) {
        loop {
            let job = {
                let mut inner = self.inner.borrow_mut();
                let pick = |inner: &Inner, prefetch: bool| {
                    inner
                        .slots
                        .iter()
                        .filter(|(_, s)| s.state == State::Wanted)
                        .filter(|(_, s)| if prefetch { s.drawn == 0 } else { inner.recent(s) })
                        .min_by_key(|(k, s)| if prefetch { (0, s.born, **k) } else { (u64::MAX - s.drawn, 0, **k) })
                        .map(|(k, _)| *k)
                };
                let (key, prefetch) = if inner.visible_out < VISIBLE_INFLIGHT_MAX {
                    match pick(&inner, false) {
                        Some(k) => (k, false),
                        None if inner.prefetch_out < PREFETCH_INFLIGHT_MAX => match pick(&inner, true) {
                            Some(k) => (k, true),
                            None => return,
                        },
                        None => return,
                    }
                } else if inner.prefetch_out < PREFETCH_INFLIGHT_MAX {
                    match pick(&inner, true) {
                        Some(k) => (k, true),
                        None => return,
                    }
                } else {
                    return;
                };
                if prefetch {
                    inner.prefetch_out += 1;
                } else {
                    inner.visible_out += 1;
                }
                let slot = inner.slots.get_mut(&key).expect("just picked");
                slot.state = State::Fetching { prefetch };
                Job { key, url: slot.url.clone(), w: slot.w, h: slot.h, cancel: slot.cancel.clone() }
            };
            let key = job.key;
            if !self.starter.start(job, self.tx.clone()) {
                let mut inner = self.inner.borrow_mut();
                let prefetch = matches!(inner.slots.get(&key).map(|s| s.state), Some(State::Fetching { prefetch: true }));
                if prefetch {
                    inner.prefetch_out = inner.prefetch_out.saturating_sub(1);
                } else {
                    inner.visible_out = inner.visible_out.saturating_sub(1);
                }
                if let Some(slot) = inner.slots.get_mut(&key) {
                    // the OS refused a thread: a transient condition, retried with backoff
                    slot.attempts = slot.attempts.saturating_add(1);
                    slot.state = State::Retry(Instant::now() + Duration::from_millis(RETRY_MIN_MS));
                }
                return;
            }
        }
    }

    /// The key for a request, interning it on a miss. `None` when a prefetch could not be given
    /// a slot. The bool says the slot is new.
    fn ask(&self, path: &str, w: i32, h: i32, touch: Touch) -> Option<(u32, bool)> {
        let mut inner = self.inner.borrow_mut();
        let frame = inner.frame;
        let req = (path.to_string(), w, h);
        if let Some(&key) = inner.by_req.get(&req) {
            let slot = inner.slots.get_mut(&key).expect("interned keys have slots");
            if touch == Touch::Draw {
                slot.drawn = frame;
                let now = Instant::now();
                match slot.state {
                    State::Retry(at) | State::Failed(at) | State::Refused(at) if now >= at => slot.state = State::Wanted,
                    _ => {}
                }
            }
            return Some((key, false));
        }
        if inner.slots.len() >= SLOTS_MAX && !inner.evict_one() && touch == Touch::Warm {
            return None; // every slot is on screen: no room for speculation
        }
        let key = inner.next;
        inner.next = inner.next.wrapping_add(1).max(1);
        inner.by_req.insert(req, key);
        let url = sized_url(path, w, h).into_owned();
        let drawn = if touch == Touch::Draw { frame } else { 0 };
        inner.slots.insert(key, Slot { url, w, h, state: State::Wanted, drawn, born: frame, attempts: 0, settled: 0, missed: false, cancel: Cancel::default() });
        Some((key, true))
    }

    fn state(&self, key: u32) -> Option<State> {
        self.inner.borrow().slots.get(&key).map(|s| s.state)
    }
}

impl tex::Source for Art {
    fn probe(&self, _srv: u16, path: &str, w: i32, h: i32, _png: bool) -> Option<PosterKey> {
        if path.is_empty() {
            return None;
        }
        let (key, _) = self.ask(path, w, h, Touch::Draw)?;
        self.pump();
        (self.state(key) == Some(State::Ready)).then_some(PosterKey(key))
    }

    fn warm(&self, _srv: u16, path: &str, w: i32, h: i32, _png: bool) -> Warm {
        if path.is_empty() {
            return Warm::Known;
        }
        let Some((_, new)) = self.ask(path, w, h, Touch::Warm) else { return Warm::Full };
        self.pump();
        if new {
            Warm::Claimed
        } else {
            Warm::Known
        }
    }

    fn logo(&self, _srv: u16, _rk: &str) -> Option<PosterKey> {
        None
    }

    fn logo_warm(&self, _srv: u16, _rk: &str) -> Warm {
        Warm::Known
    }

    fn logo_failed(&self, _srv: u16, _rk: &str) -> bool {
        true
    }

    fn failed(&self, _srv: u16, path: &str, w: i32, h: i32, _png: bool) -> bool {
        let inner = self.inner.borrow();
        inner
            .by_req
            .get(&(path.to_string(), w, h))
            .and_then(|k| inner.slots.get(k))
            .is_some_and(|s| s.missed && s.state != State::Ready)
    }

    fn unresident(&self, key: PosterKey, refused: bool) {
        let mut inner = self.inner.borrow_mut();
        // Only a READY claim is revoked. A rejected result (`tex::accept` of a settled miss)
        // reaches here too, and its `Failed` hold must stand: demoting it to `Wanted` refetched a
        // 404 / undecodable image on every draw and kept the frame from ever settling.
        if let Some(slot) = inner.slots.get_mut(&key.0).filter(|s| s.state == State::Ready) {
            // Stop answering READY now. A refused arrival cools down; a lost texture is asked for
            // again by the next draw that wants it (from the disk tier, so cheaply).
            slot.state = if refused { State::Refused(Instant::now() + Duration::from_millis(REFUSED_MS)) } else { State::Wanted };
        }
    }

    fn idle(&self) -> bool {
        let inner = self.inner.borrow();
        inner.visible_out == 0
            && inner.prefetch_out == 0
            && !inner.slots.values().any(|s| s.state == State::Wanted && (inner.recent(s) || s.drawn == 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_ui::tex::Source;
    use std::sync::{Arc, Mutex};

    /// Records the jobs started; `ok` decides whether they start at all.
    struct Recorder {
        started: Arc<Mutex<Vec<(String, u32, Cancel)>>>,
        ok: bool,
    }
    impl Starter for Recorder {
        fn start(&self, job: Job, _tx: Sender<Fetched>) -> bool {
            if self.ok {
                self.started.lock().unwrap().push((job.url, job.key, job.cancel));
            }
            self.ok
        }
    }

    type Started = Arc<Mutex<Vec<(String, u32, Cancel)>>>;

    fn art(ok: bool) -> (Art, Started) {
        let started = Arc::new(Mutex::new(Vec::new()));
        (Art::new(Box::new(Recorder { started: started.clone(), ok })), started)
    }

    fn keys(started: &Started) -> Vec<u32> {
        started.lock().unwrap().iter().map(|(_, k, _)| *k).collect()
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// A photographic-ish test card: gradients and noise, so the codecs do real work.
    fn photo(w: u32, h: u32) -> image::RgbImage {
        let mut seed = 0x1234_5678u32;
        image::RgbImage::from_fn(w, h, |x, y| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let n = (seed >> 24) as u8 / 8;
            image::Rgb([(x * 255 / w) as u8 ^ n, (y * 255 / h) as u8, ((x + y) % 256) as u8 ^ n])
        })
    }

    fn encode(img: &image::RgbImage, format: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    fn ready(art: &Art, key: u32, w: u32, h: u32) {
        art.tx.send(Fetched { key, outcome: Outcome::Ready(decode_cover(&png(w, h), 250, 375).unwrap(), Origin::Network) }).unwrap();
    }

    #[test]
    fn known_hosts_are_asked_for_the_smallest_variant_that_covers_the_box() {
        let card = |u: &str| sized_url(u, 250, 375).into_owned();
        let still = |u: &str| sized_url(u, 420, 236).into_owned();
        let hero = |u: &str| sized_url(u, 1920, 1080).into_owned();
        let meta = "https://images.metahub.space/poster/small/tt0903747/img";
        assert_eq!(card(meta), "https://images.metahub.space/poster/medium/tt0903747/img");
        assert_eq!(hero(meta), "https://images.metahub.space/poster/large/tt0903747/img");
        assert_eq!(card("https://images.metahub.space/poster/large/tt1/img"), "https://images.metahub.space/poster/medium/tt1/img");
        let tmdb = "https://image.tmdb.org/t/p/original/abc.jpg";
        assert_eq!(card(tmdb), "https://image.tmdb.org/t/p/w342/abc.jpg");
        assert_eq!(still(tmdb), "https://image.tmdb.org/t/p/w500/abc.jpg");
        assert_eq!(hero("https://image.tmdb.org/t/p/w92/abc.jpg"), "https://image.tmdb.org/t/p/w1280/abc.jpg");
        assert_eq!(
            card("https://m.media-amazon.com/images/M/MV5BMjA5._V1_QL75_UX380_CR0,1,380,562_.jpg"),
            "https://m.media-amazon.com/images/M/MV5BMjA5._V1_UX300_.jpg"
        );
        assert_eq!(card("https://m.media-amazon.com/images/M/MV5BMjA5._V1_.jpg"), "https://m.media-amazon.com/images/M/MV5BMjA5._V1_UX300_.jpg");
        assert_eq!(hero("http://ia.media-imdb.com/images/M/X._V1_SX300.png"), "http://ia.media-imdb.com/images/M/X._V1_UX1280_.png");
        for untouched in [
            "https://images.metahub.space/background/medium/tt1/img",
            "https://images.metahub.space/poster/huge/tt1/img",
            "https://image.tmdb.org/t/p/w9999/abc.jpg",
            "https://image.tmdb.org/t/p/w342/abc.jpg?v=2",
            "https://m.media-amazon.com/images/M/MV5BMjA5.jpg",
            "https://cdn.example/poster/small/x.jpg",
            "https://addon.example/cfg/abc/poster.webp",
            "not a url",
            "ftp://images.metahub.space/poster/small/tt1/img",
        ] {
            assert_eq!(card(untouched), untouched, "only the known patterns are rewritten");
        }
    }

    #[test]
    fn a_draw_asks_once_and_is_answered_only_once_the_pixels_arrive() {
        let _g = sn_base::testlock::serial();
        tex::reset_for_test(64 << 20);
        let (art, started) = art(true);
        assert_eq!(art.probe(0, "https://img.example/a.jpg", 250, 375, false), None, "a miss starts the fetch");
        assert_eq!(art.probe(0, "https://img.example/a.jpg", 250, 375, false), None, "asking again starts nothing more");
        assert_eq!(keys(&started).len(), 1);
        let key = keys(&started)[0];
        ready(&art, key, 500, 750);
        art.drain();
        assert_eq!(art.probe(0, "https://img.example/a.jpg", 250, 375, false), Some(PosterKey(key)));
        assert!(art.idle());
        assert_eq!(art.stats().fetched, 1);
        tex::reset_for_test(64 << 20);
    }

    #[test]
    fn the_request_is_the_sized_variant() {
        let _g = sn_base::testlock::serial();
        let (art, started) = art(true);
        art.probe(0, "https://image.tmdb.org/t/p/original/p.jpg", 250, 375, false);
        assert_eq!(started.lock().unwrap()[0].0, "https://image.tmdb.org/t/p/w342/p.jpg");
    }

    #[test]
    fn visible_fetches_are_bounded_newest_drawn_first_and_a_prefetch_takes_one_more_worker() {
        let _g = sn_base::testlock::serial();
        let (art, started) = art(true);
        for i in 0..VISIBLE_INFLIGHT_MAX + 3 {
            art.probe(0, &format!("https://img.example/{i}.jpg"), 250, 375, false);
        }
        assert_eq!(keys(&started).len(), VISIBLE_INFLIGHT_MAX, "never more than the bound out for draws");
        assert_eq!(art.warm(0, "https://img.example/ahead.jpg", 250, 375, false), Warm::Claimed);
        assert_eq!(keys(&started).len(), VISIBLE_INFLIGHT_MAX + 1, "one prefetch rides its own worker");
        assert_eq!(art.warm(0, "https://img.example/ahead2.jpg", 250, 375, false), Warm::Claimed);
        assert_eq!(keys(&started).len(), VISIBLE_INFLIGHT_MAX + 1, "and only one");
        // next frame: only the last card is still drawn; one visible fetch comes back
        art.drain();
        art.probe(0, &format!("https://img.example/{}.jpg", VISIBLE_INFLIGHT_MAX + 2), 250, 375, false);
        let first = keys(&started)[0];
        art.tx.send(Fetched { key: first, outcome: Outcome::Failed { transient: false, error: PosterError::Fetch } }).unwrap();
        art.drain();
        let last = started.lock().unwrap().last().map(|(u, _, _)| u.clone()).unwrap();
        assert_eq!(last, format!("https://img.example/{}.jpg", VISIBLE_INFLIGHT_MAX + 2), "the most recently drawn waiting key starts next");
    }

    #[test]
    fn a_row_scrolled_away_is_cancelled_queued_and_in_flight() {
        let _g = sn_base::testlock::serial();
        let (art, started) = art(true);
        for i in 0..VISIBLE_INFLIGHT_MAX + 4 {
            art.probe(0, &format!("https://img.example/{i}.jpg"), 250, 375, false);
        }
        assert_eq!(keys(&started).len(), VISIBLE_INFLIGHT_MAX);
        // the row leaves the screen: nothing draws it again
        for _ in 0..=CANCEL_FRAMES + 1 {
            art.drain();
        }
        assert!(started.lock().unwrap().iter().all(|(_, _, c)| c.is_cancelled()), "the requests out were told to stop");
        assert_eq!(keys(&started).len(), VISIBLE_INFLIGHT_MAX, "the queued ones never started");
        assert!(art.stats().cancelled >= 4);
        // they come back cancelled, and the workers go to what is on screen now
        for k in keys(&started) {
            art.tx.send(Fetched { key: k, outcome: Outcome::Cancelled }).unwrap();
        }
        art.probe(0, "https://img.example/now-on-screen.jpg", 250, 375, false);
        art.drain();
        assert_eq!(started.lock().unwrap().last().unwrap().0, "https://img.example/now-on-screen.jpg");
    }

    #[test]
    fn a_transient_failure_backs_off_and_a_draw_after_it_asks_again() {
        let _g = sn_base::testlock::serial();
        let (art, started) = art(true);
        let url = "https://img.example/flaky.jpg";
        art.probe(0, url, 250, 375, false);
        let key = keys(&started)[0];
        art.tx.send(Fetched { key, outcome: Outcome::Failed { transient: true, error: PosterError::Fetch } }).unwrap();
        art.drain();
        art.probe(0, url, 250, 375, false);
        assert_eq!(keys(&started).len(), 1, "inside the backoff nothing is asked");
        assert!(!art.failed(0, url, 250, 375, false), "a transient failure has not settled: the skeleton stays");
        std::thread::sleep(Duration::from_millis(RETRY_MIN_MS + 30));
        art.probe(0, url, 250, 375, false);
        assert_eq!(keys(&started).len(), 2, "the first draw after the backoff asks again");
        assert_eq!(art.stats().retries, 1);
    }

    #[test]
    fn a_settled_miss_is_reported_and_not_refetched() {
        let _g = sn_base::testlock::serial();
        tex::reset_for_test(64 << 20);
        let (art, started) = art(true);
        let url = "https://img.example/gone.jpg";
        art.probe(0, url, 250, 375, false);
        let key = keys(&started)[0];
        art.tx.send(Fetched { key, outcome: Outcome::Failed { transient: false, error: PosterError::Decode } }).unwrap();
        art.drain();
        for _ in 0..10 {
            assert_eq!(art.probe(0, url, 250, 375, false), None);
        }
        assert_eq!(keys(&started).len(), 1);
        assert!(art.failed(0, url, 250, 375, false));
        tex::reset_for_test(64 << 20);
    }

    /// The same settled miss with the source INSTALLED, as in the product: `tex::accept`'s
    /// rejected result calls back into `unresident`, which must not undo the miss's hold. It did
    /// (`State::Failed` -> `Wanted`), so every draw of a 404 / undecodable poster refetched it,
    /// the source never went idle and the frame never settled.
    #[test]
    fn a_settled_miss_round_tripped_through_the_render_cache_is_not_refetched() {
        let _g = sn_base::testlock::serial();
        tex::reset_for_test(64 << 20);
        let (art, started) = art(true);
        let art = art.install();
        let url = "https://img.example/404.jpg";
        art.probe(0, url, 250, 375, false);
        let key = keys(&started)[0];
        art.tx.send(Fetched { key, outcome: Outcome::Failed { transient: false, error: PosterError::Fetch } }).unwrap();
        art.drain();
        for _ in 0..10 {
            assert_eq!(art.probe(0, url, 250, 375, false), None);
            art.drain();
        }
        assert_eq!(keys(&started).len(), 1, "a settled miss is held, not refetched every frame");
        assert!(art.failed(0, url, 250, 375, false));
        assert!(art.idle(), "nothing is wanted: the frame may settle");
        tex::reset_for_test(64 << 20);
    }

    /// Move key's parked state to "due now", as if its hold had run out.
    fn expire(art: &Art, key: u32) {
        let mut inner = art.inner.borrow_mut();
        let slot = inner.slots.get_mut(&key).unwrap();
        slot.state = match slot.state {
            State::Failed(_) => State::Failed(Instant::now()),
            State::Retry(_) => State::Retry(Instant::now()),
            s => s,
        };
    }

    fn hold_left(art: &Art, key: u32) -> Duration {
        match art.state(key) {
            Some(State::Failed(at) | State::Retry(at)) => at.saturating_duration_since(Instant::now()),
            s => panic!("not parked: {s:?}"),
        }
    }

    /// A first settled miss (a 404) is asked ONCE more, soon, by the next draw that wants it (the
    /// card back in view), and the tile keeps reading failed (its fallback) through that retry;
    /// a second miss settles for the full hold; pixels arriving clear it.
    #[test]
    fn a_settled_miss_is_retried_once_by_a_later_draw_and_keeps_its_fallback_meanwhile() {
        let _g = sn_base::testlock::serial();
        tex::reset_for_test(64 << 20);
        let (art, started) = art(true);
        let url = "https://img.example/sometimes-404.jpg";
        let miss = |key| art.tx.send(Fetched { key, outcome: Outcome::Failed { transient: false, error: PosterError::Fetch } }).unwrap();
        art.probe(0, url, 250, 375, false);
        let key = keys(&started)[0];
        miss(key);
        art.drain();
        assert!(art.failed(0, url, 250, 375, false), "settled: the fallback");
        assert!(hold_left(&art, key) <= Duration::from_millis(SETTLED_RETRY_MS), "the first miss is held briefly");
        expire(&art, key);
        art.drain(); // nobody drew it: nothing is asked
        assert_eq!(keys(&started).len(), 1);
        art.probe(0, url, 250, 375, false);
        assert_eq!(keys(&started).len(), 2, "the draw after the hold asks once more");
        assert!(art.failed(0, url, 250, 375, false), "while it is out the tile keeps its fallback");
        miss(key);
        art.drain();
        assert!(hold_left(&art, key) > Duration::from_millis(SETTLED_RETRY_MS), "a second miss settles for long");
        expire(&art, key);
        art.probe(0, url, 250, 375, false);
        assert_eq!(keys(&started).len(), 3);
        ready(&art, key, 250, 375);
        art.drain();
        assert!(!art.failed(0, url, 250, 375, false), "the pixels arrived");
        tex::reset_for_test(64 << 20);
    }

    /// A host that keeps failing transiently (a timeout, a 503, an empty body) keeps being asked
    /// with backoff, but after a few tries the tile shows its fallback instead of a skeleton.
    #[test]
    fn repeated_transient_failures_read_failed_while_the_backoff_keeps_asking() {
        let _g = sn_base::testlock::serial();
        tex::reset_for_test(64 << 20);
        let (art, started) = art(true);
        let url = "https://img.example/down.jpg";
        art.probe(0, url, 250, 375, false);
        let key = keys(&started)[0];
        for n in 1..=FALLBACK_AFTER_ATTEMPTS {
            assert!(!art.failed(0, url, 250, 375, false), "attempt {n}: still a wait");
            art.tx.send(Fetched { key, outcome: Outcome::Failed { transient: true, error: PosterError::Fetch } }).unwrap();
            art.drain();
            expire(&art, key);
            art.probe(0, url, 250, 375, false);
        }
        assert!(art.failed(0, url, 250, 375, false), "the fallback, after {FALLBACK_AFTER_ATTEMPTS} transient failures");
        assert_eq!(keys(&started).len() as u32, FALLBACK_AFTER_ATTEMPTS + 1, "and it is still asked");
        ready(&art, key, 250, 375);
        art.drain();
        assert!(!art.failed(0, url, 250, 375, false));
        tex::reset_for_test(64 << 20);
    }

    #[test]
    fn a_worker_refused_by_the_os_is_retried_with_backoff() {
        let _g = sn_base::testlock::serial();
        let (art, _) = art(false);
        art.probe(0, "https://img.example/a.jpg", 250, 375, false);
        assert!(!art.failed(0, "https://img.example/a.jpg", 250, 375, false));
        assert!(art.idle(), "nothing spins while it waits");
    }

    #[test]
    fn eviction_takes_the_least_recently_drawn_and_never_what_is_on_screen() {
        let _g = sn_base::testlock::serial();
        tex::reset_for_test(64 << 20);
        let (art, started) = art(true);
        art.probe(0, "https://img.example/old.jpg", 250, 375, false);
        let old = keys(&started)[0];
        ready(&art, old, 10, 15);
        art.drain();
        art.drain();
        for i in 0..SLOTS_MAX {
            art.probe(0, &format!("https://img.example/{i}.jpg"), 250, 375, false);
            for k in std::mem::take(&mut *started.lock().unwrap()).into_iter().map(|(_, k, _)| k) {
                art.tx.send(Fetched { key: k, outcome: Outcome::Failed { transient: false, error: PosterError::Fetch } }).unwrap();
            }
        }
        assert_eq!(art.take_freed(), vec![PosterKey(old)], "the oldest READY key leaves, and its texture with it");
        assert!(art.inner.borrow().slots.len() <= SLOTS_MAX + 1);
        // a screenful drawn THIS frame is never a victim, even past the bound
        art.drain();
        art.drain();
        let screen: Vec<String> = (0..8).map(|i| format!("https://img.example/screen{i}.jpg")).collect();
        for u in &screen {
            art.probe(0, u, 250, 375, false);
        }
        for u in &screen {
            assert!(art.inner.borrow().by_req.contains_key(&(u.clone(), 250, 375)), "{u} survived its own frame");
        }
        tex::reset_for_test(64 << 20);
    }

    /// In-memory disk tier.
    #[derive(Default)]
    struct MemDisk(Mutex<HashMap<(String, u32, u32), Vec<u8>>>);
    impl Disk for MemDisk {
        fn read(&self, url: &str, w: u32, h: u32) -> Option<Vec<u8>> {
            self.0.lock().unwrap().get(&(url.into(), w, h)).cloned()
        }
        fn write(&self, url: &str, w: u32, h: u32, bytes: &[u8]) {
            self.0.lock().unwrap().insert((url.into(), w, h), bytes.to_vec());
        }
        fn remove(&self, url: &str, w: u32, h: u32) {
            self.0.lock().unwrap().remove(&(url.into(), w, h));
        }
    }

    /// Serves one body (or one error) and counts the requests.
    struct OneBody {
        body: Result<Vec<u8>, FetchError>,
        asked: Mutex<u32>,
    }
    impl Fetch for OneBody {
        fn get(&self, _: &str, _: &[(String, String)]) -> Result<Vec<u8>, FetchError> {
            *self.asked.lock().unwrap() += 1;
            self.body.clone()
        }
        fn post(&self, _: &str, _: &[(String, String)], _: &[u8]) -> Result<Vec<u8>, FetchError> {
            unreachable!()
        }
    }

    #[test]
    fn a_revisit_is_served_from_disk_with_no_fetch_and_no_decode() {
        let disk = MemDisk::default();
        let jpeg = encode(&photo(600, 900), image::ImageFormat::Jpeg);
        let net = OneBody { body: Ok(jpeg), asked: Mutex::new(0) };
        let url = "https://img.example/p.jpg";
        let first = load(&net, &disk, url, 250, 375, &Cancel::default());
        let Outcome::Ready(a, Origin::Network) = first else { panic!("the first visit fetches") };
        assert_eq!((a.w, a.h), (250, 375), "scaled on the worker before it is kept");
        let stored = disk.read(url, 250, 375).expect("the scaled pixels were kept");
        assert_eq!(stored.len(), PX_HEAD + 250 * 375 * 3, "an opaque poster is kept as RGB");
        let down = OneBody { body: Err(FetchError::Transport("offline".into())), asked: Mutex::new(0) };
        let Outcome::Ready(b, Origin::Disk) = load(&down, &disk, url, 250, 375, &Cancel::default()) else { panic!("the revisit is a disk hit") };
        assert_eq!(*down.asked.lock().unwrap(), 0, "no request at all");
        assert_eq!(a.rgba, b.rgba, "the same pixels");
        // a corrupt disk entry is dropped and fetched again
        disk.write(url, 250, 375, b"SNPX1\0garbage");
        assert!(matches!(load(&net, &disk, url, 250, 375, &Cancel::default()), Outcome::Ready(_, Origin::Network)));
    }

    #[test]
    fn failures_are_classified_and_a_cancelled_load_never_dials() {
        let disk = MemDisk::default();
        let cancelled = Cancel::default();
        cancelled.cancel();
        let net = OneBody { body: Ok(b"x".to_vec()), asked: Mutex::new(0) };
        assert!(matches!(load(&net, &disk, "https://x/a.jpg", 250, 375, &cancelled), Outcome::Cancelled));
        assert_eq!(*net.asked.lock().unwrap(), 0);
        let case = |e: FetchError| load(&OneBody { body: Err(e), asked: Mutex::new(0) }, &disk, "https://x/a.jpg", 250, 375, &Cancel::default());
        assert!(matches!(case(FetchError::Status(503)), Outcome::Failed { transient: true, .. }));
        assert!(matches!(case(FetchError::Transport("timed out".into())), Outcome::Failed { transient: true, .. }));
        assert!(matches!(case(FetchError::Status(404)), Outcome::Failed { transient: false, .. }));
        let junk = OneBody { body: Ok(b"<html>not an image</html>".to_vec()), asked: Mutex::new(0) };
        assert!(matches!(load(&junk, &disk, "https://x/b.jpg", 250, 375, &Cancel::default()), Outcome::Failed { transient: false, error: PosterError::Decode }));
        assert!(disk.read("https://x/b.jpg", 250, 375).is_none(), "a bad answer is never kept");
    }

    #[test]
    fn webp_jpeg_and_png_all_decode_and_scale_to_cover() {
        let img = photo(300, 450);
        for format in [image::ImageFormat::Jpeg, image::ImageFormat::Png, image::ImageFormat::WebP] {
            let d = decode_cover(&encode(&img, format), 250, 375).unwrap_or_else(|e| panic!("{format:?}: {e:?}"));
            assert_eq!((d.w, d.h), (250, 375), "{format:?}");
        }
        assert_eq!(cover_size(1000, 1500, 250, 375), (250, 375));
        assert_eq!(cover_size(1920, 1080, 250, 375), (667, 375), "a wide image keeps its height and overhangs");
        assert_eq!(cover_size(200, 300, 250, 375), (200, 300), "never scaled up");
        assert!(matches!(decode_cover(b"not an image", 250, 375), Err(PosterError::Decode)));
        let d = decode_cover(&png(4, 4), 250, 375).unwrap();
        assert_eq!(decode_pixels(&encode_pixels(&d)).map(|x| x.rgba), Some(d.rgba.clone()));
    }

    /// Decode + cover-scale cost of one card on this host. Informational; run with
    /// `cargo test --release -p stremionative-app art::tests::decode_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn decode_timing() {
        let img = photo(300, 450);
        for (name, format) in [("jpeg", image::ImageFormat::Jpeg), ("webp(lossless)", image::ImageFormat::WebP)] {
            let body = encode(&img, format);
            let n = 200;
            let t = Instant::now();
            for _ in 0..n {
                std::hint::black_box(decode_cover(&body, 250, 375).unwrap());
            }
            let per = t.elapsed() / n;
            let d = decode_cover(&body, 250, 375).unwrap();
            let px = encode_pixels(&d);
            let t = Instant::now();
            for _ in 0..n {
                std::hint::black_box(decode_pixels(&px).unwrap());
            }
            eprintln!("art timing: {name} 300x450 {} bytes: decode+scale {per:?}/poster; disk-tier read {:?}/poster", body.len(), t.elapsed() / n);
        }
    }
}
