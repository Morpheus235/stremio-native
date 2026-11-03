#!/bin/sh
# The desktop simulator: build it, run it, and drive it from a script or another terminal.
#
#   tools/sim.sh run              build (optimised) and open the app in a window, signed out
#   tools/sim.sh key up up ok     one remote key per step: up down left right ok back
#   tools/sim.sh text dune        type a word into the focused text field
#   tools/sim.sh shot             save the next frame as a PNG and print its path
#
# SIM_DIR      the instance root: settings, event log, screenshots (default /tmp/stremionative-sim)
# SIM_PROFILE  cargo profile: tvdev (default, optimised, no LTO) or dev (faster to build, ~3x
#              slower to draw)
# SIM_STEP     seconds to let the UI settle after each command (default 0.8)
set -eu

cd "$(dirname "$0")/.."
SIM_DIR=${SIM_DIR:-/tmp/stremionative-sim}
SIM_PROFILE=${SIM_PROFILE:-tvdev}
SIM_STEP=${SIM_STEP:-0.8}
CMD="$SIM_DIR/stremionative-memcmd"
export PATH="$HOME/.cargo/bin:$PATH"

# Hand one command to the running app (it polls the file twice a second and removes it), then
# wait for the UI to settle, so focus moves are not lost in an animation.
send() {
    pgrep -f stremionative-sim >/dev/null || { echo "sim.sh: the simulator is not running (tools/sim.sh run)" >&2; exit 1; }
    printf '%s\n' "$1" > "$CMD"
    i=0
    while [ -e "$CMD" ]; do
        i=$((i + 1))
        [ $i -le 50 ] || { echo "sim.sh: the app did not read '$1'" >&2; exit 1; }
        sleep 0.1
    done
    sleep "$SIM_STEP"
}

case "${1:-}" in
run)
    CARGO_INCREMENTAL=0 cargo build -p stremionative-app --features hostsim --bin stremionative-sim --profile "$SIM_PROFILE"
    dir=$SIM_PROFILE
    [ "$dir" = dev ] && dir=debug
    mkdir -p "$SIM_DIR" && touch "$SIM_DIR/stremionative-guest"
    SNATIVE_APP_DIR="$PWD/pkg" SNATIVE_RUNTIME_DIR="$SIM_DIR" exec "target/$dir/stremionative-sim"
    ;;
key)
    shift
    for k in "$@"; do send "$k"; done
    ;;
text)
    [ $# -eq 2 ] || { echo "usage: tools/sim.sh text <word>" >&2; exit 2; }
    send "text=$2"
    ;;
shot)
    # a new run numbers its shots from 1 again, so look for a file newer than the request
    mark="$SIM_DIR/.shot-mark"
    touch "$mark"
    sleep 1
    send shot
    i=0
    while [ -z "$(find "$SIM_DIR" -maxdepth 1 -name 'shot-*.png' -newer "$mark")" ]; do
        i=$((i + 1))
        [ $i -le 50 ] || { echo "sim.sh: no screenshot appeared" >&2; exit 1; }
        sleep 0.1
    done
    find "$SIM_DIR" -maxdepth 1 -name 'shot-*.png' -newer "$mark"
    ;;
*)
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
