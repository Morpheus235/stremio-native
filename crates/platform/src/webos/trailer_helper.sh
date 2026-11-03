# The detached helper that sends the SECOND launch of a trailer (see webos/trailer_helper.rs and
# webos/youtube.rs). Run as `/bin/sh -c <this text> trailer-helper <video id> <lock path>`: the id
# and the lock path are positional arguments, never part of this text. POSIX sh on busybox ash:
# no bash-isms, integer sleeps only. It writes no log: it runs as root in a shared directory.
#
#   1. take the one-at-a-time lock (a dir at the lock path holding "<pid> <seconds>"); the NEWEST
#      helper wins: a live, fresh holder is sent TERM and given 5 s to go, then the lock is taken
#   2. poll the foreground app every POLL s, for at most FRONT s, until it is YouTube; never: exit
#   3. wait until the LATER of SETTLE s after YouTube was in front and REPLAY s after this start
#   4. ask the foreground once more; launch the video only if YouTube is still in front
#
# All times are whole seconds of /proc/uptime (monotonic), or of `date +%s` where that file is
# unreadable. SN_TRAILER_TEST_PATH (tool search path), SN_TRAILER_TEST_UPTIME (the uptime file) and
# SN_TRAILER_POLL / _FRONT / _SETTLE / _REPLAY (seconds) exist for the host tests; the defaults
# below are the measured values.

LC_ALL=C
export LC_ALL
PATH=${SN_TRAILER_TEST_PATH:-/usr/sbin:/usr/bin:/sbin:/bin}
export PATH
# `script` runs its -c command with $SHELL
SHELL=/bin/sh
export SHELL
trap '' HUP

ID=$1
LOCK=$2
YT=youtube.leanback.v4
UPTIME=${SN_TRAILER_TEST_UPTIME:-/proc/uptime}
OWN=0

# A whole number from $1, else the default $2 (no sign, no leading zero: arithmetic stays safe).
num() {
    case $1 in
        '' | *[!0-9]* | 0?*) echo "$2" ;;
        *) echo "$1" ;;
    esac
}
POLL=$(num "$SN_TRAILER_POLL" 1)
FRONT=$(num "$SN_TRAILER_FRONT" 15)
SETTLE=$(num "$SN_TRAILER_SETTLE" 5)
REPLAY=$(num "$SN_TRAILER_REPLAY" 10)
if [ "$POLL" -lt 1 ]; then POLL=1; fi

# Whole seconds on a monotonic clock: the integer part of the first field of /proc/uptime.
now() {
    up=
    read up rest 2> /dev/null < "$UPTIME"
    up=${up%%.*}
    case $up in
        '' | *[!0-9]* | 0?*) date +%s ;;
        *) echo "$up" ;;
    esac
}

# Remove the lock only while it is still ours (the pid in it is this shell's): the helper that
# replaced us has a lock of its own by the time our exit trap runs.
release() {
    if [ "$OWN" = 1 ]; then
        owner=
        read owner rest 2> /dev/null < "$LOCK/pid"
        if [ "$owner" = "$$" ]; then rm -rf "$LOCK"; fi
    fi
}
trap release EXIT

# Sleep $1 seconds in a way TERM interrupts at once: a foreground `sleep` defers the trap until it
# ends, and a newer helper only waits 5 s for this one to go.
SLEEP_PID=
nap() {
    sleep "$1" &
    SLEEP_PID=$!
    wait "$SLEEP_PID"
    SLEEP_PID=
}

# INT or TERM: take a running nap with us (no orphaned sleep), then exit (the EXIT trap releases).
stop() {
    if [ -n "$SLEEP_PID" ]; then kill "$SLEEP_PID" 2> /dev/null; fi
    exit 1
}
trap stop INT TERM

# The id was checked by the app; the charset below is what makes the payload safe to build from it.
case $ID in
    '' | *[!A-Za-z0-9_-]*)
        exit 2
        ;;
esac
if [ "${#ID}" -lt 6 ] || [ "${#ID}" -gt 20 ]; then
    exit 2
