# screens/ — the application's screens (read `../../ui/src/CLAUDE.md` first)

The crate `sn_screens`: every screen the dispatcher mounts, the [`Bundle`] host they share
(`registry.rs`) and the stores they read (`stores/`).

## The layer rule

| module | may name |
|---|---|
| `stores/` | `sn_stremio`, `sn_machine::machine` — never a screen, never `sn_ui` |
| a screen (`home.rs`, `search.rs`, `livetv.rs`, `library.rs`, `addons.rs`, `settings.rs`, `grid.rs`, `menu.rs`, `detail.rs`, `player.rs`, `signin.rs`) | `sn_ui`, `stores/`, `sn_stremio` types, `registry`, `tabs`, `cards`, `item_cards` — never a sibling screen, never the app |
| `tabs.rs`, `cards.rs`, `chrome.rs`, `item_cards.rs` (shared, not screens) | `sn_ui`, `stores/`, `registry` |
| `registry.rs` | the screens (it is the one `mount` match) |

**The addons the app uses** are ONE answer: `stores::addons::AddonsStore::in_use()` (the account's
collection signed in, `DEFAULT_ADDONS` otherwise), and their loaded manifests are
`registry::Views::addons()` (the catalog store the application rebuilds from `in_use`). Nothing
here reads `sn_stremio::account::DEFAULT_ADDONS` itself; a search plans against `Views::addons()`
filtered by `AddonsStore::filter()` (the per-addon switch the Addons page toggles).

Nothing here fetches, spawns or blocks. A screen says what it wants as an `AppFx` (`registry.rs`);
the application's rig (`crates/app/src/app/bridge.rs`) changes the store, runs the store's jobs on
workers and folds their landings back in; the tree then hears `StoreChanged` and the screen
re-reads its `Views`. A store is pure: commands in, jobs out, landings in — which is what lets
`stores/*` tests drive it without a network and `test_support::TestApp` drive a whole screen
through the real dispatcher.

## The top bar and its destinations

The shared top bar is PlxNative's (`sn_ui::widgets::StripRender` + the container's `STRIP` row,
`sn_ui::containers::tabs`). `tabs::TABS` is the ONE list of destinations, in bar order — the bar
(`chrome::ChromeBar`), the colour/channel hotkeys (`tabs::route_hotkey`, `docs/remote-keys.md`)
and BACK all read it. Home is the root of the shared stack and every other tab is a
`NavOp::SelectTab` over it. A destination's page wears `Chrome::TabBar` (derived from
`Tab::of_arg`), keeps its content below `sn_ui::widgets::TOP_BAR_BOTTOM`
(`RouteLayout::below_top_bar` for a route page) and follows four rules, each a `tabs` helper:

- `Screen::links` declares `tabs::links(first_group)` (UP from the first row to the bar, DOWN back);
- BACK on its content is `tabs::on_back` (focus to its own pill; on the bar the container's BACK:
  a pop to Home, or at Home the application's root BACK);
- a pill's press is `tabs::on_pill`;
- a page whose content lands after it mounts seats it with `registry::FirstSeat`, counting only a
  seat `tabs::rests` accepts (the pill the remote arrived on is a seat nobody chose).

Adding a destination (the account page, say) is one `TABS` line, its `AppArg` variant and its
`mount` arm. The Search pill must stay LAST: the strip draws its last pill as the magnifier.

The addons every page plans against are `Views::addons()` (and `Bridge::addons()` on the rig
side): the catalog store the application builds from `AddonsStore::in_use()` — the account's
collection signed in, the defaults otherwise. Search's chips and the Addons page's *Include in
search* toggle the same `AddonsStore::filter()`. `Views::account` is the session (Settings' and the
Addons page's account rows, sign-out through `AccountCmd::SignOut`); `Views::session` is its
signed-in bit (the item menu's Add to library reads it, with `LibraryStore::add_state`; the press
is `LibraryCmd::Add`, the account's `datastorePut`).

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
