#!/usr/bin/env bash
# Start Zitadel compose for GraphQL OIDC e2e and bootstrap env (local + CI).
#
# Pattern for other IdP adapters (Keycloak/Authentik):
#   1) Ensure writable dirs/volumes for IdP-generated secrets
#   2) docker compose up -d --wait (or health probe)
#   3) Run provider bootstrap → export GATE + OIDC_* + mint credentials
#   4) cargo test --test graphql_oidc_<provider> with GATE=1
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE_FILE="$ROOT/tests/graphql_oidc_zitadel/docker-compose.yml"
MACHINEKEY_DIR="$ROOT/tests/graphql_oidc_zitadel/machinekey"
ZITADEL_HOST="${ZITADEL_HOST:-http://localhost:8080}"

cd "$ROOT"

echo "==> Prepare machinekey dir (writable by container — FirstInstance key)"
# Zitadel runs as non-root by default; bind mounts on GHA are owned by runner
# and get "permission denied" writing zitadel-admin-sa.json unless world-writable
# (or the service runs as root — we do both for reliability).
mkdir -p "$MACHINEKEY_DIR"
chmod 777 "$MACHINEKEY_DIR"
# Clear stale keys so FirstInstance can re-create cleanly after down -v
find "$MACHINEKEY_DIR" -mindepth 1 -maxdepth 1 ! -name '.gitignore' -exec rm -rf {} + 2>/dev/null || true

echo "==> docker compose up"
docker compose -f "$COMPOSE_FILE" up -d --remove-orphans

echo "==> Wait for Zitadel process (/debug/ready is early; bootstrap waits for management)"
for i in $(seq 1 90); do
  # Prefer healthz (documented on Zitadel banner); fall back to ready.
  if curl -fsS "$ZITADEL_HOST/debug/healthz" >/dev/null 2>&1 \
    || curl -fsS "$ZITADEL_HOST/debug/ready" >/dev/null 2>&1; then
    echo "    probe ok (${i}s) — management readiness is enforced in bootstrap"
    break
  fi
  # Surface early container death (e.g. permission denied on machinekey)
  if ! docker compose -f "$COMPOSE_FILE" ps --status running --services 2>/dev/null | grep -q '^zitadel$'; then
    echo "ERROR: zitadel container not running"
    docker compose -f "$COMPOSE_FILE" ps -a || true
    docker compose -f "$COMPOSE_FILE" logs --tail=80 zitadel || true
    exit 1
  fi
  sleep 2
  if [[ $i -eq 90 ]]; then
    echo "ERROR: Zitadel never became reachable"
    docker compose -f "$COMPOSE_FILE" logs --tail=120 || true
    exit 1
  fi
done

echo "==> Wait for FirstInstance machine key"
for i in $(seq 1 60); do
  shopt -s nullglob
  keys=("$MACHINEKEY_DIR"/*.json)
  shopt -u nullglob
  if [[ ${#keys[@]} -gt 0 && -s "${keys[0]}" ]]; then
    echo "    found ${keys[0]}"
    break
  fi
  sleep 2
  if [[ $i -eq 60 ]]; then
    echo "ERROR: no machine key in $MACHINEKEY_DIR"
    ls -la "$MACHINEKEY_DIR" || true
    docker compose -f "$COMPOSE_FILE" logs --tail=80 zitadel || true
    exit 1
  fi
done

echo "==> Bootstrap OIDC app + e2e users (retries management API until not 503)"
chmod +x "$ROOT/scripts/ci-bootstrap-graphql-oidc.sh"
"$ROOT/scripts/ci-bootstrap-graphql-oidc.sh"

echo "==> Done. Source env and run tests:"
echo "    set -a && source $ROOT/graphql-oidc.env && set +a"
echo "    cargo test --test graphql_oidc_zitadel --features graphql,sqlite,metrics"