fi

# True when the lock stamp ($1, seconds) is a number no newer than now and younger than two
# minutes. A missing or odd stamp is NOT fresh: only a stamp proves the holder is recent, and a
# reused pid cannot hold a lock for good.
fresh() {
    case $1 in
        '' | *[!0-9]* | 0?*) return 1 ;;
    esac
    at=$(now)
    [ "$1" -le "$at" ] && [ $((at - $1)) -lt 120 ]
}

# True when pid $1 is a trailer helper (its command line carries the name `trailer-helper`), so a
# reused pid never gets a TERM. Without /proc (the host tests) nothing can be told: yes.
is_helper() {
    [ -d /proc/self ] || return 0
    case $(tr '\000' ' ' 2> /dev/null < "/proc/$1/cmdline") in
        *trailer-helper*) return 0 ;;
    esac
    return 1
}

# The runtime directory is world-writable and this runs as root: never use a symlink someone left
# at the lock's path.
take_lock() {
    if [ -L "$LOCK" ]; then rm -f "$LOCK"; fi
    if ! mkdir "$LOCK" 2> /dev/null; then
        holder=
        stamp=
        read holder stamp 2> /dev/null < "$LOCK/pid"
        if [ -z "$holder" ]; then
            # the owner has made the dir and not yet written its pid
            nap 1
            read holder stamp 2> /dev/null < "$LOCK/pid"
        fi
        case $holder in
            '' | *[!0-9]*) ;;
            *)
                if [ "$holder" != "$$" ] && kill -0 "$holder" 2> /dev/null && fresh "$stamp" && is_helper "$holder"; then
                    # the newest helper wins: the viewer pressed the button again. 5 s, because a
                    # TERM that lands during a luna call waits for the call (3 s, plus script).
                    kill -TERM "$holder" 2> /dev/null
                    waited=0
                    while kill -0 "$holder" 2> /dev/null && [ "$waited" -lt 5 ]; do
                        nap 1
                        waited=$((waited + 1))
                    done
                    if kill -0 "$holder" 2> /dev/null; then return 1; fi
                fi
                ;;
        esac
        # a dead, old, unstamped or replaced owner: take the lock over
        rm -rf "$LOCK"
        mkdir "$LOCK" 2> /dev/null || return 1
    fi
    OWN=1
    echo "$$ $(now)" > "$LOCK/pid"
    return 0
}

if ! take_lock; then
    exit 0
fi

START=$(now)

# One luna-send call: its reply on one line. luna-send prints the reply only on a tty, hence script.
luna() {
    script -q -c "luna-send -n 1 -w 3000 $1 '$2'" /dev/null 2> /dev/null < /dev/null | tr -d '\r' | tr '\n' ' '
}

foreground() {
    luna luna://com.webos.applicationManager/getForegroundAppInfo '{}'
}

reply=
found=0
while :; do
    reply=$(foreground)
    case $reply in
        *\"$YT\"*)
            found=1
            break
            ;;
    esac
    if [ $(($(now) - START)) -ge "$FRONT" ]; then break; fi
    nap "$POLL"
done
if [ "$found" != 1 ]; then
    exit 0
fi
SEEN=$(now)

# the LATER of SETTLE s after it was in front and REPLAY s after the start
DUE=$((SEEN + SETTLE))
if [ $((START + REPLAY)) -gt "$DUE" ]; then DUE=$((START + REPLAY)); fi
WAIT=$((DUE - $(now)))
if [ "$WAIT" -gt 0 ]; then nap "$WAIT"; fi

# the viewer may have left YouTube meanwhile: a launch now would pull them back into it
reply=$(foreground)
case $reply in
    *\"$YT\"*) ;;
    *)
        exit 0
        ;;
esac
luna luna://com.webos.applicationManager/launch "{\"id\":\"$YT\",\"params\":{\"handledBy\":\"app\",\"contentTarget\":\"v=$ID\"}}" > /dev/null
exit 0
