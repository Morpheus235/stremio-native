#!/bin/sh
# Stage the HOST FFmpeg (`HOST=1 ci/build-ffmpeg.sh`) where the host side of the app loads it.
#
# `ff.rs` dlopens the bundled libraries by ABSOLUTE PATH out of `paths::app_dir()` (the media
# tests: `$SN_FFMPEG_DIR`, else `pkg/`), in dependency order with RTLD_GLOBAL — libavutil,
# libavcodec, libavformat — so each library finds the one before it by SONAME.
#
#   ci/stage-host-ffmpeg.sh                     # Linux: print the SN_FFMPEG_DIR to export
#   ci/stage-host-ffmpeg.sh libavformat-sn.63   # macOS: stage one dylib into pkg/
#
# LINUX: the host libraries are ELF under exactly the SONAMEs the television's copies carry
# (`libavformat-sn.so.63`), so they must NEVER be copied into `pkg/`, where the ARM ones live — a
# package built after that would ship x86_64 code. They are loaded where the build installed them:
# this prints that directory, and the media tests read it from `SN_FFMPEG_DIR`. glibc's loader
# resolves the libraries' DT_NEEDED by SONAME against what is already loaded, so no install-name
# rewrite is needed.
#
# MACOS (PlxNative's original purpose for this script): FFmpeg's configure records `--prefix` in
# each dylib's install name, and that prefix is the literal `/sn` (see ci/build-ffmpeg.sh — a
# stand-in chosen so no build path leaks into the shipped libraries). `/sn/lib/libavutil-sn.61.dylib`
# does not exist, so dyld fails to resolve libavcodec's reference to libavutil; dyld resolves by
# install name rather than through a global namespace, so the names are rewritten to
# `@loader_path` and the dylib staged beside pkg/'s payload.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX="$ROOT/vendor/ffmpeg-prefix-host"

if [ "$(uname -s)" != Darwin ]; then
  for lib in libavutil-sn.so.61 libavcodec-sn.so.63 libavformat-sn.so.63; do
    [ -e "$PREFIX/lib/$lib" ] || { echo "no host $lib in $PREFIX/lib — run 'HOST=1 ci/build-ffmpeg.sh'" >&2; exit 1; }
  done
  echo "SN_FFMPEG_DIR=$PREFIX/lib"
  exit 0
fi

LIBS='libavutil-sn.61 libavcodec-sn.63 libavformat-sn.63 libswscale-sn.10'
name=$1
src=$(ls "$PREFIX/lib/$name".*.*.dylib 2>/dev/null | head -1)
[ -n "$src" ] || { echo "no host $name in $PREFIX — run 'HOST=1 ci/build-ffmpeg.sh'" >&2; exit 1; }
out="$ROOT/pkg/$name.dylib"
cp "$src" "$out"
chmod u+w "$out"
install_name_tool -id "@loader_path/$name.dylib" "$out"
for dep in $LIBS; do
  install_name_tool -change "/sn/lib/$dep.dylib" "@loader_path/$dep.dylib" "$out"
done
# Rewriting load commands invalidates the ad-hoc signature Apple silicon requires; without this
# the dlopen fails with "code signature invalid" and nothing says which of the two edits did it.
codesign -f -s - "$out" 2>/dev/null || true
