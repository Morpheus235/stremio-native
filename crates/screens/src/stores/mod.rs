//! The stores behind the screens: pure state machines over `sn_stremio`'s protocol types. Each
//! plans the requests it needs as jobs, the application runs them off the frame thread, and their
//! outcomes land back here on the frame thread; a screen reads a store through
//! [`crate::registry::Views`] and hears that it moved as `ScreenEvent::StoreChanged` with the
//! store's [`StoreId`] ordinal. No store names a screen.

pub mod account; // the session: restore, sign in (password or device link), sign out
pub mod addons; // THE addons in use (`AddonsStore::in_use`): defaults, or the account's collection
pub mod catalog;
pub mod detail;
pub mod epg; // the TV guide: what Live TV's channels show now and next
pub mod library; // the account's library: Continue Watching and the Library page
pub mod grid;
pub mod search;
pub mod settings;

use sn_machine::machine::StoreOrd;

/// Every store a screen can hear from, as the dispatcher's `StoreOrd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreId {
    Catalog,
    Detail,
    /// The playback publication the player page reads (`registry::PlaybackView`).
    Playback,
    Account,
    Addons,
    Library,
    Search,
    Grid,
    Settings,
    /// The TV guide (`epg::EpgStore`).
    Epg,
}

impl StoreId {
    pub const fn ord(self) -> StoreOrd {
        StoreOrd(match self {
            StoreId::Catalog => 0,
            StoreId::Detail => 1,
            StoreId::Playback => 2,
            StoreId::Account => 6,
            StoreId::Addons => 7,
            StoreId::Library => 8,
            StoreId::Search => 3,
            StoreId::Grid => 4,
            StoreId::Settings => 5,
            StoreId::Epg => 9,
        })
    }
}

/// **A failure's cause, short, localised and scrubbed** — THE one reading of a [`FetchError`] a
/// page shows (the stream list, Search's closing line, the Addons page): a status code, or a word
/// for the kind of failure, never the transport's own English message (which can carry a host).
pub fn fetch_reason(e: &sn_stremio::FetchError) -> String {
    use sn_platform::i18n::msg;
    use sn_stremio::FetchError;
    match e {
        FetchError::Status(code) => format!("HTTP {code}"),
        FetchError::Transport(why) if why.to_ascii_lowercase().contains("time") => msg::app_reason_timeout().to_string(),
        FetchError::Transport(_) => msg::app_reason_network().to_string(),
        FetchError::Decode(_) => msg::app_reason_bad_response().to_string(),
        FetchError::Cancelled => msg::app_reason_cancelled().to_string(),
    }
}

/// [`fetch_reason`] for the account service: its own message when it sent one (written for people,
/// server text), else the error number, else the transport's cause.
pub fn api_reason(e: &sn_stremio::account::ApiError) -> String {
    use sn_stremio::account::ApiError;
    match e {
        ApiError::Api { message, .. } if !message.trim().is_empty() => message.trim().to_string(),
        ApiError::Api { code, .. } => sn_platform::i18n::msg::app_reason_api_error(*code),
        ApiError::Fetch(f) => fetch_reason(f),
    }
}

#[cfg(test)]
mod reason_tests {
    use super::*;
    use sn_platform::i18n::{language_on_this_thread_for_test, msg, Preference};
    use sn_stremio::FetchError;

    /// No raw English transport text reaches a page: every cause is the UI language's word.
    #[test]
    fn a_failure_reads_in_the_ui_language() {
        let _g = language_on_this_thread_for_test(Preference::Fr);
        assert_eq!(fetch_reason(&FetchError::Transport("Operation timed out after 15000 ms".into())), msg::app_reason_timeout());
        assert_eq!(fetch_reason(&FetchError::Transport("Could not resolve host: x.example".into())), "pas de connexion");
        assert_eq!(fetch_reason(&FetchError::Decode("expected value at line 1".into())), msg::app_reason_bad_response());
        assert_eq!(fetch_reason(&FetchError::Status(503)), "HTTP 503");
        let api = sn_stremio::account::ApiError::Fetch(FetchError::Transport("reset".into()));
        assert_eq!(api_reason(&api), msg::app_reason_network());
        assert_eq!(api_reason(&sn_stremio::account::ApiError::Api { code: 7, message: String::new() }), "erreur 7");
    }
}
