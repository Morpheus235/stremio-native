//! The stores behind the screens: pure state machines over `sn_stremio`'s protocol types. Each
//! plans the requests it needs as jobs, the application runs them off the frame thread, and their
//! outcomes land back here on the frame thread; a screen reads a store through
//! [`crate::registry::Views`] and hears that it moved as `ScreenEvent::StoreChanged` with the
//! store's [`StoreId`] ordinal. No store names a screen.

pub mod account; // the session: restore, sign in (password or device link), sign out
pub mod addons; // THE addons in use (`AddonsStore::in_use`): defaults, or the account's collection
pub mod catalog;
pub mod detail;
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
        })
    }
}
