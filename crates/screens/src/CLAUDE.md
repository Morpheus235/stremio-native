# screens/ — the application's screens (read `../../ui/src/CLAUDE.md` first)

The crate `sn_screens`: every screen the dispatcher mounts, the [`Bundle`] host they share
(`registry.rs`) and the stores they read (`stores/`).

## The layer rule

| module | may name |
|---|---|
| `stores/` | `sn_stremio`, `sn_machine::machine` — never a screen, never `sn_ui` |
| a screen (`home.rs`, `detail.rs`, `player.rs`) | `sn_ui`, `stores/`, `sn_stremio` types, `registry` — never a sibling screen, never the app |
| `registry.rs` | the screens (it is the one `mount` match) |

Nothing here fetches, spawns or blocks. A screen says what it wants as an `AppFx` (`registry.rs`);
the application's rig (`crates/app/src/app/bridge.rs`) changes the store, runs the store's jobs on
workers and folds their landings back in; the tree then hears `StoreChanged` and the screen
re-reads its `Views`. A store is pure: commands in, jobs out, landings in — which is what lets
`stores/*` tests drive it without a network and `test_support::TestApp` drive a whole screen
through the real dispatcher.

## Adding a screen

1. A `word::` name, an `AppArg` variant with its `ScreenArg` (`ScreenId`, `Chrome`) and
   `LogicalState`, and its arm in `AppMounter::mount`.
2. The screen: `Machine<Bundle>` + `Screen<Bundle>`, built from `sn_ui` components (`cards::Stack`
   for shelves, `TableScreen` for lists, `StatusOverlay` for read-outs) — never a bespoke draw.
3. If its content lands after it mounts, seat focus with `registry::FirstSeat` on its tick, or OK
   does nothing until the viewer presses a direction.
4. A `TestApp` test of what it asks for and where OK takes it.

App-owned text is a `sn_platform::i18n::msg` accessor (`locales/*/app.json`); server text (titles,
addon names, stream labels) is never logged.
