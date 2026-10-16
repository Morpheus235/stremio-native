//! Host link configuration for the screens layer's own test binary, and nothing else.
//!
//! The screens draw through `sn_ui` and `sn_gfx`, whose `extern "C"` GL, SDL2_ttf and nanosvg
//! declarations this crate's `--test` binary reaches; a `cargo:rustc-link-arg` line (the nanosvg
//! object) reaches only the package that prints it, so the lines come from the same shared file
//! (`../build_support/host_link.rs`) every other drawing crate uses. On the television this prints
//! nothing: the ARM build is a staticlib the Makefile links.

#[path = "../build_support/host_link.rs"]
mod host_link;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../build_support/host_link.rs");
    host_link::emit(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
}
