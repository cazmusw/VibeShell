#!/bin/sh
set -eu
: "${MOSAIC_BIN:=target/debug}"
"$MOSAIC_BIN/mosaic-services" > /dev/null 2>&1 &
service=$!
trap 'kill "$service" 2>/dev/null || true' EXIT
# Retry the real operation until the service owns its names; no fixed startup sleep.
tries=0
until gdbus call --session --dest org.freedesktop.Notifications --object-path /org/freedesktop/Notifications --method org.freedesktop.Notifications.GetCapabilities > /dev/null 2>&1; do
 tries=$((tries+1)); test "$tries" -lt 50; sleep .1
done
reply=$(gdbus call --session --dest org.freedesktop.Notifications --object-path /org/freedesktop/Notifications --method org.freedesktop.Notifications.Notify 'Fixture' 0 '' 'Smoke test' 'Body' '[]' '{}' 5000)
case "$reply" in *'uint32 1'*) ;; *) echo "Unexpected notification response: $reply"; exit 1;; esac
snapshot=$(gdbus call --session --dest org.mosaic.Services1 --object-path /org/mosaic/Services1 --method org.mosaic.Services1.GetNotifications)
case "$snapshot" in *'Smoke test'*) ;; *) echo 'Notification missing from snapshot'; exit 1;; esac
# A second server must fail instead of displacing the first.
if "$MOSAIC_BIN/mosaic-services" >/dev/null 2>&1; then echo 'Second server unexpectedly acquired names'; exit 1; fi
printf '%s\n' 'D-Bus notification smoke: PASS'
