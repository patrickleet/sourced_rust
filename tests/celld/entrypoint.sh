#!/bin/sh
# Local Azurite path. object_store's emulator client uses 127.0.0.1:10000;
# socat forwards that to the azurite compose service.
set -eu
bucket="${CELLD_BUCKET:-az://celld}"
advertise="${CELLD_ADVERTISE:-127.0.0.1:8081}"
watch="${CELLD_WATCH:-/var/lib/celld/state}"
mkdir -p "$watch"

i=0
while [ "$i" -lt 30 ]; do
  if socat /dev/null TCP:azurite:10000,connect-timeout=1 >/dev/null 2>&1; then
    break
  fi
  i=$((i + 1))
  sleep 1
done

socat TCP-LISTEN:10000,bind=127.0.0.1,fork,reuseaddr TCP:azurite:10000 &
socat_pid=$!

celld --bucket "$bucket" \
  --listen 0.0.0.0:8080 \
  --internal-listen 127.0.0.1:8081 \
  --advertise "$advertise" &
celld_pid=$!

term() {
  kill "$celld_pid" "$socat_pid" 2>/dev/null || true
  wait "$celld_pid" 2>/dev/null || true
  wait "$socat_pid" 2>/dev/null || true
}
trap term TERM INT

wait "$celld_pid"
status=$?
kill "$socat_pid" 2>/dev/null || true
exit "$status"
