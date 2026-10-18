//! Viewport-first loading.
//!
//! A page of catalog rows (or a grid of posters) asks for many loads at once, and the reader
//! moves faster than they complete. A first-in-first-out queue drains from the TOP: jump ten rows
//! down and the rows on screen wait behind every row scrolled past. This queue instead:
//!
//! * starts the request NEAREST the viewport first (on-screen, then outward by distance);
//! * when the viewport moves, CANCELS in-flight work that left the keep window (through its
//!   [`Cancel`] token, which the transport polls) and drops queued work outside it;
//! * forgets what it dropped, so a row that scrolls back into view is simply asked for again.
//!
//! Positions are abstract indices (row number, tile number); the caller maps its layout to them.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cancellation flag shared with the request doing the work.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub struct Scheduler<K> {
    /// Concurrent requests allowed.
    pub slots: usize,
    /// Items beyond the viewport, on each side, still worth loading.
    pub lookahead: usize,
    view: Range<usize>,
    queued: HashMap<K, usize>,
    running: HashMap<K, (usize, Cancel)>,
}

impl<K: Clone + Eq + Hash> Scheduler<K> {
    pub fn new(slots: usize, lookahead: usize) -> Self {
        Scheduler { slots: slots.max(1), lookahead, view: 0..0, queued: HashMap::new(), running: HashMap::new() }
    }

    fn window(&self) -> Range<usize> {
        self.view.start.saturating_sub(self.lookahead)..self.view.end.saturating_add(self.lookahead)
    }

    fn distance(&self, at: usize) -> usize {
        if at < self.view.start {
            self.view.start - at
        } else if at >= self.view.end {
            at + 1 - self.view.end
        } else {
            0
        }
    }

    /// Ask for `key`, which sits at position `at`. A key already queued or running is not asked
    /// twice; one outside the keep window is ignored (it will be asked again when it nears view).
    pub fn want(&mut self, key: K, at: usize) {
        if self.running.contains_key(&key) || !self.window().contains(&at) {
            return;
        }
        self.queued.insert(key, at);
    }

    /// The viewport moved. Returns the keys whose in-flight requests were cancelled.
    pub fn set_view(&mut self, view: Range<usize>) -> Vec<K> {
        self.view = view;
        let window = self.window();
        self.queued.retain(|_, at| window.contains(at));
        let gone: Vec<K> = self.running.iter().filter(|(_, (at, _))| !window.contains(at)).map(|(k, _)| k.clone()).collect();
        for key in &gone {
            if let Some((_, cancel)) = self.running.remove(key) {
                cancel.cancel();
            }
        }
        gone
    }

    /// The caller renumbered its positions (a filter changed which items it lays out): every
    /// queued and running key moves to its new position, and one with none any more is dropped (a
    /// running one cancelled). Returns the keys cancelled. Nothing in flight that is still laid
    /// out is asked again.
    pub fn remap(&mut self, at: impl Fn(&K) -> Option<usize>) -> Vec<K> {
        self.queued = std::mem::take(&mut self.queued).into_keys().filter_map(|k| Some((k.clone(), at(&k)?))).collect();
        let mut gone = Vec::new();
        for (k, (_, cancel)) in std::mem::take(&mut self.running) {
            match at(&k) {
                Some(n) => {
                    self.running.insert(k, (n, cancel));
                }
                None => {
                    cancel.cancel();
                    gone.push(k);
                }
            }
        }
        gone
    }

    /// The next request to start, nearest the viewport first, if a slot is free.
    pub fn next(&mut self) -> Option<(K, Cancel)> {
        if self.running.len() >= self.slots {
            return None;
        }
        let key = self
            .queued
            .iter()
            .min_by_key(|(_, at)| (self.distance(**at), **at))
            .map(|(k, _)| k.clone())?;
        let at = self.queued.remove(&key)?;
        let cancel = Cancel::default();
        self.running.insert(key.clone(), (at, cancel.clone()));
        Some((key, cancel))
    }

    /// A request finished (or failed). Returns false when it had already been cancelled, so the
    /// caller drops its result instead of landing it.
    pub fn done(&mut self, key: &K) -> bool {
        self.running.remove(key).is_some()
    }

    pub fn running(&self) -> usize {
        self.running.len()
    }
    pub fn queued(&self) -> usize {
        self.queued.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(s: &mut Scheduler<usize>) -> Vec<usize> {
        let mut order = Vec::new();
        while let Some((k, _)) = s.next() {
            order.push(k);
            s.done(&k);
        }
        order
    }

    #[test]
    fn on_screen_rows_load_first_then_outward() {
        let mut s = Scheduler::new(1, 3);
        s.set_view(10..13);
        for row in 0..30 {
            s.want(row, row);
        }
        assert_eq!(drain(&mut s), [10, 11, 12, 9, 13, 8, 14, 7, 15]);
    }

    #[test]
    fn jumping_far_ahead_cancels_and_drops_the_rows_left_behind() {
        let mut s = Scheduler::new(2, 2);
        s.set_view(0..4);
        for row in 0..6 {
            s.want(row, row);
        }
        let (a, ca) = s.next().unwrap();
        let (b, cb) = s.next().unwrap();
        assert_eq!((a, b), (0, 1));
        assert!(s.next().is_none(), "both slots busy");

        let mut cancelled = s.set_view(40..44);
        cancelled.sort();
        assert_eq!(cancelled, [0, 1]);
        assert!(ca.is_cancelled() && cb.is_cancelled());
        assert_eq!(s.queued(), 0, "queued top rows are dropped, not drained");
        assert!(!s.done(&0), "a cancelled result is not landed");

        for row in 38..46 {
            s.want(row, row);
        }
        assert_eq!(drain(&mut s), [40, 41, 42, 43, 39, 44, 38, 45]);
    }

    /// A filter renumbers the rows: what stays laid out keeps running at its new position, what
    /// left the layout is cancelled, and nothing is asked twice.
    #[test]
    fn a_remap_moves_what_stays_and_cancels_what_left() {
        let mut s = Scheduler::new(2, 0);
        s.set_view(0..4);
        for k in 0..4 {
            s.want(k, k);
        }
        let (zero, c0) = s.next().unwrap();
        let (one, c1) = s.next().unwrap();
        assert_eq!((zero, one), (0, 1), "nearest first, then by position");
        // keep the odd keys, packed: 1 -> 0, 3 -> 1
        assert_eq!(s.remap(|k| (k % 2 == 1).then_some(k / 2)), vec![0]);
        assert!(c0.is_cancelled() && !c1.is_cancelled());
        assert_eq!((s.running(), s.queued()), (1, 1), "2 dropped, 3 still queued");
        s.set_view(0..1);
        assert!(s.done(&1), "1 is at 0 now, inside the view: its landing is kept");
        assert_eq!(s.queued(), 0, "3 is at 1 now, outside the view");
    }

    #[test]
    fn a_row_scrolled_back_into_view_is_asked_again() {
        let mut s = Scheduler::new(4, 0);
        s.set_view(0..2);
        s.want(0, 0);
        s.set_view(10..12);
        s.want(0, 0);
        assert_eq!(s.queued(), 0, "outside the window it is ignored");
        s.set_view(0..2);
        s.want(0, 0);
        assert_eq!(drain(&mut s), [0]);
    }

    #[test]
    fn a_running_key_is_not_started_twice() {
        let mut s = Scheduler::new(4, 4);
        s.set_view(0..4);
        s.want(1, 1);
        let _ = s.next().unwrap();
        s.want(1, 1);
        assert!(s.next().is_none());
    }
}
