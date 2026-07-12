#!/usr/bin/env bash
# Local helper: start compose + bootstrap env for GraphQL OIDC e2e.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p tests/graphql_oidc_zitadel/machinekey
docker compose -f tests/graphql_oidc_zitadel/docker-compose.yml up -d
exec "$ROOT/scripts/ci-bootstrap-graphql-oidc.sh"
