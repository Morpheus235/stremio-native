//! sn_screens: the application's screens and the bundle they are compiled against.
//!
//! `registry` is the bundle (`Bundle`, the `Host` every container and screen here is generic
//! over) and the one `mount` match; `tabs` is the one registry of top-bar destinations (and the
//! remote hotkeys); `chrome` owns the shared top bar; `stores` are the state machines the screens
//! read through `registry::Views`; the rest are screens. The layer rule: a screen names `sn_ui`,
//! `stores`, `registry`, `tabs`, `cards` and the protocol types of `sn_stremio`, never the
//! application crate and never a sibling screen — reaching another screen is a navigation op,
//! reaching the application is an `AppFx`. Read `CLAUDE.md` beside this file before adding a
//! screen.

pub mod addons; // a top-bar destination: the addons in use (the sign-in work fills it)
pub mod cards; // the CardSource over a run of catalog items that Search, the grid and Live TV share
pub mod chrome; // the shared top bar's owner: labels, strip motion, the members the container focuses
pub mod detail; // one item: episodes or per-addon streams; in its second mode, one episode's streams
pub mod grid; // one catalog whole: the paginated grid a search row's MORE opens
pub mod home; // a top-bar destination: a shelf per browsable catalog of the installed addons
pub mod library; // a top-bar destination: the user's library (signed out: how to get one)
pub mod livetv; // a top-bar destination: the tv/channel catalogs by genre
pub mod menu; // the popover menu: a card's item menu, a chip's choice
pub mod player; // the hardware video plane's page: working / failure read-outs, remote transport
pub mod registry; // the bundle: AppArg, AppFx, Views, Bundle, AppMounter
pub mod search; // a top-bar destination: the field, the addon filter, one result row per catalog
pub mod settings; // a top-bar destination and its family: playback, remote, about, legal, privacy
pub mod stores; // the catalog, detail, search, grid and settings stores (pure; jobs out, landings in)
pub mod tabs; // the top bar's destinations, BACK to the bar, and the remote's hotkeys

#[cfg(any(test, feature = "test-support"))]
pub mod test_support; // a rig over the real stores and the dispatcher, for driving screens with no SDL

#[cfg(test)]
mod home_tests;
#[cfg(test)]
mod nav_tests;
