//! **The streaming server store** — whether the server Settings names answers, and the servers a
//! scan of the home network found. Not the setting itself (the settings store keeps and persists
//! that address): this is what the network said about it, so its generation moves with every
//! answer and never makes the settings file rewrite.
//!
//! The store plans two jobs and the application runs them off the frame thread: a *check* of one
//! server (`GET {base}settings`, `sn_stremio::streaming_server::parse_settings`) and a *scan* of
//! the networks the television is on. Each job carries a sequence number; an answer for a check
//! or scan that has since been replaced is dropped, so reopening the page or changing the server
//! while one is out never shows the old answer. Nothing here is logged with an address.

use sn_stremio::streaming_server::ServerInfo;

/// What the last check of the server in use found.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ServerStatus {
    /// No server set, or none checked yet.
    #[default]
    Unknown,
    /// A check is out.
    Checking,
    /// It answered; its version when it said one.
    Alive(Option<String>),
    /// It did not answer, or answered as something else.
    Unreachable,
}

/// A server a scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The normalized base URL.
    pub base: String,
    pub version: Option<String>,
}

/// The scan's state.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Scan {
    /// None run since the app started.
    #[default]
    Idle,
    Searching,
    /// Finished: what it found, by address (possibly nothing).
    Done(Vec<Found>),
}

/// What the application is asked to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerJob {
    /// Ask the server at `base` whether it is alive.
    Check { base: String, seq: u32 },
    /// Look for servers on the networks the television is on.
    Scan { seq: u32 },
}

/// What a job brought back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerLanding {
    /// `info` is `None` when the server did not answer as one.
    Checked { base: String, seq: u32, info: Option<ServerInfo> },
    Scanned { seq: u32, found: Vec<Found> },
}

#[derive(Debug, Default)]
pub struct ServerStore {
    /// The server `status` is about.
    base: Option<String>,
    status: ServerStatus,
    check_seq: u32,
    scan: Scan,
    scan_seq: u32,
    gen: u32,
}

