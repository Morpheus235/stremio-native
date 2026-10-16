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
| `crates/media` | demux, the HLS client, stream probing, subtitles, the Starfish buffer-feed player |
| `crates/stremio` | the addon protocol: manifests, catalogs, metas, streams, viewport-first scheduling |
| `crates/screens` | the screens (Home, Detail/streams, the player page) and the stores they read |
| `crates/app` | the application core the C shim links: boot, the frame loop, the webOS port, workers |
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

`make check` runs the host tests and the packaging selftests (`ci/flavor.py`, `ci/mkipk.py`,
`ci/hbrepo.py`); `make selftest` runs only the selftests. Neither needs the NDK.

## Building the package

```sh
rustup toolchain install nightly --component rust-src
make setup-env                     # the webOS NDK into ~/webos-ndk (or pass WEBOS_SDK=<sdk root>)
make ipk                           # build/com.morpheus235.stremionative.debug_<version>_arm.ipk
make RELEASE=1 FLAVOR=stable ipk   # what a release publishes
make RELEASE=1 FLAVOR=beta ipk     # the StremioNative Beta app, version X.Y.<UTC date>
```

`make` links `build/stremionative` (the C shim in `native/` plus the `stremionative-app` staticlib)
and `build/stremionative-storage`. `FLAVOR` picks the install (`debug`, `beta`, `stable`; see
`ci/flavor.py`); `stable` and `beta` refuse to build without `RELEASE=1`, which also drops every dev
trigger. `ARM_PROFILE=tvdev|release` overrides the cargo profile (`tvdev`, no LTO, is the default
only for a local debug build). The NDK is published for linux-aarch64 and macOS only.

**Bundled libraries.** The package carries its own FFmpeg (`libavutil-sn.so.61`,
`libavcodec-sn.so.63`, `libavformat-sn.so.63`; plus `libswscale-sn.so.10` in a non-release build),
which the player dlopens from the app directory. `make ipk` builds it with `ci/build-ffmpeg.sh`
into `vendor/ffmpeg-prefix` (`make ffmpeg` alone; incremental after the first ~minutes);
`BUNDLED_LIB_DIR=<dir>` takes prebuilt `-sn` libraries instead. A missing required library stops
the build with its name. libass (`libass-sn.so.0`, styled ASS/SSA subtitles) is packaged when
`LIBASS_SO=<path>` (default `vendor/libass-prefix/lib/libass-sn.so.0`) exists; without it the build
warns and the player renders plain-text subtitles only.

The package is reproducible: `ci/mkipk.py` normalises every archive field, so the same inputs give
the same sha256, and `make ipk` reads the result back (`ci/mkipk.py --verify`). The launcher art in
`pkg/` (and the badged `pkg/beta/`, `pkg/debug/` sets) is drawn by `ci/make-icons.py`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE), [NOTICE](NOTICE), [docs/legal.md](docs/legal.md) and [PRIVACY.md](PRIVACY.md). Not affiliated with Stremio.
