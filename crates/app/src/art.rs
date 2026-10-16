//! **The artwork source** — `sn_ui::tex::Source` for the addons' posters, the render cache's
//! other half (`sn_ui::tex`'s module doc).
//!
//! A tile asks for `(url, w, h)` while it draws; this interns the request into a [`PosterKey`],
//! fetches the URL on a worker ([`NetFetch`], `Profile::Art`), decodes it, scales it down to
//! COVER the requested box (the shape a Plex transcode answers with, which the tile then crops),
//! and hands the pixels to the frame thread, which gives them to the cache ([`Art::drain`] →
//! `tex::accept`). The upload itself is the cache's, in the dispatcher's prepare pass.
//!
//! Bounded on both sides: at most [`INFLIGHT_MAX`] fetches at once (newest request first, so the
//! row the reader is looking at wins over the one scrolled past), and at most [`SLOTS_MAX`] keys
//! remembered — the least recently drawn is forgotten, and its texture freed on the next prepare.
//! A failed fetch is remembered as failed for [`RETRY_MS`] so a broken URL is not hammered every
//! frame; a key the cache could not keep resident cools down for the same time.
//!
//! Addons have no clearLogo, so `logo` answers nothing and `logo_failed` says settled.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use sn_base::task::OffFrame;
use sn_machine::machine::PosterKey;
use sn_stremio::Fetch;
use sn_ui::tex::{self, Decoded, PosterError, PosterReady, Warm};

use crate::fetch::{NetFetch, Profile};

/// Fetches out at once.
pub const INFLIGHT_MAX: usize = 6;
/// Keys remembered (each a poster's texture at most, ~375 KB at 250x375).
pub const SLOTS_MAX: usize = 160;
/// How long a failed or refused key stays settled before a draw may ask again.
pub const RETRY_MS: u64 = 15_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Asked for, waiting for a fetch slot.
    Wanted,
    Fetching,
    /// Handed to the cache.
    Ready,
    /// Settled as a miss (or refused residency) until `until`.
    Cooling(Instant),
}

struct Slot {
    url: String,
    w: i32,
    h: i32,
    state: State,
    used: u64,
}

#[derive(Default)]
struct Inner {
    by_req: HashMap<(String, i32, i32), u32>,
    slots: HashMap<u32, Slot>,
    next: u32,
    clock: u64,
    inflight: usize,
    /// Keys forgotten while their pixels were with the cache: freed on the next prepare.
    freed: Vec<PosterKey>,
}

/// What a worker posts back.
pub struct Fetched {
    key: u32,
    result: Result<Decoded, PosterError>,
}

/// Starts one fetch. The product spawns a worker; a test runs it inline or not at all.
pub trait Starter {
    fn start(&self, url: String, w: i32, h: i32, key: u32, tx: Sender<Fetched>) -> bool;
}

/// One small-stack worker per fetch.
pub struct Workers;

impl Starter for Workers {
    fn start(&self, url: String, w: i32, h: i32, key: u32, tx: Sender<Fetched>) -> bool {
        sn_base::task::spawn_off_frame("art", move |off| {
            let result = fetch_and_decode(off, &url, w, h);
            let _ = tx.send(Fetched { key, result });
            sn_machine::present::wake_from_worker();
        })
    }
}

pub struct Art {
    inner: RefCell<Inner>,
    tx: Sender<Fetched>,
    rx: Receiver<Fetched>,
    starter: Box<dyn Starter>,
}

impl Art {
    pub fn new(starter: Box<dyn Starter>) -> Self {
        let (tx, rx) = channel();
        Art { inner: RefCell::new(Inner { next: 1, ..Inner::default() }), tx, rx, starter }
    }