impl ServerStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    fn bump(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    pub fn status(&self) -> &ServerStatus {
        &self.status
    }

    pub fn scan_state(&self) -> &Scan {
        &self.scan
    }

    /// The servers the last finished scan found (none while it runs or before it ran).
    pub fn found(&self) -> &[Found] {
        match &self.scan {
            Scan::Done(found) => found,
            _ => &[],
        }
    }

    /// **Check the server in use**: `base` is the setting now (`None` is off). Any check still out
    /// is superseded. Returns the job to run.
    pub fn check(&mut self, base: Option<&str>) -> Option<ServerJob> {
        let Some(base) = base else {
            if self.base.take().is_some() || self.status != ServerStatus::Unknown {
                self.status = ServerStatus::Unknown;
                self.check_seq = self.check_seq.wrapping_add(1);
                self.bump();
            }
            return None;
        };
        self.base = Some(base.to_string());
        self.status = ServerStatus::Checking;
        self.check_seq = self.check_seq.wrapping_add(1);
        self.bump();
        Some(ServerJob::Check { base: base.to_string(), seq: self.check_seq })
    }

    /// **Start a scan** of the home network; `None` while one is already running.
    pub fn scan(&mut self) -> Option<ServerJob> {
        if self.scan == Scan::Searching {
            return None;
        }
        self.scan = Scan::Searching;
        self.scan_seq = self.scan_seq.wrapping_add(1);
        self.bump();
        Some(ServerJob::Scan { seq: self.scan_seq })
    }

    /// Fold an answer in. `true` when what the pages read changed.
    pub fn land(&mut self, landing: ServerLanding) -> bool {
        match landing {
            ServerLanding::Checked { base, seq, info } => {
                if seq != self.check_seq || self.base.as_deref() != Some(base.as_str()) {
                    return false; // a check since replaced, or for a server no longer set
                }
                self.status = match info {
                    Some(info) => ServerStatus::Alive(info.version),
                    None => ServerStatus::Unreachable,
                };
                self.bump();
                true
            }
            ServerLanding::Scanned { seq, found } => {
                if seq != self.scan_seq || self.scan != Scan::Searching {
                    return false;
                }
                self.scan = Scan::Done(found);
                self.bump();
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "http://192.168.1.20:11470/";
    const B: &str = "http://192.168.1.21:11470/";

    fn alive(v: &str) -> Option<ServerInfo> {
        Some(ServerInfo { version: Some(v.into()) })
    }

    fn seq(job: Option<ServerJob>) -> u32 {
        match job {
            Some(ServerJob::Check { seq, .. } | ServerJob::Scan { seq }) => seq,
            None => panic!("a job was expected"),
        }
    }

    #[test]
    fn a_check_runs_for_the_server_set_and_its_answer_is_the_status() {
        let mut s = ServerStore::new();
        assert_eq!(s.status(), &ServerStatus::Unknown);
        assert_eq!(s.check(None), None, "no server, nothing to ask");
        assert_eq!(s.gen(), 0, "…and nothing moved");
        let job = s.check(Some(A));
        assert!(matches!(&job, Some(ServerJob::Check { base, .. }) if base == A));
        assert_eq!(s.status(), &ServerStatus::Checking);
        assert!(s.land(ServerLanding::Checked { base: A.into(), seq: seq(job), info: alive("4.20.8") }));
        assert_eq!(s.status(), &ServerStatus::Alive(Some("4.20.8".into())));
        let job = s.check(Some(A));
        assert_eq!(s.status(), &ServerStatus::Checking, "asked again: checking, not the old answer");
        assert!(s.land(ServerLanding::Checked { base: A.into(), seq: seq(job), info: None }));
        assert_eq!(s.status(), &ServerStatus::Unreachable);
    }

    #[test]
    fn an_answer_for_a_replaced_check_or_another_server_is_dropped() {
        let mut s = ServerStore::new();
        let first = seq(s.check(Some(A)));
        let second = seq(s.check(Some(A)));
        let gen = s.gen();
        assert!(!s.land(ServerLanding::Checked { base: A.into(), seq: first, info: alive("1") }), "superseded");
        assert_eq!((s.status(), s.gen()), (&ServerStatus::Checking, gen));
        let other = seq(s.check(Some(B)));
        assert!(!s.land(ServerLanding::Checked { base: A.into(), seq: other, info: alive("1") }), "another server's answer");
        assert!(s.land(ServerLanding::Checked { base: B.into(), seq: other, info: alive("2") }));
        assert!(second != other);
        // switched off while a check was out: its answer names nothing
        let out = seq(s.check(Some(A)));
        assert_eq!(s.check(None), None);
        assert_eq!(s.status(), &ServerStatus::Unknown);
        assert!(!s.land(ServerLanding::Checked { base: A.into(), seq: out, info: alive("1") }));
        assert_eq!(s.status(), &ServerStatus::Unknown);
        let gen = s.gen();
        assert_eq!(s.check(None), None);
        assert_eq!(s.gen(), gen, "off again is no change");
    }

    #[test]
    fn a_scan_runs_once_at_a_time_and_lands_its_servers() {
        let mut s = ServerStore::new();
        assert_eq!((s.scan_state(), s.found()), (&Scan::Idle, &[][..]));
        let job = s.scan();
        assert_eq!(s.scan_state(), &Scan::Searching);
        assert_eq!(s.scan(), None, "one scan at a time");
        let found = vec![Found { base: A.into(), version: Some("4.20.8".into()) }, Found { base: B.into(), version: None }];
        assert!(s.land(ServerLanding::Scanned { seq: seq(job), found: found.clone() }));
        assert_eq!((s.scan_state(), s.found()), (&Scan::Done(found.clone()), &found[..]));
        assert!(!s.land(ServerLanding::Scanned { seq: 1, found: vec![] }), "an answer with no scan out is dropped");
        let again = s.scan();
        assert!(s.found().is_empty(), "the old list is gone while it searches");
        assert!(!s.land(ServerLanding::Scanned { seq: 1, found: found.clone() }), "the first scan's answer is stale");
        assert!(s.land(ServerLanding::Scanned { seq: seq(again), found: vec![] }));
        assert_eq!(s.scan_state(), &Scan::Done(vec![]), "nothing found is a finished scan");
    }
}
