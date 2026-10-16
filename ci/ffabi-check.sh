#!/bin/sh
# The FFmpeg ABI gate: compile ci/ffabi-assert.c (or print ci/ffabi-dump.c's table) against the
# headers the bundled libraries were built from. ff.rs reads and writes FFmpeg structs at hard-coded
# offsets; this is what makes a wrong one a compile error instead of silent memory corruption on a
# television with no debugger.
#
#   ci/ffabi-check.sh assert [--arm-eabi | --host | --cc <compiler>] [include-dir]
#   ci/ffabi-check.sh dump   [--arm-eabi | --host | --cc <compiler>] [include-dir]
#
#   --cc <compiler>  the NDK: --cc "$WEBOS_SDK/bin/arm-webos-linux-gnueabi-gcc", include-dir
#                    vendor/ffmpeg-prefix/include. THE AUTHORITY for the television's table.
#   --host           the host compiler (`cc`) against vendor/ffmpeg-prefix-host/include: the 64-bit
#                    table the desktop simulator and the host media tests run.
#   --arm-eabi       clang --target=armv7a-unknown-linux-gnueabi with ci/ffabi-libc-stub/ for libc,
#                    against vendor/ffmpeg-prefix-host/include by default: the 32-bit ARM EABI layout
#                    (4-byte pointers, 8-aligned int64) proven on a machine with no NDK. The installed
#                    headers are the same files for every target except avconfig.h, which changes no
#                    layout; see ci/ffabi-libc-stub/README.
#
# `assert` prints nothing and exits 0 when every constant holds. `dump` prints `name value`.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
mode=${1:-}
[ "$mode" = assert ] || [ "$mode" = dump ] || { echo "usage: $0 assert|dump [--arm-eabi|--host|--cc CC] [include-dir]" >&2; exit 2; }
shift
target=host
cc=cc
case "${1:-}" in
  --arm-eabi) target=arm-eabi; cc=clang; shift ;;
  --host) shift ;;
  --cc) target=cross; cc=$2; shift 2 ;;
esac
if [ "$target" = cross ]; then
  inc=${1:-$ROOT/vendor/ffmpeg-prefix/include}
else
  inc=${1:-$ROOT/vendor/ffmpeg-prefix-host/include}
fi
[ -f "$inc/libavformat/avformat.h" ] || { echo "ffabi: no FFmpeg headers in $inc — run ci/build-ffmpeg.sh (HOST=1 for the host prefix)" >&2; exit 1; }

if [ "$target" = arm-eabi ]; then
  resource=$(clang -print-resource-dir)
  set -- --target=armv7a-unknown-linux-gnueabi -nostdinc -isystem "$resource/include" -isystem "$ROOT/ci/ffabi-libc-stub"
else
  set --
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
case "$mode" in
  assert)
    "$cc" "$@" -I "$inc" -std=c11 -c "$ROOT/ci/ffabi-assert.c" -o "$tmp/assert.o"
    ;;
  dump)
    if [ "$target" = host ]; then
      "$cc" -I "$inc" -std=c11 -DSN_FFABI_MAIN "$ROOT/ci/ffabi-dump.c" -o "$tmp/dump"
      "$tmp/dump"
    else
      "$cc" "$@" -I "$inc" -std=c11 -fcommon -c "$ROOT/ci/ffabi-dump.c" -o "$tmp/dump.o"
      if [ "$target" = cross ]; then nm="${cc%gcc}nm"; else nm=llvm-nm; fi
      # Each value is the size of `sn_<name>`, stored plus one (see ci/ffabi-dump.c).
      "$nm" -S --defined-only "$tmp/dump.o" | while read -r _ size _ name; do
        case "$name" in sn_*) printf '%-28s %d\n' "${name#sn_}" $((0x$size - 1)) ;; esac
      done
    fi
    ;;
esac
