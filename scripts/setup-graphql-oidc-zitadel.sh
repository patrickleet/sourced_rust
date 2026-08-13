#!/usr/bin/env bash
# Local helper: start compose + bootstrap env for GraphQL OIDC e2e.
# Delegates to oidc-zitadel-up.sh (machinekey perms + wait + bootstrap).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/scripts/oidc-zitadel-up.sh"