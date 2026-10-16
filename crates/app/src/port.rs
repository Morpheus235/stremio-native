//! The webOS port: the one table that fills the `sn_platform::tv` interfaces, and the C entry that
//! installs it.
//!
//! Everything outside this module reaches the television through `sn_platform::tv`; this is the
//! only place that names `webos`, `keymanager`, `system` and `player::ffi` together. Under
//! `cfg(test)` the sink is the platform's `NoSink`, so the host suite links no Starfish symbol.

use std::os::raw::c_int;

#[cfg(not(test))]
const SINK: &dyn sn_platform::tv::sink::VideoSink = &sn_media::player::ffi::StarfishSink;
#[cfg(test)]
const SINK: &dyn sn_platform::tv::sink::VideoSink = &sn_platform::tv::sink::NoSink;

static PORT: sn_platform::tv::Port = sn_platform::tv::Port {
    probe_device: sn_platform::webos::probe,
    start_capability_probe: sn_platform::webos::caps::start_probe,
    repair_sandbox: sn_platform::webos::jail_repair::execute,
    seal: sn_platform::keymanager::seal,
    open: sn_platform::keymanager::open,
    remove: sn_platform::keymanager::remove,
    system_locale: sn_platform::webos::system_locale,
    go_home: sn_platform::webos::go_home,
    poll_home: sn_platform::webos::poll_home,
    deliver_toast: sn_platform::webos::toast::deliver,
    bind_window: sn_platform::webos::bind_window,
    grab_surface: crate::system::sys_grab_wayland,
    release_surface: crate::system::sys_release_wayland,
    arm_opaque_region: crate::system::opaque_region_init,
    opaque_route: crate::system::opaque_route,
    clear_opaque_region: crate::system::clear_opaque_region,
    pump_bus: crate::system::ls2_pump,
    frame_probe_request: crate::system::nothing,
    frame_probe_prepare_begin: crate::system::nothing,
    frame_probe_prepare_end: crate::system::nothing,
    frame_probe_waiting: crate::system::nothing,
    frame_probe_acquired: crate::system::nothing,
    frame_probe_fields: String::new,
    sink: SINK,
};

/// **The C shim's entry** (`native/main.c`, its last call): install the port, then hand over to
/// [`crate::app::run_application`]. Returns the process exit status.
#[no_mangle]
pub extern "C" fn sn_run() -> c_int {
    let _ = sn_platform::tv::install(&PORT);
    #[cfg(target_os = "linux")]
    let _ = sn_platform::storage::client::install_activator(sn_platform::webos::activate_storage_helper);
    crate::app::run_application()
}
