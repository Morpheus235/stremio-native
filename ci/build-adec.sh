#!/bin/sh
# Build libsnadec.so, the software audio decoder (native/adec.c), against the bundled FFmpeg.
#
# The shim is compiled against the headers the shipped -sn libraries were built from
# (ci/build-ffmpeg.sh's prefix) and linked to them by SONAME, so the Rust side
# (crates/media/src/adec.rs) models no FFmpeg struct. It must need NOTHING but those -sn
# libraries and libc/libm: a firmware libavcodec in its DT_NEEDED would bind the TV's FFmpeg.
#
#   ci/build-adec.sh            # ARM: vendor/ffmpeg-prefix/lib/libsnadec.so
#   HOST=1 ci/build-adec.sh     # the desktop/test copy beside vendor/ffmpeg-prefix-host
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
HOST=${HOST-}
if [ -n "$HOST" ]; then
  PREFIX="$ROOT/vendor/ffmpeg-prefix-host"
  CC=${CC:-cc}
  if [ "$(uname -s)" = Darwin ]; then
    OUT="$PREFIX/lib/libsnadec.dylib"
    # The host dylibs' install names are /sn/lib/...; link against them, then point the shim at
    # its siblings so a test can dlopen it from the directory it stages the FFmpeg dylibs into.
    $CC -O2 -fPIC -dynamiclib -Wall -Wextra -Werror -I "$PREFIX/include" -o "$OUT" "$ROOT/native/adec.c" \
      -L"$PREFIX/lib" -lavcodec-sn -lswresample-sn -lavutil-sn -Wl,-install_name,@loader_path/libsnadec.dylib
    for dep in $(otool -L "$OUT" | awk '/\/sn\/lib\//{print $1}'); do
      install_name_tool -change "$dep" "@loader_path/$(basename "$dep")" "$OUT"
    done
    codesign -f -s - "$OUT" 2>/dev/null || true
  else
    OUT="$PREFIX/lib/libsnadec.so"
    $CC -O2 -fPIC -shared -Wall -Wextra -Werror -I "$PREFIX/include" -o "$OUT" "$ROOT/native/adec.c" \
      -L"$PREFIX/lib" -lavcodec-sn -lswresample-sn -lavutil-sn -lm -Wl,-soname,libsnadec.so -Wl,-rpath,'$ORIGIN'
  fi
  echo "adec: $OUT"
  exit 0
fi

WEBOS_SDK=${WEBOS_SDK:-$HOME/webos-ndk/arm-webos-linux-gnueabi_sdk-buildroot}
export WEBOS_SDK
SYSROOT="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot"
READELF="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-readelf"
PREFIX="$ROOT/vendor/ffmpeg-prefix"
OUT="$PREFIX/lib/libsnadec.so"
MAP="$PREFIX/lib/libsnadec.map"
echo '{ global: sn_adec_*; local: *; };' > "$MAP"
# --no-undefined: every symbol resolves against the -sn libraries at link time, not on the TV.
python3 "$ROOT/ci/arm-cc.py" --sysroot="$SYSROOT" -O2 -fPIC -shared -Wall -Wextra -Werror \
  -I "$PREFIX/include" -o "$OUT" "$ROOT/native/adec.c" \
  -L"$PREFIX/lib" -lavcodec-sn -lswresample-sn -lavutil-sn -lm \
  -Wl,-soname,libsnadec.so -Wl,-rpath,'$ORIGIN' -Wl,--no-undefined -Wl,--version-script="$MAP" -Wl,--build-id=sha1 >/dev/null

# The DT_NEEDED gate: the bundled -sn FFmpeg and libc/libm, nothing else.
needed=$("$READELF" -d "$OUT" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
for n in $needed; do
  case "$n" in
    libavcodec-sn.so.*|libswresample-sn.so.*|libavutil-sn.so.*|libc.so.6|libm.so.6) ;;
    *) echo "adec: $OUT needs $n — only the bundled -sn FFmpeg and libc/libm are allowed" >&2; exit 1 ;;
  esac
done
exports=$("$READELF" --dyn-syms --wide "$OUT" | awk '$5 ~ /GLOBAL|WEAK/ && $7 != "UND" {print $8}' | grep -v '^sn_adec_' || true)
[ -z "$exports" ] || { echo "adec: exports outside sn_adec_*: $exports" >&2; exit 1; }
echo "adec: $OUT (NEEDED: $(echo $needed))"
