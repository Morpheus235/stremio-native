# The remote key map

Every key StremioNative binds, which field of the key event it is matched in, what it does, and
**where each code comes from**. The viewer's version of this table is Settings > Remote keys
(`crates/screens/src/settings.rs`, `Doc::Remote`).

StremioNative is derived from PlxNative, which runs on the same television with the same remote,
so the codes below are PlxNative's (`crates/ui/src/consts.rs`, ported unchanged, and PlxNative's
`docs/remote-keys.md`). Nothing in this file was measured on a television by this project yet;
the **Provenance** column says, for every code, who established it and how.

## How a press reaches the app

LG's SDL fork writes a key event's state at `+16`, the webOS key code (`wcode`, which is the SDL
*scancode*) at `+20` and the SDL `sym` at `+24` (`crates/app/src/app/input.rs`). Keys from 300
upwards are LG's private range: `sym` arrives as 0 and they are matched in `wcode`.

Each raw press goes through two steps, in this order (`crates/app/src/app/run.rs`, `ingest`):

1. **The hotkeys** (`sn_screens::tabs::route_hotkey`). A hotkey is taken — and never reaches a
   page — on its DOWN edge (its repeat and release are spent with it), **except** while
   - the television keyboard is up (text entry: the key belongs to the text),
   - a popover menu is open (the menu owns the remote),
   - the player is on top (its transport keys are its own).
   In those three cases the key goes on to the dispatcher like any other.
2. **The dispatcher** (`sn_ui::consts::classify`): the navigation keys, OK, BACK and the
   transport keys.

EXIT is checked before both and ends the process.

## Navigation

| key | code | field | does | provenance |
|---|---|---|---|---|
| ▲ ▼ ◀ ▶ | sym `82/81/80/79 \| 1<<30` | `sym` | move focus; UP from a page's first row reaches the top bar; ◀ ▶ walk the bar's pills | PlxNative, device-measured (336 key lines, 2026-08-22) |
| OK | sym `13` (RETURN) | `sym` | open / choose; on a pill, select that tab; hold on a card for its item menu | PlxNative, device-measured |
| BACK | wcode `482` (also `461`) | `wcode` | on a page's content: focus to the top bar; on the bar of a tab other than Home: Home; on Home's bar: the television's Home (the app keeps running); on a pushed page: back; in a menu: close it | 482 PlxNative device-measured; 461 is a defensive second code with no evidence on this firmware |
| EXIT | wcode `505` | either | close StremioNative | PlxNative, from the firmware's evdev table (evdev 174 `KEY_EXIT`) |

BACK on the bar at Home follows PlxNative: no "exit?" prompt. The webOS ≤ 5 platform behaviour
for an entry page's BACK is to show the Home launcher, and the remote's EXIT key is the one that
quits (PlxNative `docs/remote-keys.md` §8).

## Shortcuts (the hotkeys)

| key | code | field | does | provenance |
|---|---|---|---|---|
| RED | wcode `486`, sym `0` | `wcode` | previous tab (wraps) | **device-measured** by PlxNative on the dev set, 2026-08-26 (all four colour keys pressed with the log open) |
| GREEN | wcode `487`, sym `0` | `wcode` | next tab (wraps) | device-measured, as RED |
| YELLOW | wcode `488`, sym `0` | `wcode` | Settings | device-measured, as RED |
| BLUE | wcode `489`, sym `0` | `wcode` | Search | device-measured, as RED |
| CH ▲ | wcode `300` | `wcode` | previous tab (wraps) | **not measured**: read from the firmware's evdev→scancode table (evdev 402 `KEY_CHANNELUP`; `libSDL2-2.0.so.0.4.1`, offset `0x92840`) by PlxNative; never observed off a remote |
| CH ▼ | wcode `301` | `wcode` | next tab (wraps) | **not measured**: evdev 403 `KEY_CHANNELDOWN`, as CH ▲ |

The tab order is the bar's: Home, Live TV, Library, Addons, Settings, Search
(`sn_screens::tabs::TABS`). A hotkey pressed on a page pushed over a tab (a Detail page opened
from Search) leaves that page; the hotkey for the tab already under it returns to that tab rather
than opening a fresh one.

Why the colour keys and not the standard evdev colour codes: on this firmware `KEY_RED` (398),
`KEY_YELLOW` (400) and `KEY_BLUE` (401) map to 0 (not producible) and `KEY_GREEN` (399) maps to
504, a code the real remote never sends. The measured 486–489 are LG's private block
(evdev 289–292). (PlxNative, `docs/remote-keys.md` §9.)

**There is no dedicated Search key binding.** No remote search button has a code on record —
neither measured nor in the firmware table PlxNative read — so binding one would be a guess. BLUE
is the Search shortcut.

PlxNative leaves the colour keys unbound (LG's checklist row #40 wants unsupported keys inert);
StremioNative binds them on purpose, as navigation shortcuts.

## During playback

| key | code | field | does | provenance |
|---|---|---|---|---|
| OK, PLAY, PAUSE, PLAY/PAUSE | sym 13; wcode `450`/`19`/`402` (play), `72`/`415` (pause), `261` (toggle) | as `classify` | pause or resume | PlxNative's transport bindings, unchanged; each code's provenance is in PlxNative's `docs/remote-keys.md` §2 (261 is evdev 164 `KEY_PLAYPAUSE` from the firmware table; 19 is `SDL_SCANCODE_P`, an open question there, §4.2) |
| BACK, STOP | wcode `482`; `413`/`120`/`260` | as `classify` | stop and go back | 120/260 from the firmware table (`KEY_STOP`/`KEY_STOPCD`); 413 kept by PlxNative with no positive evidence |

## While typing (the Search field)

The television's own keyboard draws over the app and commits text as `SDL_TEXTINPUT` (text at
`+16` in the fork's event, `crates/app/src/textinput.rs`, ported from PlxNative). Its edit keys
arrive as keys: ◀ ▶ move the caret, delete is sym `8` (backspace), *Clear all* is sym
`156 | 1<<30` (PlxNative, device-measured 2026-08-15). OK or ▼ commits the query (and keeps it
on the recent searches), BACK puts the keyboard away. No hotkey fires while the keyboard is up.

## What a TV session should settle

1. CH ▲ / CH ▼ really are 300 / 301 on the Magic Remote and on a standard IR remote (PlxNative
   `docs/remote-keys.md` §7 is the capture recipe: press every button with the event log open
   and decode `+20`).
2. Whether a remote with a dedicated search or voice button sends a code this app could bind.
3. That the colour keys still read 486–489 on this firmware build (they were measured on the
   PlxNative dev set's firmware, 4.10.x).
