//! **The streaming server's off-frame jobs** (`sn_screens::stores::server`'s `ServerJob`s): ask
//! one server whether it is alive, and scan the home network for servers.
//!
//! A scan probes `GET {base}settings` over [`Profile::LanScan`] and a check over [`Profile::Lan`] or
//! [`Profile::Remote`] (`work.rs` picks by address): no redirects, a small body, and short
//! deadlines for the home network. The answer is read with
//! `sn_stremio::streaming_server::parse_settings`. A scan asks
//! `127.0.0.1` and the other 253 hosts of the /24 of each of the television's private IPv4
//! addresses, [`PROBES_AT_ONCE`] at a time, on small workers of its own (the job thread is one of
//! them). The log says how many hosts were asked, how many answered and how long it took, never
//! an address: the viewer's server and network are theirs.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use sn_base::eventlog::log;
use sn_base::task::OffFrame;
use sn_screens::stores::server::Found;
use sn_stremio::streaming_server::{parse_settings, scan_targets, settings_url, ServerInfo, DEFAULT_PORT};
use sn_stremio::Fetch;

use crate::fetch::{NetFetch, Profile};

/// How many hosts a scan asks at once.
pub const PROBES_AT_ONCE: usize = 32;

/// Is `base` a live streaming server? `None` for anything else: no answer, a status that is not
/// 2xx, a body that is not a server's settings.
pub fn run_check(fetch: &dyn Fetch, base: &str) -> Option<ServerInfo> {
    let began = std::time::Instant::now();
    let info = probe(fetch, base);
    log(&format!(
        "streaming server: {} in {} ms",
        if info.is_some() { "answered" } else { "did not answer" },
        began.elapsed().as_millis()
    ));
    info
}

fn probe(fetch: &dyn Fetch, base: &str) -> Option<ServerInfo> {
    fetch.get(&settings_url(base), &[]).ok().and_then(|body| parse_settings(&body))
}

/// The base URL a scan probes on `ip`.
fn base_of(ip: Ipv4Addr) -> String {
    format!("http://{ip}:{DEFAULT_PORT}/")
}

