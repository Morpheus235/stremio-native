#!/bin/sh
# Replace an installed StremioNative's app files (binary, bundled libraries, fonts) on a rooted TV
# and bring the app back to the front.
#
#   SN_TV=root@<tv address> tools/deploy-tv.sh [FLAVOR]      (default FLAVOR=beta)
#
# Builds nothing: run `make RELEASE=1 FLAVOR=<flavor> ipk` first; this takes the staged app folder.
# The app is closed through the application manager, never killed: a killed app leaves the TV on
# its last input (HDMI). After the launch the foreground app is checked and relaunched once.
set -eu
FLAVOR=${1:-beta}
: "${SN_TV:?set SN_TV=root@<tv address>}"
ID=$(make -s print-appid FLAVOR="$FLAVOR" RELEASE=1)
STAGE=build/stage/$FLAVOR/app
[ -f "$STAGE/stremionative" ] || { echo "no $STAGE/stremionative — build it first"; exit 1; }
DIR=/media/developer/apps/usr/palm/applications/$ID
tar -C "$STAGE" -cf - . | ssh "$SN_TV" "cat > /tmp/sn-deploy.tar"
# luna-send needs a tty or a backgrounded call on webOS; `script -qc` gives it one.
ssh "$SN_TV" "set -e
  luna() { script -qc \"luna-send -n 1 \$1 '\$2'\" /dev/null; }
  luna luna://com.webos.applicationManager/closeByAppId '{\"id\":\"$ID\"}' >/dev/null || true
  sleep 2
  tar -xf /tmp/sn-deploy.tar -C $DIR && rm -f /tmp/sn-deploy.tar && chmod 755 $DIR/stremionative
  for attempt in 1 2; do
    luna luna://com.webos.applicationManager/launch '{\"id\":\"$ID\"}' >/dev/null
    sleep 4
    if luna luna://com.webos.applicationManager/getForegroundAppInfo '{}' | grep -q '\"$ID\"'; then
      echo \"$ID is in the foreground\"; exit 0
    fi
  done
  echo \"$ID did not come to the foreground\"; exit 1"
