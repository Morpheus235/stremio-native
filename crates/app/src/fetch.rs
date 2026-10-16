//! **The addon transport**: `sn_stremio::Fetch` over `sn_net`'s blocking libcurl request.
//!
//! Every addon request (a manifest, a catalog page, a meta, a stream list) and every artwork fetch
//! goes through [`NetFetch`]. It runs only off the frame thread — it takes the [`OffFrame`] proof,
//! and `sn_net` asserts it may block — with:
//!
//! - **deadlines**: [`Profile::Api`] is `sn_net::API` (8 s connect, 25 s whole request), the
//!   answer a user waits for behind a spinner; [`Profile::Art`] is `sn_net::BULK` (8 s connect,
//!   no wall clock, a transfer below one byte a second for 30 s ends);
//! - **a bounded body**: a response larger than the profile's cap fails as a transport error
//!   instead of growing without limit on a 32-bit set;
//! - **the product's `User-Agent`** (`StremioNative/<version>`), installed once at boot through
//!   `sn_net::set_user_agent`, so every request carries it;
//! - **cancellation** through the scheduler's [`Cancel`] token, polled before the request starts:
//!   a row scrolled out of the keep window while its load waited answers `Cancelled`.
//!
//! What a failure says is a closed vocabulary (`timed out`, `curl <code>`, `body over <n> bytes`)
//! and never the URL: an addon URL can carry the user's configuration.

use sn_base::task::OffFrame;
use sn_stremio::sched::Cancel;
use sn_stremio::{Fetch, FetchError};

/// The largest addon JSON accepted: a catalog page or a stream list of a few hundred entries is
/// well under a megabyte; eight leaves room for an addon that inlines long descriptions.
pub const API_BODY_MAX: usize = 8 << 20;
/// The largest artwork body accepted (a poster or a backdrop JPEG/PNG).
pub const ART_BODY_MAX: usize = 6 << 20;

/// Which deadlines and body cap a request runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Api,
    Art,
}

impl Profile {
    fn timeouts(self) -> sn_net::net::Timeouts {
        match self {
            Profile::Api => sn_net::net::API,
            Profile::Art => sn_net::net::BULK,
        }
    }
    fn body_max(self) -> usize {
        match self {
            Profile::Api => API_BODY_MAX,
            Profile::Art => ART_BODY_MAX,
        }
    }
}

/// `sn_stremio::Fetch` over `sn_net`. Borrowing the worker's [`OffFrame`] is what makes it
/// impossible to build one on the frame thread.
pub struct NetFetch<'a> {
    _off: &'a OffFrame,
    profile: Profile,
    cancel: Option<Cancel>,
    body_max: usize,
    /// Headers the APPLICATION writes (never an addon's), sent on every request and safe to send
    /// on to whatever host a redirect names.
    own: &'static [(&'static str, &'static str)],
}

/// What an artwork request says it accepts: WebP first, so a host that negotiates (a CDN in front
/// of an addon's posters) answers the smaller encoding; anything else it has after that.
pub const ART_ACCEPT: &[(&str, &str)] = &[("Accept", "image/webp,image/*;q=0.8")];

impl<'a> NetFetch<'a> {
    pub fn new(off: &'a OffFrame, profile: Profile) -> Self {
        NetFetch { _off: off, profile, cancel: None, body_max: profile.body_max(), own: &[] }
    }

    /// Send the application's own `headers` too. Unlike a caller's (an addon's, which may carry a
    /// credential for its origin), these do not stop a redirect from being followed: they say
    /// nothing a redirected-to host should not hear.
    pub fn own_headers(mut self, headers: &'static [(&'static str, &'static str)]) -> Self {
        self.own = headers;
        self
    }

    /// Answer `Cancelled` instead of starting when `cancel` is set.
    pub fn cancelled_by(mut self, cancel: Cancel) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// A tighter body cap than the profile's (tests).
    pub fn body_max(mut self, bytes: usize) -> Self {
        self.body_max = bytes.min(self.body_max);
        self
    }

    fn send(&self, verb: &str, url: &str, headers: &[(String, String)], body: Option<&[u8]>) -> Result<Vec<u8>, FetchError> {
        if self.cancel.as_ref().is_some_and(Cancel::is_cancelled) {
            return Err(FetchError::Cancelled);
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(FetchError::Transport("not an http(s) url".into()));
        }
        let mut lines = header_lines(headers)?;
        // `sn_net` follows a redirect only for a request that carries no header of the caller's
        // (libcurl would re-send them to whatever host the redirect names), so a request with
        // headers answers a 3xx as its status instead. The application's own headers do not
        // count: an artwork request with only its `Accept` still follows a CDN's redirect.
        let follow = lines.is_empty();
        for (name, value) in self.own {
            if !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
                lines.push(format!("{name}: {value}"));
            }
        }
        let resp = sn_net::net::request_result_evidence(
            url,
            &lines,
            verb,
            body,
            self.profile.timeouts(),
            follow,
            Some(self.body_max),
            None,
        )
        .map_err(|f| transport_error(&f))?;
        if !resp.ok() {
            return Err(FetchError::Status(resp.status));
        }
        Ok(resp.body)
    }
}

