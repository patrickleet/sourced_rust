#!/usr/bin/env bash
# Bootstrap GraphQL OIDC e2e env against sites/the-website Zitadel stack (D11).
# Primary mint: machine-user JWT-bearer. Exports env for tests/graphql_oidc_zitadel.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WEBSITE="${ROOT}/sites/the-website"
ZITADEL_HOST="${ZITADEL_HOST:-http://localhost:8080}"
OUT="${GRAPHQL_OIDC_ENV:-$(cd "$(dirname "$0")/.." && pwd)/graphql-oidc.env}"

echo "==> Ensure Zitadel is up (make -C sites/the-website zitadel-up / auth-up)"
if ! curl -fsS "${ZITADEL_HOST}/debug/ready" >/dev/null 2>&1; then
  echo "ERROR: Zitadel not ready at ${ZITADEL_HOST}. Start compose first."
  exit 1
fi

if [[ -x "${WEBSITE}/scripts/setup-local-zitadel.sh" ]]; then
  echo "==> Running website Zitadel bootstrap (mgmt JWT-bearer pattern)"
  (cd "${WEBSITE}" && ./scripts/setup-local-zitadel.sh) || true
fi

echo "==> Writing ${OUT}"
# Operators fill machine key paths after creating graphql-e2e-* machine users.
cat > "${OUT}" <<ENV
ZITADEL_E2E=1
OIDC_ISSUER=${ZITADEL_HOST}
OIDC_AUDIENCE=\${OIDC_AUDIENCE:-set-me}
OIDC_CLIENT_ID=\${OIDC_CLIENT_ID:-set-me}
GRAPHQL_E2E_CUSTOMER_KEY=/path/to/customer-machine-key.json
GRAPHQL_E2E_ADMIN_KEY=/path/to/admin-machine-key.json
GRAPHQL_E2E_CUSTOMER_USER_ID=set-customer-user-id
GRAPHQL_E2E_ADMIN_USER_ID=set-admin-user-id
ENV
echo "Edit ${OUT} with machine keys from setup (JWT-bearer mint only)."
echo "Then: set -a && source ${OUT} && set +a && cargo test --features graphql,sqlite --test graphql_oidc_zitadel"