    /// Install a leaked source as the render cache's (once, at boot, on the frame thread).
    pub fn install(self) -> &'static Art {
        let art: &'static Art = Box::leak(Box::new(self));
        tex::install(art);
        art
    }

    /// Fold every fetch that came back into the cache, then start what waits for a slot. Frame
    /// thread, once a frame, before the present decision (so a waiting upload forces a present).
    pub fn drain(&self) {
        for done in self.rx.try_iter().collect::<Vec<_>>() {
            let ok = done.result.is_ok();
            {
                let mut inner = self.inner.borrow_mut();
                inner.inflight = inner.inflight.saturating_sub(1);
                let Some(slot) = inner.slots.get_mut(&done.key) else {
                    continue; // forgotten while it was out: the pixels are dropped
                };
                slot.state = if ok { State::Ready } else { State::Cooling(Instant::now() + Duration::from_millis(RETRY_MS)) };
            }
            tex::accept(PosterReady { key: PosterKey(done.key), result: done.result });
        }
        self.pump();
    }

    /// The keys forgotten while resident, for the frame thread to free (`tex::free`).
    pub fn take_freed(&self) -> Vec<PosterKey> {
        std::mem::take(&mut self.inner.borrow_mut().freed)
    }

    /// Start waiting fetches, most recently drawn first, while slots are free.
    fn pump(&self) {
        loop {
            let (key, url, w, h) = {
                let mut inner = self.inner.borrow_mut();
                if inner.inflight >= INFLIGHT_MAX {
                    return;
                }
                let Some((&key, _)) = inner.slots.iter().filter(|(_, s)| s.state == State::Wanted).max_by_key(|(_, s)| s.used) else {
                    return;
                };
                let slot = inner.slots.get_mut(&key).expect("just found");
                slot.state = State::Fetching;
                let (url, w, h) = (slot.url.clone(), slot.w, slot.h);
                inner.inflight += 1;
                (key, url, w, h)
            };
            if !self.starter.start(url, w, h, key, self.tx.clone()) {
                let mut inner = self.inner.borrow_mut();
                inner.inflight = inner.inflight.saturating_sub(1);
                if let Some(slot) = inner.slots.get_mut(&key) {
                    slot.state = State::Cooling(Instant::now() + Duration::from_millis(RETRY_MS));
                }
                return;
            }
        }
    }

    /// The key for a request, interning it (and asking for it) on a miss. `None`: not yet.
    fn ask(&self, path: &str, w: i32, h: i32) -> (u32, bool) {
        let mut inner = self.inner.borrow_mut();
        inner.clock += 1;
        let now = inner.clock;
        let req = (path.to_string(), w, h);
        if let Some(&key) = inner.by_req.get(&req) {
            let slot = inner.slots.get_mut(&key).expect("interned keys have slots");
            slot.used = now;
            if let State::Cooling(until) = slot.state {
                if Instant::now() >= until {
                    slot.state = State::Wanted;
                }
            }
            return (key, false);
        }
        if inner.slots.len() >= SLOTS_MAX {
            evict_one(&mut inner);
        }
        let key = inner.next;
        inner.next = inner.next.wrapping_add(1).max(1);
        inner.by_req.insert(req, key);
        inner.slots.insert(key, Slot { url: path.to_string(), w, h, state: State::Wanted, used: now });
        (key, true)
    }

    fn state(&self, key: u32) -> Option<State> {
        self.inner.borrow().slots.get(&key).map(|s| s.state)
    }
}

/// Forget the least recently drawn slot that is not out on a worker.
fn evict_one(inner: &mut Inner) {
    let Some((&key, _)) = inner.slots.iter().filter(|(_, s)| s.state != State::Fetching).min_by_key(|(_, s)| s.used) else {
        return;
    };
    if let Some(slot) = inner.slots.remove(&key) {
        inner.by_req.remove(&(slot.url, slot.w, slot.h));
        if slot.state == State::Ready {
            inner.freed.push(PosterKey(key));
        }
    }
}

impl tex::Source for Art {
    fn probe(&self, _srv: u16, path: &str, w: i32, h: i32, _png: bool) -> Option<PosterKey> {
        if path.is_empty() {
            return None;
        }
        let (key, _) = self.ask(path, w, h);
        self.pump();
        (self.state(key) == Some(State::Ready)).then_some(PosterKey(key))
    }

