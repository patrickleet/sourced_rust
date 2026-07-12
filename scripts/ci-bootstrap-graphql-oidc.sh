#!/usr/bin/env bash
# Bootstrap live Zitadel for GraphQL OIDC e2e (D11 JWT-bearer mint).
# Expects Zitadel from tests/graphql_oidc_zitadel/docker-compose.yml.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE_DIR="$ROOT/tests/graphql_oidc_zitadel"
MACHINEKEY_DIR="$COMPOSE_DIR/machinekey"
ZITADEL_HOST="${ZITADEL_HOST:-http://localhost:8080}"
OUT="${GRAPHQL_OIDC_ENV:-$ROOT/graphql-oidc.env}"
PROJECT_NAME="${GRAPHQL_OIDC_PROJECT:-distributed-graphql}"
APP_NAME="${GRAPHQL_OIDC_APP:-graphql-api}"

need() { command -v "$1" >/dev/null 2>&1 || { echo "ERROR: $1 required"; exit 1; }; }
need jq
need curl
need openssl

b64url() { openssl base64 -e -A | tr '+/' '-_' | tr -d '='; }

echo "==> Waiting for Zitadel at $ZITADEL_HOST ..."
for i in $(seq 1 90); do
  if curl -fsS "$ZITADEL_HOST/debug/ready" >/dev/null 2>&1; then
    echo "    ready"
    break
  fi
  sleep 2
  if [[ $i -eq 90 ]]; then
    echo "ERROR: Zitadel never ready"
    exit 1
  fi
done