impl Fetch for NetFetch<'_> {
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<Vec<u8>, FetchError> {
        self.send("GET", url, headers, None)
    }
    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<Vec<u8>, FetchError> {
        let mut with_type = headers.to_vec();
        if !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("content-type")) {
            with_type.push(("Content-Type".into(), "application/json".into()));
        }
        self.send("POST", url, &with_type, Some(body))
    }
}

/// `Name: value` lines for libcurl. A name that is not an HTTP token, or a value carrying a line
/// break or a NUL, is refused rather than sent: it would split the request.
pub fn header_lines(headers: &[(String, String)]) -> Result<Vec<String>, FetchError> {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    headers
        .iter()
        .map(|(name, value)| {
            let name = name.trim();
            if name.is_empty() || !name.chars().all(token) || value.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
                return Err(FetchError::Transport("refused a malformed request header".into()));
            }
            Ok(format!("{name}: {}", value.trim()))
        })
        .collect()
}

/// A transport failure as the user's read-out and the log may carry it: no URL, no header.
fn transport_error(f: &sn_net::net::RequestFailure) -> FetchError {
    use sn_net::net::RequestError;
    if let Some(limit) = f.body_limit {
        return FetchError::Transport(format!("body over {limit} bytes"));
    }
    match (f.cause, f.curl_rc) {
        (RequestError::TimedOut, _) => FetchError::Transport("timed out".into()),
        (RequestError::Transport, Some(rc)) => FetchError::Transport(format!("curl {rc}")),
        (RequestError::Transport, None) => FetchError::Transport("no transport".into()),
    }
}

#[cfg(test)]
mod tests {
    //! The adapter against a real loopback HTTP server through the real libcurl stack.
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// One canned reply per path; every request head is recorded.
    struct Server {
        port: u16,
        seen: Arc<Mutex<Vec<String>>>,
    }

