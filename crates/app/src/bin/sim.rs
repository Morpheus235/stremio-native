//! **`stremionative-sim`**: the application on a desktop window (SDL2 + desktop GL), for looking at
//! UI work without the television. No video plane, no Starfish, no Luna bus: the platform's
//! `hostsim` arms stand in for them. Its instance root is `SNATIVE_RUNTIME_DIR` (triggers, event
//! log, the `memcmd` remote), so several can run at once. Frame rates here describe the desktop's
//! GPU, never the television's.

fn main() {
    let rc = stremionative_app::port::sn_run();
    std::process::exit(rc)
}