/// The hosts a scan asks: this television (`127.0.0.1`), then the other hosts of each `own`
/// address's /24, once each. Only private addresses (10/8, 172.16/12, 192.168/16) start a scan:
/// the television is never made to sweep a public network.
pub fn scan_hosts(own: &[Ipv4Addr]) -> Vec<Ipv4Addr> {
    let mut hosts = vec![Ipv4Addr::LOCALHOST];
    for ip in own.iter().filter(|ip| ip.is_private()) {
        for host in scan_targets(*ip) {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
    }
    hosts
}

/// **A scan of the networks the television is on**, over the real transport.
pub fn run_scan(off: &OffFrame, own: &[Ipv4Addr]) -> Vec<Found> {
    let began = std::time::Instant::now();
    let hosts = scan_hosts(own);
    let asked = hosts.len();
    let found = scan_with(off, hosts, PROBES_AT_ONCE, |off, ip| probe(&NetFetch::new(off, Profile::LanScan), &base_of(ip)));
    log(&format!("streaming server: scan of {asked} hosts found {} in {} ms", found.len(), began.elapsed().as_millis()));
    found
}

/// Ask every host of `hosts` with `ask`, at most `limit` at a time, and return those that
/// answered as a server, by address. The calling thread is one of the askers; the rest are small
/// workers, and one the OS refuses just leaves the others with more to do.
pub fn scan_with(
    off: &OffFrame,
    hosts: Vec<Ipv4Addr>,
    limit: usize,
    ask: impl Fn(&OffFrame, Ipv4Addr) -> Option<ServerInfo> + Send + Sync + 'static,
) -> Vec<Found> {
    let queue = Arc::new(Mutex::new(VecDeque::from(hosts)));
    let alive = Arc::new(Mutex::new(Vec::<(Ipv4Addr, ServerInfo)>::new()));
    let ask = Arc::new(ask);
    let ask_all = {
        let (queue, alive, ask) = (queue.clone(), alive.clone(), ask.clone());
        move |off: &OffFrame| loop {
            let next = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
            let Some(ip) = next else { break };
            if let Some(info) = ask(off, ip) {
                alive.lock().unwrap_or_else(|e| e.into_inner()).push((ip, info));
            }
        }
    };
    // a worker's sender is dropped when it ends, which is what the wait below counts
    let (done, ended) = channel::<()>();
    let workers = limit.saturating_sub(1).min(queue.lock().unwrap_or_else(|e| e.into_inner()).len());
    for _ in 0..workers {
        let (ask_all, done) = (ask_all.clone(), done.clone());
        sn_base::task::spawn_off_frame("lan-probe", move |off| {
            ask_all(off);
            drop(done);
        });
    }
    drop(done);
    ask_all(off);
    while ended.recv().is_ok() {}
    let mut alive = std::mem::take(&mut *alive.lock().unwrap_or_else(|e| e.into_inner()));
    alive.sort_by_key(|(ip, _)| *ip);
    alive.into_iter().map(|(ip, info)| Found { base: base_of(ip), version: info.version }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_stremio::FetchError;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    /// Answers from a URL → body map, as a server or a router would.
    struct Net(Vec<(String, Result<&'static str, FetchError>)>);
    impl Fetch for Net {
        fn get(&self, url: &str, headers: &[(String, String)]) -> Result<Vec<u8>, FetchError> {
            assert!(headers.is_empty(), "a probe sends nothing of its own");
            match self.0.iter().find(|(u, _)| u == url) {
                Some((_, Ok(body))) => Ok(body.as_bytes().to_vec()),
                Some((_, Err(e))) => Err(e.clone()),
                None => Err(FetchError::Status(404)),
            }
        }
        fn post(&self, url: &str, headers: &[(String, String)], _body: &[u8]) -> Result<Vec<u8>, FetchError> {
            self.get(url, headers)
        }
    }

    const BASE: &str = "http://192.168.1.20:11470/";
    const URL: &str = "http://192.168.1.20:11470/settings";
    const SETTINGS: &str = r#"{"values":{"serverVersion":"4.20.8"}}"#;

    #[test]
    fn a_check_reads_the_settings_of_the_server_asked() {
        let alive = Net(vec![(URL.into(), Ok(SETTINGS))]);
        assert_eq!(run_check(&alive, BASE), Some(ServerInfo { version: Some("4.20.8".into()) }));
        let router = Net(vec![(URL.into(), Ok("<html>router</html>"))]);
        assert_eq!(run_check(&router, BASE), None, "an answer that is not a server's settings");
        let other_json = Net(vec![(URL.into(), Ok(r#"{"ok":true}"#))]);
        assert_eq!(run_check(&other_json, BASE), None);
        let down = Net(vec![(URL.into(), Err(FetchError::Transport("timed out".into())))]);
        assert_eq!(run_check(&down, BASE), None);
        assert_eq!(run_check(&Net(vec![]), BASE), None, "a 404 is not alive");
    }

    #[test]
    fn a_scan_asks_this_television_and_the_subnet_of_each_private_address_once() {
        let own = [Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(192, 168, 1, 21), Ipv4Addr::new(10, 0, 5, 2), Ipv4Addr::new(203, 0, 113, 9), Ipv4Addr::new(169, 254, 3, 3)];
        let hosts = scan_hosts(&own);
        assert_eq!(hosts[0], Ipv4Addr::LOCALHOST, "this television first");
        // two interfaces in one /24 ask it once; a public or link-local address starts no scan
        let mut sorted = hosts.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), hosts.len(), "no host twice");
        assert_eq!(hosts.len(), 1 + 254 + 253, "127.0.0.1, 192.168.1.0/24 (both own addresses are asked by the other), 10.0.5.0/24 less its own");
        assert!(hosts.contains(&Ipv4Addr::new(192, 168, 1, 1)) && hosts.contains(&Ipv4Addr::new(10, 0, 5, 254)));
        assert!(!hosts.iter().any(|h| h.octets()[0] == 203 || h.octets()[..2] == [169, 254]));
        assert_eq!(scan_hosts(&[]), vec![Ipv4Addr::LOCALHOST], "no network: only itself");
    }

    #[test]
    fn a_scan_lists_the_servers_that_answered_by_address() {
        let alive = |ip: Ipv4Addr| match ip.octets()[3] {
            7 => Some(ServerInfo { version: Some("4.20.8".into()) }),
            200 => Some(ServerInfo { version: None }),
            30 => Some(ServerInfo { version: Some("4.19.0".into()) }),
            _ => None,
        };
        let hosts = scan_targets(Ipv4Addr::new(192, 168, 1, 20));
        let found = scan_with(&OffFrame::for_test(), hosts, 8, move |_, ip| alive(ip));
        assert_eq!(
            found,
            vec![
                Found { base: "http://192.168.1.7:11470/".into(), version: Some("4.20.8".into()) },
                Found { base: "http://192.168.1.30:11470/".into(), version: Some("4.19.0".into()) },
                Found { base: "http://192.168.1.200:11470/".into(), version: None },
            ]
        );
        let off = OffFrame::for_test();
        assert!(scan_with(&off, scan_targets(Ipv4Addr::new(10, 1, 1, 1)), 8, |_, _| None).is_empty(), "nothing answered: an empty list");
        assert!(scan_with(&off, vec![], 8, |_, _| None).is_empty());
    }

    /// No more than `limit` hosts are asked at once, and they are asked in parallel.
    #[test]
    fn a_scan_asks_at_most_its_limit_at_once() {
        let (now, most, asked) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (n, m, a) = (now.clone(), most.clone(), asked.clone());
        let hosts = scan_targets(Ipv4Addr::new(192, 168, 1, 20));
        let total = hosts.len();
        let found = scan_with(&OffFrame::for_test(), hosts, 6, move |_, _| {
            let at = n.fetch_add(1, SeqCst) + 1;
            m.fetch_max(at, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(2));
            n.fetch_sub(1, SeqCst);
            a.fetch_add(1, SeqCst);
            None
        });
        assert!(found.is_empty());
        assert_eq!(asked.load(SeqCst), total, "every host asked exactly once");
        assert!(most.load(SeqCst) <= 6, "at most 6 at once, saw {}", most.load(SeqCst));
        assert!(most.load(SeqCst) > 1, "asked in parallel");
        assert_eq!(now.load(SeqCst), 0);
    }
}
