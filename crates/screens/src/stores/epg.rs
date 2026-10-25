//! **The TV guide store** — what Live TV's channels show now and next, from the XMLTV guide
//! Settings → TV guide names (`settings::tv_guide_url`).
//!
//! The store plans one load at a time ([`EpgJob`]) and holds the parsed guide that lands
//! ([`EpgLanding`]); the application fetches, caches and parses it off the frame thread. It asks
//! again every [`RELOAD_S`] (the application answers from its day-old cache until the guide is
//! due a new download), after a failure not before [`RETRY_S`], and at once when the region
//! changes. The wall clock arrives through [`EpgStore::tick`]: its generation moves when the
//! minute does, so a card's "Now:" follows the programme changing without anything landing.

use sn_stremio::epg::{Guide, NowNext};

/// How often a held guide is asked for again (the parse window runs well past it).
pub const RELOAD_S: i64 = 6 * 3600;
/// How long after a failed load the next is planned.
pub const RETRY_S: i64 = 15 * 60;

/// Load the guide at `url`, reading the time as `now_s` (Unix seconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpgJob {
    pub url: &'static str,
    pub now_s: i64,
}

/// What a load brought: the guide, or why there is none.
#[derive(Debug)]
pub struct EpgLanding {
    pub url: &'static str,
    pub result: Result<Guide, String>,
}

#[derive(Debug, Default)]
pub struct EpgStore {
    /// The guide the settings name (`None`: off).
    source: Option<&'static str>,
    /// The guide held, and the URL it came from.
    guide: Option<(&'static str, Guide)>,
    inflight: bool,
    /// No load is planned before this: [`RELOAD_S`] after a guide landed, [`RETRY_S`] after a
    /// failure, 0 (at once) for a region not loaded yet.
    due_at: i64,
    now_s: i64,
    gen: u32,
}

impl EpgStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    fn bump(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    /// The frame's turn: the guide `source` the settings name now and the wall clock. Returns the
    /// load to start, if one is due.
    pub fn tick(&mut self, source: Option<&'static str>, now_s: i64) -> Option<EpgJob> {
        if source != self.source {
            self.source = source;
            self.due_at = 0;
            if self.guide.as_ref().is_some_and(|g| Some(g.0) != source) {
                self.guide = None;
                self.bump();
            }
        }
        if self.guide.is_some() && now_s / 60 != self.now_s / 60 {
            self.bump();
        }
        self.now_s = now_s;
        let url = self.source?;
        if self.inflight || now_s < self.due_at {
            return None;
        }
        self.inflight = true;
        self.due_at = now_s + RELOAD_S; // counted from the ask; a failure shortens it
        Some(EpgJob { url, now_s })
    }

    /// Fold a load in. `true` when what the pages read changed.
    pub fn land(&mut self, l: EpgLanding) -> bool {
        self.inflight = false;
        if Some(l.url) != self.source {
            return false; // the region changed while it loaded
        }
        match l.result {
            Ok(guide) => {
                self.guide = Some((l.url, guide));
                self.bump();
                true
            }
            Err(_) => {
                // a held guide stays until it is replaced; the next try waits
                self.due_at = self.now_s + RETRY_S;
                false
            }
        }
    }

    /// What `channel` (an addon's channel name) shows now. `None` with no guide, or when the guide
    /// does not know the channel.
    pub fn now_next(&self, channel: &str) -> Option<NowNext<'_>> {
        self.guide.as_ref()?.1.now_next(channel, self.now_s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://example.test/guide.xml.gz";
    const OTHER: &str = "https://example.test/other.xml.gz";
    const T0: i64 = 1_792_929_600; // 2026-10-25 12:00 UTC

    fn guide() -> Guide {
        let doc = r#"<tv><channel id="ZDF.de"><display-name>ZDF</display-name></channel>
            <programme start="20261025120000 +0000" stop="20261025130000 +0000" channel="ZDF.de"><title>heute</title></programme>
            <programme start="20261025130000 +0000" stop="20261025140000 +0000" channel="ZDF.de"><title>Sport</title></programme>
            <programme start="20261025120000 +0000" stop="20261025235900 +0000" channel="ARD.de"><title>Long day</title></programme></tv>"#;
        sn_stremio::epg::parse(doc.as_bytes(), T0 - 3600, T0 + 86_400).unwrap()
    }

    #[test]
    fn off_plans_nothing_and_a_region_loads_once_until_it_is_due_again() {
        let mut s = EpgStore::new();
        assert_eq!(s.tick(None, T0), None, "off");
        let job = s.tick(Some(URL), T0).expect("a region loads");
        assert_eq!(job, EpgJob { url: URL, now_s: T0 });
        assert_eq!(s.tick(Some(URL), T0 + 5), None, "one load at a time");
        assert!(s.land(EpgLanding { url: URL, result: Ok(guide()) }));
        assert_eq!(s.tick(Some(URL), T0 + 60), None, "held, not due");
        assert!(s.tick(Some(URL), T0 + RELOAD_S).is_some(), "due again");
    }

    #[test]
    fn now_and_next_follow_the_clock_and_the_minute_moves_the_generation() {
        let mut s = EpgStore::new();
        s.tick(Some(URL), T0 + 30 * 60);
        s.land(EpgLanding { url: URL, result: Ok(guide()) });
        let nn = s.now_next("ZDF HD").unwrap();
        assert_eq!((nn.now.unwrap().title.as_str(), nn.next.unwrap().title.as_str()), ("heute", "Sport"));
        let g = s.gen();
        s.tick(Some(URL), T0 + 30 * 60 + 10);
        assert_eq!(s.gen(), g, "same minute: nothing to redraw");
        s.tick(Some(URL), T0 + 61 * 60);
        assert!(s.gen() != g, "a new minute redraws");
        assert_eq!(s.now_next("zdf").unwrap().now.unwrap().title, "Sport");
        assert!(s.now_next("Unknown").is_none());
    }

    #[test]
    fn a_failure_waits_before_trying_again_and_keeps_the_guide_it_had() {
        let mut s = EpgStore::new();
        s.tick(Some(URL), T0);
        s.land(EpgLanding { url: URL, result: Ok(guide()) });
        assert!(s.tick(Some(URL), T0 + RELOAD_S).is_some());
        assert!(!s.land(EpgLanding { url: URL, result: Err("timed out".into()) }));
        assert!(s.now_next("ARD").is_some(), "the held guide stays");
        assert_eq!(s.tick(Some(URL), T0 + RELOAD_S + 60), None, "waits");
        assert!(s.tick(Some(URL), T0 + RELOAD_S + RETRY_S).is_some());
    }

    #[test]
    fn changing_or_turning_off_the_region_drops_the_guide_and_a_stale_landing() {
        let mut s = EpgStore::new();
        s.tick(Some(URL), T0);
        s.land(EpgLanding { url: URL, result: Ok(guide()) });
        let job = s.tick(Some(OTHER), T0 + 60).expect("the new region loads at once");
        assert_eq!(job.url, OTHER);
        assert!(s.now_next("ZDF").is_none(), "the old region's guide is gone");
        assert!(!s.land(EpgLanding { url: URL, result: Ok(guide()) }), "a landing for the old region is dropped");
        s.tick(None, T0 + 120);
        assert!(!s.land(EpgLanding { url: OTHER, result: Ok(guide()) }), "turned off while it loaded");
        assert!(s.now_next("ZDF").is_none());
    }
}
