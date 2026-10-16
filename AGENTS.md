# Project instructions

StremioNative is a native Stremio client for rooted LG webOS TVs (32-bit ARM, hardware video
plane, StarfishMediaAPIs). Rust layer crates live under `crates/`; `native/` holds the C boot/crash
shim, the Starfish/ACB seam and the libass/nanosvg glue. `docs/roadmap.md` is the plan.

## Layers
`base` → `machine` → `net`, `platform` → `gfx` → `ui`; `media` (demux, curl media source,
subtitles, the Starfish buffer-feed player) sits on base/machine/platform/net; `stremio` (the
addon protocol) depends on nothing above serde. `screens` (the screens and their pure stores)
sits on `ui` and `stremio`; `app` (the staticlib `native/main.c` links: boot, the loop, the webOS
port, the workers) is the top and may name everything. References point down only.

## Build and package
- `make` (needs the webOS NDK, `make setup-env`) links `build/stremionative`; `make ipk` writes
  `build/<app id>_<version>_arm.ipk` for `FLAVOR=debug|beta|stable` (stable and beta need
  `RELEASE=1`). `make check` / `make selftest` need no NDK. README.md, "Building the package".

## Working rules
- Host checks: `CARGO_INCREMENTAL=0 cargo test --workspace --no-fail-fast` (needs `libsdl2-dev
  libsdl2-ttf-dev libgl-dev`). Warnings are denied.
- Behind an HTTPS proxy, `net::roots_tests` needs its test hostnames excluded:
  `NO_PROXY="$NO_PROXY,.invalid"`.
- Fixing a reported bug starts with a test that fails against the broken behaviour.
- webOS native behaviour is under-documented: verify it against the device's own binaries or
  primary documentation, never infer it from a symbol name.
- Never commit TV addresses, MACs, Stremio auth keys or addon URLs carrying user configuration.
- The repository is GPL-3.0-or-later and derived from PlxNative; keep `LICENSE` and `NOTICE`.
