# StremioNative

A native Stremio client for rooted LG webOS TVs (webOS 4.5 and later), written in Rust with a
small C boot shim. It draws its own UI with OpenGL ES and plays media through the TV's hardware
decoders (StarfishMediaAPIs) instead of the web runtime.

Status: early. The foundation layers are in place; the Stremio addon layer, the player and the
screens are being built (see [docs/roadmap.md](docs/roadmap.md)).

## Built on PlxNative

StremioNative is built on [PlxNative](https://github.com/GLinnik21/plx-native), the native Plex
client for LG webOS. Its renderer, UI toolkit, focus engine, network transport, Starfish
hardware-playback seam, FFmpeg demuxer and subtitle pipeline are the foundation this app was
ported from, and much of what makes it work on these TVs was worked out there first. Thank you to
its author and contributors.

## Layout

| Path | What |
| --- | --- |
| `crates/base` | event log, file locations, spawn discipline, dlopen seam, frame instruments |
| `crates/machine` | state machines, effects, the present gate, idle wake, motion |
| `crates/net` | HTTP(S) transport over the TV's libcurl, TLS fallbacks, DNS pins |
| `crates/platform` | webOS services, device capabilities, i18n, image cache, storage |
| `crates/gfx` | GL ES renderer, text, SVG, images |
| `crates/ui` | theme, layout, focus engine, widgets, card rows and grids |
| `crates/storage` | the private storage helper process |
| `native/` | C boot/crash shim, Starfish/ACB seam, libass glue, nanosvg rasterizer |
| `locales/` | translation catalogs |
| `assets/`, `pkg/` | icons, fonts, the packaged app manifest and root bundle |

## Host build and tests

```sh
# Debian/Ubuntu: apt-get install libsdl2-dev libsdl2-ttf-dev libgl-dev
CARGO_INCREMENTAL=0 cargo test --workspace
```

The TV build cross-compiles to `arm-unknown-linux-gnueabi` with the webOS NDK; `.cargo/config.toml`
pins the codegen flags that keep the binary from SIGILL-ing on the TV.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE), [NOTICE](NOTICE), [docs/legal.md](docs/legal.md) and [PRIVACY.md](PRIVACY.md). Not affiliated with Stremio.