    fn warm(&self, _srv: u16, path: &str, w: i32, h: i32, _png: bool) -> Warm {
        if path.is_empty() {
            return Warm::Known;
        }
        let (_, new) = self.ask(path, w, h);
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
            .is_some_and(|s| matches!(s.state, State::Cooling(_)))
    }

    fn unresident(&self, key: PosterKey, refused: bool) {
        let mut inner = self.inner.borrow_mut();
        if let Some(slot) = inner.slots.get_mut(&key.0) {
            // Stop answering READY now. A refused arrival cools down; a lost texture may be asked
            // for again by the next draw that wants it.
            slot.state = if refused { State::Cooling(Instant::now() + Duration::from_millis(RETRY_MS)) } else { State::Wanted };
        }
    }

    fn idle(&self) -> bool {
        let inner = self.inner.borrow();
        inner.inflight == 0 && !inner.slots.values().any(|s| s.state == State::Wanted)
    }
}

/// Fetch, decode and cover-scale one image, on a worker.
pub fn fetch_and_decode(off: &OffFrame, url: &str, w: i32, h: i32) -> Result<Decoded, PosterError> {
    let body = NetFetch::new(off, Profile::Art).get(url, &[]).map_err(|_| PosterError::Fetch)?;
    decode_cover(&body, w, h)
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

/// A `Fetch`-driven fetch for tests that exercise the decode path without a network.
pub fn fetch_with(fetch: &dyn Fetch, url: &str, w: i32, h: i32) -> Result<Decoded, PosterError> {
    let body = fetch.get(url, &[]).map_err(|_| PosterError::Fetch)?;
    decode_cover(&body, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_ui::tex::Source;
    use std::sync::{Arc, Mutex};

    /// Records the fetches started; `ok` decides whether they start at all.
    struct Recorder {
        started: Arc<Mutex<Vec<(String, u32)>>>,
        ok: bool,
    }
    impl Starter for Recorder {
        fn start(&self, url: String, _w: i32, _h: i32, key: u32, _tx: Sender<Fetched>) -> bool {
            if self.ok {
                self.started.lock().unwrap().push((url, key));
            }
            self.ok
        }
    }

    fn art(ok: bool) -> (Art, Arc<Mutex<Vec<(String, u32)>>>) {
        let started = Arc::new(Mutex::new(Vec::new()));
        (Art::new(Box::new(Recorder { started: started.clone(), ok })), started)
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_draw_asks_once_and_is_answered_only_once_the_pixels_arrive() {
        let _g = sn_base::testlock::serial();
        tex::reset_for_test(64 << 20);
        let (art, started) = art(true);
        assert_eq!(art.probe(0, "https://img.example/a.jpg", 250, 375, false), None, "a miss starts the fetch");
        assert_eq!(art.probe(0, "https://img.example/a.jpg", 250, 375, false), None, "asking again starts nothing more");
        assert_eq!(started.lock().unwrap().len(), 1);
        let key = started.lock().unwrap()[0].1;
        art.tx.send(Fetched { key, result: decode_cover(&png(500, 750), 250, 375) }).unwrap();
        art.drain();
        assert_eq!(art.probe(0, "https://img.example/a.jpg", 250, 375, false), Some(PosterKey(key)));
        assert!(art.idle());
        tex::reset_for_test(64 << 20);
    }

    #[test]
    fn fetches_are_bounded_and_the_newest_request_goes_first() {
        let _g = sn_base::testlock::serial();
        let (art, started) = art(true);
        for i in 0..INFLIGHT_MAX + 3 {
            art.probe(0, &format!("https://img.example/{i}.jpg"), 250, 375, false);
        }
        assert_eq!(started.lock().unwrap().len(), INFLIGHT_MAX, "never more than the bound out at once");
        assert!(!art.idle(), "three still wait");
        // one comes back: the most recently drawn of the waiting three starts next
        let first = started.lock().unwrap()[0].1;
        art.tx.send(Fetched { key: first, result: Err(PosterError::Fetch) }).unwrap();
        art.drain();
        let last = started.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.0, format!("https://img.example/{}.jpg", INFLIGHT_MAX + 2));
    }

    #[test]
    fn a_failure_settles_for_a_while_instead_of_refetching_every_frame() {
        let _g = sn_base::testlock::serial();
        let (art, started) = art(true);
        art.probe(0, "https://img.example/broken.jpg", 250, 375, false);
        let key = started.lock().unwrap()[0].1;
        art.tx.send(Fetched { key, result: Err(PosterError::Decode) }).unwrap();
        art.drain();
        for _ in 0..10 {
            assert_eq!(art.probe(0, "https://img.example/broken.jpg", 250, 375, false), None);
        }
        assert_eq!(started.lock().unwrap().len(), 1, "settled: no refetch inside the retry window");
        assert!(art.failed(0, "https://img.example/broken.jpg", 250, 375, false));
    }

    #[test]
    fn a_worker_refused_by_the_os_cools_down_instead_of_spinning() {
        let _g = sn_base::testlock::serial();
        let (art, _) = art(false);
        art.probe(0, "https://img.example/a.jpg", 250, 375, false);
        assert!(art.failed(0, "https://img.example/a.jpg", 250, 375, false));
        assert!(art.idle());
    }

    #[test]
    fn the_least_recently_drawn_key_is_forgotten_and_its_texture_freed() {
        let _g = sn_base::testlock::serial();
        tex::reset_for_test(64 << 20);
        let (art, started) = art(true);
        art.probe(0, "https://img.example/old.jpg", 250, 375, false);
        let old = started.lock().unwrap()[0].1;
        art.tx.send(Fetched { key: old, result: decode_cover(&png(10, 15), 250, 375) }).unwrap();
        art.drain();
        started.lock().unwrap().clear();
        for i in 0..SLOTS_MAX {
            art.probe(0, &format!("https://img.example/{i}.jpg"), 250, 375, false);
            // keep the fetch bound from mattering: everything that starts comes straight back
            let out: Vec<u32> = started.lock().unwrap().drain(..).map(|(_, k)| k).collect();
            for k in out {
                art.tx.send(Fetched { key: k, result: Err(PosterError::Fetch) }).unwrap();
            }
            art.drain();
        }
        assert_eq!(art.take_freed(), vec![PosterKey(old)], "the oldest READY key leaves, and its texture with it");
        assert!(art.inner.borrow().slots.len() <= SLOTS_MAX);
        tex::reset_for_test(64 << 20);
    }

    #[test]
    fn a_large_image_is_scaled_down_to_cover_the_box_and_a_small_one_is_left_alone() {
        assert_eq!(cover_size(1000, 1500, 250, 375), (250, 375));
        assert_eq!(cover_size(1920, 1080, 250, 375), (667, 375), "a wide image keeps its height and overhangs");
        assert_eq!(cover_size(200, 300, 250, 375), (200, 300), "never scaled up");
        let d = decode_cover(&png(500, 750), 250, 375).unwrap();
        assert_eq!((d.w, d.h, d.rgba.len()), (250, 375, 250 * 375 * 4));
        assert!(matches!(decode_cover(b"not an image", 250, 375), Err(PosterError::Decode)));
    }

    #[test]
    fn a_fetch_failure_is_a_fetch_error() {
        struct Down;
        impl Fetch for Down {
            fn get(&self, _: &str, _: &[(String, String)]) -> Result<Vec<u8>, sn_stremio::FetchError> {
                Err(sn_stremio::FetchError::Status(404))
            }
            fn post(&self, _: &str, _: &[(String, String)], _: &[u8]) -> Result<Vec<u8>, sn_stremio::FetchError> {
                unreachable!()
            }
        }
        assert!(matches!(fetch_with(&Down, "https://x/y.jpg", 250, 375), Err(PosterError::Fetch)));
    }
}