# Wait for FirstInstance machine key
KEYFILE=""
for i in $(seq 1 60); do
  shopt -s nullglob
  keys=("$MACHINEKEY_DIR"/*.json)
  shopt -u nullglob
  if [[ ${#keys[@]} -gt 0 ]]; then
    KEYFILE="${keys[0]}"
    break
  fi
  sleep 2
done
if [[ -z "$KEYFILE" || ! -s "$KEYFILE" ]]; then
  echo "ERROR: no machine key in $MACHINEKEY_DIR (FirstInstance steps)"
  exit 1
fi
echo "==> Using machine key: $KEYFILE"

USER_ID=$(jq -r .userId "$KEYFILE")
KEY_ID=$(jq -r .keyId "$KEYFILE")
# Zitadel machine keys store PEM under .key
KEY_PEM=$(jq -r .key "$KEYFILE")
if [[ -z "$USER_ID" || "$USER_ID" == "null" || -z "$KEY_PEM" || "$KEY_PEM" == "null" ]]; then
  echo "ERROR: invalid machine key JSON"
  exit 1
fi

# Sign JWT-bearer assertion (same pattern as website setup-local-zitadel.sh)
NOW=$(date +%s)
EXP=$((NOW + 60))
HEADER=$(printf '{"alg":"RS256","typ":"JWT","kid":"%s"}' "$KEY_ID" | b64url)
PAYLOAD=$(printf '{"iss":"%s","sub":"%s","aud":"%s","iat":%s,"exp":%s}' \
  "$USER_ID" "$USER_ID" "$ZITADEL_HOST" "$NOW" "$EXP" | b64url)
SIGNING_INPUT="${HEADER}.${PAYLOAD}"
TMPKEY=$(mktemp)
trap 'rm -f "$TMPKEY"' EXIT
printf '%s\n' "$KEY_PEM" > "$TMPKEY"
SIGNATURE=$(printf '%s' "$SIGNING_INPUT" | openssl dgst -sha256 -sign "$TMPKEY" | b64url)
JWT="${SIGNING_INPUT}.${SIGNATURE}"

echo "==> Exchanging admin JWT for access token..."
TOKEN_RESPONSE=$(curl -fsS -X POST "$ZITADEL_HOST/oauth/v2/token" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode "grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer" \
  --data-urlencode "scope=openid urn:zitadel:iam:org:project:id:zitadel:aud" \
  --data-urlencode "assertion=$JWT")
ACCESS_TOKEN=$(echo "$TOKEN_RESPONSE" | jq -r .access_token)
if [[ -z "$ACCESS_TOKEN" || "$ACCESS_TOKEN" == "null" ]]; then
  echo "ERROR: token exchange failed"; echo "$TOKEN_RESPONSE" | jq .; exit 1
fi

api() {
  local method="$1" path="$2" body="${3:-}"
  if [[ -n "$body" ]]; then
    curl -fsS -X "$method" "$ZITADEL_HOST$path" \
      -H "Authorization: Bearer $ACCESS_TOKEN" \
      -H 'Content-Type: application/json' \
      -d "$body"
  else
    curl -fsS -X "$method" "$ZITADEL_HOST$path" \
      -H "Authorization: Bearer $ACCESS_TOKEN"
  fi
}

echo "==> Ensure project $PROJECT_NAME"
PROJECT_SEARCH=$(api POST /management/v1/projects/_search '{}')
PROJECT_ID=$(echo "$PROJECT_SEARCH" | jq -r --arg n "$PROJECT_NAME" \
  '.result[]? | select(.name == $n) | .id' | head -n1)
if [[ -z "$PROJECT_ID" ]]; then
  PROJECT_ID=$(api POST /management/v1/projects "$(jq -n --arg n "$PROJECT_NAME" '{name: $n}')" | jq -r .id)
fi
echo "    project=$PROJECT_ID"

# Project roles admin + customer
for role in admin customer; do
  api POST "/management/v1/projects/$PROJECT_ID/roles" \
    "$(jq -n --arg k "$role" --arg d "$role" '{key: $k, displayName: $d}')" >/dev/null 2>&1 || true
done

echo "==> Ensure OIDC app $APP_NAME (JWT access tokens + role assertion)"
APP_SEARCH=$(api POST "/management/v1/projects/$PROJECT_ID/apps/_search" '{}')
APP_ID=$(echo "$APP_SEARCH" | jq -r --arg n "$APP_NAME" \
  '.result[]? | select(.name == $n) | .id' | head -n1)
CLIENT_ID=""
if [[ -z "$APP_ID" ]]; then
  APP_RESP=$(api POST "/management/v1/projects/$PROJECT_ID/apps/oidc" "$(jq -n \
    --arg name "$APP_NAME" \
    '{
      name: $name,
      redirectUris: ["http://localhost/callback"],
      responseTypes: ["OIDC_RESPONSE_TYPE_CODE"],
      grantTypes: ["OIDC_GRANT_TYPE_AUTHORIZATION_CODE", "OIDC_GRANT_TYPE_REFRESH_TOKEN"],
      appType: "OIDC_APP_TYPE_WEB",
      authMethodType: "OIDC_AUTH_METHOD_TYPE_BASIC",
      postLogoutRedirectUris: ["http://localhost/"],
      version: "OIDC_VERSION_1_0",
      devMode: true,
      accessTokenType: "OIDC_TOKEN_TYPE_JWT",
      accessTokenRoleAssertion: true,
      idTokenRoleAssertion: true,
      idTokenUserinfoAssertion: true
    }')")
  CLIENT_ID=$(echo "$APP_RESP" | jq -r .clientId)
  APP_ID=$(echo "$APP_RESP" | jq -r .appId // .id // empty)
else
  # Existing app: client id from search result
  CLIENT_ID=$(echo "$APP_SEARCH" | jq -r --arg n "$APP_NAME" \
    '.result[]? | select(.name == $n) | .oidcConfig.clientId // empty' | head -n1)
fi
if [[ -z "$CLIENT_ID" || "$CLIENT_ID" == "null" ]]; then
  echo "ERROR: could not resolve OIDC client id"
  exit 1
fi
echo "    client_id=$CLIENT_ID"

create_machine_with_key() {
  local username="$1" role="$2" key_out="$3"
  local search uid key_resp

  search=$(api POST /management/v1/users/_search "$(jq -n --arg n "$username" \
    '{queries: [{userNameQuery: {userName: $n, method: "TEXT_QUERY_METHOD_EQUALS"}}]}')")
  uid=$(echo "$search" | jq -r '.result[0].id // empty')
  if [[ -z "$uid" ]]; then
    echo "==> Creating machine user $username"
    uid=$(api POST /management/v1/users/machine "$(jq -n --arg u "$username" \
      '{userName: $u, name: $u, description: "GraphQL e2e", accessTokenType: "ACCESS_TOKEN_TYPE_JWT"}')" \
      | jq -r .userId)
  fi
  if [[ -z "$uid" || "$uid" == "null" ]]; then
    echo "ERROR: machine user $username"; exit 1
  fi

  # Grant project role
  api POST "/management/v1/users/$uid/grants" "$(jq -n \
    --arg pid "$PROJECT_ID" --arg r "$role" \
    '{projectId: $pid, roleKeys: [$r]}')" >/dev/null 2>&1 || true

  # Machine key (JSON type 1) for JWT-bearer
  key_resp=$(api POST "/management/v1/users/$uid/keys" \
    "$(jq -n '{type: "KEY_TYPE_JSON", expirationDate: "2029-01-01T00:00:00Z"}')")
  # Response may embed keyDetails as base64 or return key object
  local key_json
  if echo "$key_resp" | jq -e '.keyDetails' >/dev/null 2>&1; then
    key_json=$(echo "$key_resp" | jq -r .keyDetails | base64 -d 2>/dev/null || echo "$key_resp" | jq -r .keyDetails)
    # If still not JSON, try raw
    if ! echo "$key_json" | jq -e . >/dev/null 2>&1; then
      # keyDetails might already be object when re-encoded
      key_json=$(echo "$key_resp" | jq -c '{keyId: .keyId, key: .key, userId: .userId}')
    fi
  else
    key_json=$(echo "$key_resp" | jq -c .)
  fi

  # Prefer structured fields
  if echo "$key_resp" | jq -e '.keyId and .key' >/dev/null 2>&1; then
    echo "$key_resp" | jq -c --arg uid "$uid" '{keyId: .keyId, key: .key, userId: $uid}' > "$key_out"
  elif echo "$key_json" | jq -e . >/dev/null 2>&1; then
    echo "$key_json" | jq -c --arg uid "$uid" '. + {userId: (.userId // $uid)}' > "$key_out"
  else
    echo "ERROR: unexpected key response for $username"
    echo "$key_resp" | jq . || echo "$key_resp"
    exit 1
  fi

  # Ensure userId present
  if [[ "$(jq -r .userId "$key_out")" == "null" || -z "$(jq -r .userId "$key_out")" ]]; then
    jq --arg uid "$uid" '.userId = $uid' "$key_out" > "${key_out}.tmp" && mv "${key_out}.tmp" "$key_out"
  fi
  echo "$uid"
}

mkdir -p "$MACHINEKEY_DIR/e2e"
CUSTOMER_KEY="$MACHINEKEY_DIR/e2e/customer.json"
ADMIN_KEY="$MACHINEKEY_DIR/e2e/admin.json"
CUSTOMER_UID=$(create_machine_with_key "graphql-e2e-customer" "customer" "$CUSTOMER_KEY")
ADMIN_UID=$(create_machine_with_key "graphql-e2e-admin" "admin" "$ADMIN_KEY")

umask 077
cat > "$OUT" <<EOF
ZITADEL_E2E=1
OIDC_ISSUER=$ZITADEL_HOST
OIDC_AUDIENCE=$CLIENT_ID
OIDC_CLIENT_ID=$CLIENT_ID
GRAPHQL_E2E_CUSTOMER_KEY=$CUSTOMER_KEY
GRAPHQL_E2E_ADMIN_KEY=$ADMIN_KEY
GRAPHQL_E2E_CUSTOMER_USER_ID=$CUSTOMER_UID
GRAPHQL_E2E_ADMIN_USER_ID=$ADMIN_UID
EOF
echo "==> Wrote $OUT"
cat "$OUT"
