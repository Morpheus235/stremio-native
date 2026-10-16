//! **stremionative-app** — the application core the C boot shim (`native/main.c`) links: boot,
//! the frame loop over the `sn_ui` dispatcher, input, the app lifecycle, the webOS port, the
//! addon transport over `sn_net`, the artwork source and playback orchestration.
//!
//! The ARM build is a staticlib (`make` asks `cargo rustc --crate-type staticlib` for it); the
//! C entry points it exports are:
//!
//! | symbol | caller | defined in |
//! |---|---|---|
//! | `sn_run()` | `native/main.c`'s `main`, last | [`port`] |
//! | `sn_crash_write_image_marker(fd)` | `main.c`, before the tracer is armed | [`crash`] |
//! | `sn_runtime_path(name, out, cap)` | `main.c`, for its three log sinks | `sn_base::paths` |
//! | `sf_on_event` / `acb_on_event` | `native/starfish.c`'s callbacks | `sn_media` |
//!
//! Layering: this crate is the top of the stack and may name every crate below it. It is the only
//! place that names `sn_platform::webos`, the keymanager and `sn_media::player::ffi` together
//! ([`port`]), and the only crate that touches SDL's event queue and window ([`app`]).

pub mod app;
pub mod art;
pub mod crash;
pub mod fetch;
pub mod port;
pub mod system;
pub mod textinput;
pub mod work;

/// The version this binary reports (`User-Agent`, the boot preamble): the package version for a
/// release build, `pkg/appinfo.json`'s with `-dev` otherwise (`build.rs`).
pub const VERSION: &str = env!("SN_VERSION");

/// The `User-Agent` every request this application makes carries.
pub fn user_agent() -> String {
    format!("StremioNative/{VERSION}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_user_agent_names_the_product_and_its_version() {
        let ua = super::user_agent();
        assert!(ua.starts_with("StremioNative/"), "{ua}");
        assert!(ua.len() > "StremioNative/".len(), "{ua}");
        assert!(!ua.contains(char::is_whitespace), "one product token: {ua}");
    }
}
