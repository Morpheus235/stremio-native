//! **Loading the TV guide** (`sn_screens::stores::epg`'s jobs), off the frame thread.
//!
//! The guide is downloaded at most once a day per region and parsed into the window the store
//! reads (an hour back to two days ahead). What is kept on disk is that parsed window, not the
//! document: a national guide is 3–6 MB gzipped and ~20 MB of XML, its titles and times well
//! under a megabyte, and reading them back costs nothing at boot. A failed download falls back to
//! the cached window, however old, as long as it is the same region's.

use serde::{Deserialize, Serialize};
use sn_screens::stores::epg::EpgJob;
use sn_stremio::epg::Guide;
use sn_stremio::Fetch;

use crate::account::Files;

/// The cached window's file in the private data directory.
pub const GUIDE_FILE: &str = "tv-guide.json";
/// A cached guide younger than this is used without asking the network.
pub const FRESH_S: i64 = 24 * 3600;
/// How far back and ahead of the load a parse keeps programmes. The ahead part outlasts
/// [`FRESH_S`] by a day, so a guide used until it is due again still covers the next day.
const BEHIND_S: i64 = 3600;
const AHEAD_S: i64 = 48 * 3600;

#[derive(Serialize, Deserialize)]
struct Cached {
    url: String,
    fetched_at: i64,
    guide: Guide,
}

/// The guide `job` asks for: the cache while it is fresh, else the network (and the cache
/// rewritten), else a stale cache of the same region. The error never names the URL.
pub fn load(fetch: &dyn Fetch, files: &dyn Files, job: &EpgJob) -> Result<Guide, String> {
    let cached = files.read(GUIDE_FILE).and_then(|b| serde_json::from_slice::<Cached>(&b).ok()).filter(|c| c.url == job.url);
    if let Some(c) = cached.as_ref().filter(|c| (0..FRESH_S).contains(&(job.now_s - c.fetched_at))) {
        sn_base::eventlog::log(&format!("guide: cached ({} channels)", c.guide.channels()));
        return Ok(cached.expect("checked").guide);
    }
    let fresh = fetch
        .get(job.url, &[])
        .map_err(|e| sn_screens::stores::fetch_reason(&e))
        .and_then(|body| sn_stremio::epg::parse(&body, job.now_s - BEHIND_S, job.now_s + AHEAD_S));
    match fresh {
        Ok(guide) => {
            sn_base::eventlog::log(&format!("guide: loaded ({} channels)", guide.channels()));
            let c = Cached { url: job.url.to_string(), fetched_at: job.now_s, guide };
            if let Ok(bytes) = serde_json::to_vec(&c) {
                files.write(GUIDE_FILE, &bytes);
            }
            Ok(c.guide)
        }
        Err(why) => match cached {
            Some(c) => {
                sn_base::eventlog::log(&format!("guide: download failed ({why}); the cached one stays"));
                Ok(c.guide)
            }
            None => {
                sn_base::eventlog::log(&format!("guide: download failed ({why})"));
                Err(why)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_stremio::FetchError;
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::sync::Mutex;

    const URL: &str = "https://example.test/de.xml.gz";
    const T0: i64 = 1_792_929_600; // 2026-10-25 12:00 UTC
    const DOC: &str = r#"<tv><channel id="ZDF.de"><display-name>ZDF</display-name></channel>
        <programme start="20261025120000 +0000" stop="20261025130000 +0000" channel="ZDF.de"><title>heute</title></programme></tv>"#;

    struct Net {
        body: Result<&'static str, FetchError>,
        asked: Cell<u32>,
    }
    impl Fetch for Net {
        fn get(&self, _: &str, _: &[(String, String)]) -> Result<Vec<u8>, FetchError> {
            self.asked.set(self.asked.get() + 1);
            self.body.clone().map(|b| b.as_bytes().to_vec())
        }
        fn post(&self, _: &str, _: &[(String, String)], _: &[u8]) -> Result<Vec<u8>, FetchError> {
            unreachable!()
        }
    }
    #[derive(Default)]
    struct Mem(Mutex<HashMap<String, Vec<u8>>>);
    impl Files for Mem {
        fn read(&self, name: &str) -> Option<Vec<u8>> {
            self.0.lock().unwrap().get(name).cloned()
        }
        fn write(&self, name: &str, bytes: &[u8]) -> bool {
            self.0.lock().unwrap().insert(name.into(), bytes.to_vec());
            true
        }
        fn remove(&self, name: &str) {
            self.0.lock().unwrap().remove(name);
        }
    }
    fn ok() -> Net {
        Net { body: Ok(DOC), asked: Cell::new(0) }
    }
    fn down() -> Net {
        Net { body: Err(FetchError::Transport("timed out".into())), asked: Cell::new(0) }
    }
    fn job(url: &'static str, now_s: i64) -> EpgJob {
        EpgJob { url, now_s }
    }
    fn on_air(g: &Guide, at: i64) -> Option<String> {
        g.now_next("ZDF", at)?.now.map(|p| p.title.clone())
    }

    #[test]
    fn a_download_is_kept_and_used_for_a_day_then_asked_again() {
        let files = Mem::default();
        let net = ok();
        assert_eq!(on_air(&load(&net, &files, &job(URL, T0)).unwrap(), T0 + 60).as_deref(), Some("heute"));
        assert_eq!(net.asked.get(), 1);
        load(&net, &files, &job(URL, T0 + FRESH_S - 1)).unwrap();
        assert_eq!(net.asked.get(), 1, "fresh: the cache answers");
        load(&net, &files, &job(URL, T0 + FRESH_S)).unwrap();
        assert_eq!(net.asked.get(), 2, "a day old: the network");
    }

    #[test]
    fn a_failed_download_falls_back_to_the_same_regions_cache_only() {
        let files = Mem::default();
        load(&ok(), &files, &job(URL, T0)).unwrap();
        let g = load(&down(), &files, &job(URL, T0 + 3 * FRESH_S)).expect("stale beats nothing");
        assert_eq!(on_air(&g, T0 + 60).as_deref(), Some("heute"));
        assert!(load(&down(), &files, &job("https://example.test/uk.xml.gz", T0)).is_err(), "another region's cache is not this one");
        let err = load(&down(), &Mem::default(), &job(URL, T0)).unwrap_err();
        assert!(!err.contains("example.test"), "the reason never names the URL: {err}");
    }

    #[test]
    fn a_body_that_is_not_a_guide_fails_and_is_not_cached() {
        let files = Mem::default();
        let net = Net { body: Ok("<html>Not found</html>"), asked: Cell::new(0) };
        assert!(load(&net, &files, &job(URL, T0)).is_err());
        assert!(files.read(GUIDE_FILE).is_none());
    }
}
