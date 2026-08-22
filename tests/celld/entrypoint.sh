#!/bin/sh
# Assemble celld flags from env. Optional endpoint/region for R2/Tigris.
set -eu
bucket="${CELLD_BUCKET:?CELLD_BUCKET is required}"
advertise="${CELLD_ADVERTISE:-celld:8081}"
set -- celld --bucket "$bucket" \
  --listen 0.0.0.0:8080 \
  --internal-listen 0.0.0.0:8081 \
  --advertise "$advertise"
if [ -n "${CELLD_ENDPOINT:-}" ]; then
  set -- "$@" --endpoint "$CELLD_ENDPOINT"
fi
if [ -n "${CELLD_REGION:-}" ]; then
  set -- "$@" --region "$CELLD_REGION"
fi
exec "$@"