    fn serve(routes: Vec<(&'static str, u16, Vec<u8>)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut sock) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // the head, then a Content-Length body if any
                loop {
                    let Ok(n) = sock.read(&mut chunk) else { break };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_string();
                        let len = head
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                log.lock().unwrap().push(text);
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, s, b)| (*s, b.clone()))
                    .unwrap_or((404, Vec::new()));
                // a 3xx route's "body" is where it redirects to
                let (location, body) = if (300..400).contains(&status) {
                    (format!("Location: {}\r\n", String::from_utf8_lossy(&body)), Vec::new())
                } else {
                    (String::new(), body)
                };
                let head = format!("HTTP/1.1 {status} X\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(&body);
            }
        });
        Server { port, seen }
    }

    fn ready() -> bool {
        sn_net::net::curl_ready()
    }

    fn ua() {
        let _ = sn_net::net::set_user_agent(crate::user_agent());
    }

    #[test]
    fn a_2xx_body_comes_back_and_carries_the_user_agent_and_headers() {
        if !ready() {
            return;
        }
        ua();
        let s = serve(vec![("/manifest.json", 200, br#"{"id":"x"}"#.to_vec())]);
        let off = OffFrame::for_test();
        let f = NetFetch::new(&off, Profile::Api);
        let body = f
            .get(&format!("http://127.0.0.1:{}/manifest.json", s.port), &[("X-Probe".into(), "yes".into())])
            .expect("a 200 is a body");
        assert_eq!(body, br#"{"id":"x"}"#);
        let seen = s.seen.lock().unwrap().join("\n");
        let want_ua = format!("User-Agent: {}", crate::user_agent());
        assert!(seen.contains(&want_ua), "the product's User-Agent: {seen}");
        assert!(seen.contains("X-Probe: yes"), "the addon's own header: {seen}");
    }

    #[test]
    fn a_non_2xx_status_is_a_status_error_never_an_empty_body() {
        if !ready() {
            return;
        }
        let s = serve(vec![("/gone", 410, b"nope".to_vec())]);
        let off = OffFrame::for_test();
        let f = NetFetch::new(&off, Profile::Api);
        assert_eq!(f.get(&format!("http://127.0.0.1:{}/gone", s.port), &[]), Err(FetchError::Status(410)));
        assert_eq!(f.get(&format!("http://127.0.0.1:{}/missing", s.port), &[]), Err(FetchError::Status(404)));
    }

    #[test]
    fn an_oversized_body_fails_instead_of_growing() {
        if !ready() {
            return;
        }
        let s = serve(vec![("/big", 200, vec![b'x'; 4096])]);
        let off = OffFrame::for_test();
        let f = NetFetch::new(&off, Profile::Api).body_max(1024);
        match f.get(&format!("http://127.0.0.1:{}/big", s.port), &[]) {
            Err(FetchError::Transport(why)) => assert!(why.contains("body over 1024"), "{why}"),
            other => panic!("an oversized body is a transport failure: {other:?}"),
        }
    }

    #[test]
    fn a_post_sends_its_body_as_json() {
        if !ready() {
            return;
        }
        let s = serve(vec![("/api/addonCollectionGet", 200, br#"{"result":{}}"#.to_vec())]);
        let off = OffFrame::for_test();
        let f = NetFetch::new(&off, Profile::Api);
        let url = format!("http://127.0.0.1:{}/api/addonCollectionGet", s.port);
        assert_eq!(f.post(&url, &[], br#"{"type":"AddonCollectionGet"}"#).unwrap(), br#"{"result":{}}"#);
        let seen = s.seen.lock().unwrap().join("\n");
        assert!(seen.starts_with("POST /api/addonCollectionGet"), "{seen}");
        assert!(seen.contains("Content-Type: application/json"), "{seen}");
        assert!(seen.ends_with(r#"{"type":"AddonCollectionGet"}"#), "{seen}");
    }

    #[test]
    fn an_artwork_request_asks_for_webp_and_still_follows_a_redirect() {
        if !ready() {
            return;
        }
        let s = serve(vec![("/poster.jpg", 302, b"/cdn/poster.webp".to_vec()), ("/cdn/poster.webp", 200, b"RIFFwebp".to_vec())]);
        let off = OffFrame::for_test();
        let f = NetFetch::new(&off, Profile::Art).own_headers(ART_ACCEPT);
        let body = f.get(&format!("http://127.0.0.1:{}/poster.jpg", s.port), &[]).expect("the redirect is followed");
        assert_eq!(body, b"RIFFwebp");
        let seen = s.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "two hops");
        assert!(seen.iter().all(|r| r.contains("Accept: image/webp,image/*;q=0.8")), "every hop asks for WebP first: {seen:?}");
        // an addon's own header still stops the redirect (it could be a credential)
        let f = NetFetch::new(&off, Profile::Art).own_headers(ART_ACCEPT);
        assert_eq!(
            f.get(&format!("http://127.0.0.1:{}/poster.jpg", s.port), &[("Referer".into(), "https://addon.example/".into())]),
            Err(FetchError::Status(302))
        );
    }

    #[test]
    fn a_cancelled_request_never_dials() {
        let s = serve(vec![("/page", 200, b"{}".to_vec())]);
        let cancel = Cancel::default();
        cancel.cancel();
        let off = OffFrame::for_test();
        let f = NetFetch::new(&off, Profile::Api).cancelled_by(cancel);
        assert_eq!(f.get(&format!("http://127.0.0.1:{}/page", s.port), &[]), Err(FetchError::Cancelled));
        assert!(s.seen.lock().unwrap().is_empty(), "nothing reached the server");
    }

    #[test]
    fn a_dead_port_is_a_transport_error_that_names_no_url() {
        if !ready() {
            return;
        }
        let port = sn_net::net::dead_port();
        let off = OffFrame::for_test();
        let f = NetFetch::new(&off, Profile::Api);
        match f.get(&format!("http://127.0.0.1:{port}/secret-config/manifest.json"), &[]) {
            Err(FetchError::Transport(why)) => assert!(!why.contains("secret-config") && !why.contains("127.0.0.1"), "{why}"),
            other => panic!("a refused connect is a transport failure: {other:?}"),
        }
    }

    #[test]
    fn a_header_that_would_split_the_request_is_refused() {
        assert!(header_lines(&[("X-Ok".into(), "fine".into())]).is_ok());
        assert!(header_lines(&[("X-Bad".into(), "a\r\nInjected: 1".into())]).is_err());
        assert!(header_lines(&[("Bad Name".into(), "v".into())]).is_err());
        assert!(header_lines(&[("".into(), "v".into())]).is_err());
        let off = OffFrame::for_test();
        let f = NetFetch::new(&off, Profile::Api);
        assert!(matches!(f.get("file:///etc/passwd", &[]), Err(FetchError::Transport(_))), "only http(s)");
    }
}
