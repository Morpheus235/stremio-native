//! The application crate's build script: the host link configuration for everything cargo links
//! on the developer's machine (this crate's test binaries), and the version the binary reports.
//!
//! **The version** is `SN_VERSION`, compile-time, read by `app::boot` for the `User-Agent`
//! (`StremioNative/<version>`) and the boot preamble. The Makefile hands it in for a release
//! build — the PACKAGE's version, computed by `ci/flavor.py` (`X.Y.Z` stable, `X.Y.YYMMDDHHm` beta)
//! — so the binary and its package cannot disagree. Every other build reports
//! `pkg/appinfo.json`'s version with a `-dev` suffix, so a working tree never claims to be the
//! shipped artifact.
//!
//! On the television this prints no link line: the ARM build is a staticlib the Makefile links,
//! and `host_link::emit` returns before emitting anything for `target_arch = "arm"`.

#[path = "../build_support/host_link.rs"]
mod host_link;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../build_support/host_link.rs");
    println!("cargo:rerun-if-env-changed=SN_VERSION");
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let appinfo = repo.join("pkg/appinfo.json");
    println!("cargo:rerun-if-changed={}", appinfo.display());
    let version = match std::env::var("SN_VERSION") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => {
            let text = std::fs::read_to_string(&appinfo).expect("read pkg/appinfo.json");
            let json: serde_json::Value = serde_json::from_str(&text).expect("pkg/appinfo.json is JSON");
            let v = json["version"].as_str().expect("pkg/appinfo.json has a version").to_string();
            format!("{v}-dev")
        }
    };
    assert!(
        !version.is_empty() && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')),
        "SN_VERSION {version:?} is not a version string"
    );
    println!("cargo:rustc-env=SN_VERSION={version}");
    host_link::emit(&repo);
}
