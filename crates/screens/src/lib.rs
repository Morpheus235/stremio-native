//! sn_screens: the application's screens and the bundle they are compiled against.
//!
//! `registry` is the bundle (`Bundle`, the `Host` every container and screen here is generic
//! over) and the one `mount` match; `stores` are the state machines the screens read through
//! `registry::Views`; the rest are screens. The layer rule: a screen names `sn_ui`, `stores`,
//! `registry` and the protocol types of `sn_stremio`, never the application crate and never a
//! sibling screen — reaching another screen is a navigation op, reaching the application is an
//! `AppFx`. Read `CLAUDE.md` beside this file before adding a screen.

pub mod detail; // one item: episodes or per-addon streams; in its second mode, one episode's streams
pub mod home; // a shelf per browsable catalog of the installed addons, viewport-first
pub mod player; // the hardware video plane's page: working / failure read-outs, remote transport
pub mod registry; // the bundle: AppArg, AppFx, Views, Bundle, AppMounter
pub mod stores; // the catalog and detail stores (pure; jobs out, landings in)

#[cfg(any(test, feature = "test-support"))]
pub mod test_support; // a rig over the real stores and the dispatcher, for driving screens with no SDL

#[cfg(test)]
mod home_tests;
