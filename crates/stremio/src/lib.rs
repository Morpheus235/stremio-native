//! The Stremio addon protocol, independent of any transport.
//!
//! An addon is a base URL (its "transport URL" minus the trailing `manifest.json`) that answers
//! `GET {base}/{resource}/{type}/{id}[/{extra}].json`. This crate owns everything about that
//! protocol that can be decided without a socket:
//!
//! * [`manifest`] — what an addon declares: resources, types, id prefixes, catalogs and their
//!   extra properties (`search`, `genre`, `skip`).
//! * [`url`] — building resource URLs, with the percent-encoding Stremio addons expect.
//! * [`types`] — the response bodies: catalog previews, metas, streams, subtitles.
//! * [`catalog`] — paging a catalog to completion without trusting a fixed page size.
//! * [`search`] — which catalogs a query goes to, and each addon's result state.
//! * [`account`] — signing in and reading the user's installed addon collection.
//! * [`sched`] — the viewport-first request queue: what is on screen loads first, and work for
//!   rows scrolled past is cancelled rather than drained top-down.
//!
//! The HTTP layer is the [`Fetch`] trait, so the policy here is tested without a network.

pub mod account;
pub mod catalog;
pub mod manifest;
pub mod sched;
pub mod search;
pub mod types;
pub mod url;

/// One HTTP GET, as the addon layer needs it. Implemented over `sn_net` by the application and by
/// in-memory fakes in tests.
pub trait Fetch {
    /// GET `url` with the extra request `headers`, returning the body of a 2xx response.
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<Vec<u8>, FetchError>;
    /// POST `body` to `url` with `headers`, returning the body of a 2xx response.
    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<Vec<u8>, FetchError>;
}

/// Why a fetch produced no usable body. Every variant is shown to the user as that addon's
/// failure; none is ever folded into an empty result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The transport failed (DNS, connect, TLS, reset) or the deadline passed.
    Transport(String),
    /// The server answered with a non-2xx status.
    Status(u16),
    /// The body was not the JSON shape the resource requires.
    Decode(String),
    /// The request was cancelled by the scheduler before it completed.
    Cancelled,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Transport(why) => write!(f, "transport: {why}"),
            FetchError::Status(code) => write!(f, "HTTP {code}"),
            FetchError::Decode(why) => write!(f, "bad response: {why}"),
            FetchError::Cancelled => f.write_str("cancelled"),
        }
    }
}

/// Fetch `url` and decode it as `T`.
pub fn get_json<T: serde::de::DeserializeOwned>(
    fetch: &dyn Fetch,
    url: &str,
    headers: &[(String, String)],
) -> Result<T, FetchError> {
    let body = fetch.get(url, headers)?;
    serde_json::from_slice(&body).map_err(|e| FetchError::Decode(e.to_string()))
}

#[cfg(test)]
pub(crate) mod fake {
    use super::{Fetch, FetchError};
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Answers from a URL → body map and records every URL asked for.
    #[derive(Default)]
    pub struct FakeFetch {
        pub bodies: HashMap<String, Result<String, FetchError>>,
        pub asked: RefCell<Vec<String>>,
    }

    impl FakeFetch {
        pub fn with(mut self, url: &str, body: &str) -> Self {
            self.bodies.insert(url.to_string(), Ok(body.to_string()));
            self
        }
        pub fn failing(mut self, url: &str, err: FetchError) -> Self {
            self.bodies.insert(url.to_string(), Err(err));
            self
        }
    }

    impl Fetch for FakeFetch {
        fn get(&self, url: &str, _headers: &[(String, String)]) -> Result<Vec<u8>, FetchError> {
            self.asked.borrow_mut().push(url.to_string());
            match self.bodies.get(url) {
                Some(Ok(body)) => Ok(body.clone().into_bytes()),
                Some(Err(e)) => Err(e.clone()),
                None => Err(FetchError::Status(404)),
            }
        }
        fn post(&self, url: &str, headers: &[(String, String)], _body: &[u8]) -> Result<Vec<u8>, FetchError> {
            self.get(url, headers)
        }
    }
}
