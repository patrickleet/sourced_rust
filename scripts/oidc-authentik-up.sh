#!/usr/bin/env bash
# Start Authentik + bootstrap two M2M OAuth2 clients for GraphQL e2e (local + CI).
# Pattern: compose up → wait healthy → docker exec bootstrap → write env.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE="$ROOT/tests/graphql_oidc_authentik/docker-compose.yml"
OUT="${GRAPHQL_OIDC_AUTHENTIK_ENV:-$ROOT/graphql-oidc-authentik.env}"
BASE="${AUTHENTIK_URL:-http://localhost:9000}"
AK_EMAIL="${AUTHENTIK_BOOTSTRAP_EMAIL:-akadmin@localhost}"
AK_PASS="${AUTHENTIK_BOOTSTRAP_PASSWORD:-akadmin-e2e-pass}"

export COMPOSE AUTHENTIK_URL="$BASE" AUTHENTIK_BOOTSTRAP_EMAIL="$AK_EMAIL" \
  AUTHENTIK_BOOTSTRAP_PASSWORD="$AK_PASS" GRAPHQL_OIDC_AUTHENTIK_ENV="$OUT"

cd "$ROOT"
echo "==> docker compose up Authentik"
docker compose -f "$COMPOSE" up -d --remove-orphans

echo "==> Wait for Authentik live at $BASE"
for i in $(seq 1 120); do
  if curl -fsS "$BASE/-/health/live/" >/dev/null 2>&1 \
    || curl -fsS "$BASE/api/v3/root/" >/dev/null 2>&1; then
    echo "    live (${i}*3s)"
    break
  fi
  sleep 3
  if [[ $i -eq 120 ]]; then
    echo "ERROR: Authentik never ready"
    docker compose -f "$COMPOSE" logs --tail=120 server worker || true
    exit 1
  fi
done

# Migrations / bootstrap user settle
echo "==> Wait for worker + bootstrap user"
sleep 15
for i in $(seq 1 40); do
  if docker compose -f "$COMPOSE" exec -T worker ak version >/dev/null 2>&1; then
    echo "    worker ak ready (${i})"
    break
  fi
  sleep 3
  if [[ $i -eq 40 ]]; then
    echo "ERROR: worker never ready for ak"
    docker compose -f "$COMPOSE" logs --tail=80 worker || true
    exit 1
  fi
done

echo "==> Bootstrap OAuth2 M2M clients via ak shell"
# Create API token + two confidential OAuth2 providers/apps with client_credentials.
# Prints KEY=value lines on stdout for the outer shell to capture.
BOOT_OUT=$(docker compose -f "$COMPOSE" exec -T worker ak shell -c '
from authentik.core.models import Token, TokenIntents, User, Application, Group
from authentik.providers.oauth2.models import (
    OAuth2Provider, ClientTypes, SubModes, IssuerMode, ScopeMapping,
)
from authentik.crypto.models import CertificateKeyPair
from authentik.flows.models import Flow
from django.db import transaction

u = User.objects.filter(username="akadmin").first()
if u is None:
    u = User.objects.filter(is_superuser=True).first()
assert u is not None, "no admin user yet"

tok, _ = Token.objects.update_or_create(
    identifier="graphql-e2e-api",
    defaults={"user": u, "intent": TokenIntents.INTENT_API, "expiring": False},
)

auth_flow = Flow.objects.filter(slug="default-provider-authorization-implicit-consent").first()
if auth_flow is None:
    auth_flow = Flow.objects.filter(designation="authorization").first()
assert auth_flow is not None, "no authorization flow"

invalidation = Flow.objects.filter(slug="default-provider-invalidation-flow").first()
if invalidation is None:
    invalidation = Flow.objects.filter(designation="invalidation").first()

# RS256 signing key required for JWKS-based validation (not HS256 client secret)
signing = CertificateKeyPair.objects.filter(name__icontains="JWT").first()
if signing is None:
    signing = CertificateKeyPair.objects.filter(key_data__isnull=False).exclude(key_data="").first()
assert signing is not None, "no CertificateKeyPair for JWT signing"

# GLOBAL issuer so customer + admin tokens share one OIDC_ISSUER for e2e
scope_qs = ScopeMapping.objects.filter(scope_name__in=["openid", "profile", "email"])
if not scope_qs.exists():
    scope_qs = ScopeMapping.objects.all()[:5]

def ensure_m2m(name: str, client_id: str):
    with transaction.atomic():
        defaults = {
            "authorization_flow": auth_flow,
            "client_type": ClientTypes.CONFIDENTIAL,
            "client_id": client_id,
            "include_claims_in_id_token": True,
            "sub_mode": SubModes.USER_ID,
            "issuer_mode": IssuerMode.GLOBAL,
            "signing_key": signing,
        }
        if invalidation is not None:
            defaults["invalidation_flow"] = invalidation
        provider, _created = OAuth2Provider.objects.update_or_create(
            name=f"{name}-provider",
            defaults=defaults,
        )
        if scope_qs.exists():
            provider.property_mappings.set(list(scope_qs))
        Application.objects.update_or_create(
            slug=name,
            defaults={
                "name": name,
                "provider": provider,
                "meta_launch_url": "blank://blank",
            },
        )
        return provider

cust = ensure_m2m("graphql-e2e-customer", "graphql-e2e-customer")
adm = ensure_m2m("graphql-e2e-admin", "graphql-e2e-admin")

cust.refresh_from_db()
adm.refresh_from_db()

print(f"CUSTOMER_CLIENT_ID={cust.client_id}")
print(f"CUSTOMER_CLIENT_SECRET={cust.client_secret}")
print(f"ADMIN_CLIENT_ID={adm.client_id}")
print(f"ADMIN_CLIENT_SECRET={adm.client_secret}")
print(f"API_TOKEN={tok.key}")
' 2>/tmp/authentik-bootstrap.err) || {
  echo "ERROR: ak shell bootstrap failed"
  cat /tmp/authentik-bootstrap.err || true
  echo "$BOOT_OUT"
  exit 1
}

# Merge stderr progress with any useful errors when parsing fails
echo "$BOOT_OUT" | grep -E '^[A-Z_]+=' >/tmp/authentik-boot.env || {
  echo "ERROR: bootstrap did not emit KEY=value lines"
  echo "$BOOT_OUT"
  cat /tmp/authentik-bootstrap.err || true
  exit 1
}

# shellcheck disable=SC1091
source /tmp/authentik-boot.env

# GLOBAL issuer mode → discovery at origin; application slug path still works for apps.
ISSUER="${BASE}/"
TOKEN_URL="${BASE}/application/o/token/"

# Wait for discovery (global issuer)
echo "==> Wait for OIDC discovery at $ISSUER"
for i in $(seq 1 60); do
  if curl -fsS "${ISSUER}.well-known/openid-configuration" >/dev/null 2>&1 \
    || curl -fsS "${BASE}/application/o/graphql-e2e-customer/.well-known/openid-configuration" >/dev/null 2>&1; then
    # Prefer issuer advertised by application discovery when global not exposed
    if ! curl -fsS "${ISSUER}.well-known/openid-configuration" >/dev/null 2>&1; then
      ISSUER="${BASE}/application/o/graphql-e2e-customer/"
    fi
    echo "    discovery ready at $ISSUER (${i}s)"
    break
  fi
  sleep 2
  if [[ $i -eq 60 ]]; then
    echo "ERROR: discovery never ready"
    docker compose -f "$COMPOSE" logs --tail=80 server || true
    exit 1
  fi
done

# Wait until client_credentials mints a token
echo "==> Wait for client_credentials mint"
for i in $(seq 1 60); do
  code=$(curl -sS -o /tmp/ak-token.json -w '%{http_code}' -X POST "$TOKEN_URL" \
    -d "grant_type=client_credentials" \
    -d "client_id=${CUSTOMER_CLIENT_ID}" \
    -d "client_secret=${CUSTOMER_CLIENT_SECRET}" \
    -d "scope=openid" || echo 000)
  if [[ "$code" == "200" ]]; then
    echo "    mint ready (${i}s)"
    break
  fi
  sleep 2
  if [[ $i -eq 60 ]]; then
    echo "ERROR: client_credentials still failing (HTTP $code)"
    cat /tmp/ak-token.json 2>/dev/null || true
    # Still write env so suite can hard-fail with diagnostics
    cat > "$OUT" <<EOF
AUTHENTIK_E2E=1
OIDC_ISSUER=$ISSUER
OIDC_AUDIENCE=$CUSTOMER_CLIENT_ID
AUTHENTIK_TOKEN_URL=$TOKEN_URL
AUTHENTIK_E2E_CUSTOMER_CLIENT_ID=$CUSTOMER_CLIENT_ID
AUTHENTIK_E2E_CUSTOMER_CLIENT_SECRET=$CUSTOMER_CLIENT_SECRET
AUTHENTIK_E2E_ADMIN_CLIENT_ID=$ADMIN_CLIENT_ID
AUTHENTIK_E2E_ADMIN_CLIENT_SECRET=$ADMIN_CLIENT_SECRET
EOF
    exit 1
  fi
done

# Derive issuer + audience from minted token (authoritative for validation)
eval "$(python3 - <<PY
import json,base64,shlex
tok=json.load(open("/tmp/ak-token.json"))["access_token"]
payload=tok.split(".")[1]
payload += "=" * (-len(payload) % 4)
claims=json.loads(base64.urlsafe_b64decode(payload))
print("claims", {k: claims.get(k) for k in ("aud","azp","client_id","sub","iss","alg")}, file=__import__("sys").stderr)
hdr=json.loads(base64.urlsafe_b64decode(tok.split(".")[0] + "=" * (-len(tok.split(".")[0]) % 4)))
print("header", hdr, file=__import__("sys").stderr)
if hdr.get("alg","").startswith("HS"):
    raise SystemExit("token still HS* — signing_key not applied")
aud=claims.get("aud")
azp=claims.get("azp")
if isinstance(aud, list):
  aud = aud[0] if aud else (azp or "${CUSTOMER_CLIENT_ID}")
elif not aud:
  aud = azp or "${CUSTOMER_CLIENT_ID}"
iss = claims.get("iss") or "${ISSUER}"
if not iss.endswith("/"):
    # normalize only if discovery uses trailing slash convention
    pass
print(f"AUD={shlex.quote(str(aud))}")
print(f"ISSUER={shlex.quote(str(iss) if str(iss).endswith('/') else str(iss)+'/')}")
# keep issuer exactly as in token for validation (jsonwebtoken normalizes trailing slash)
print(f"ISSUER_RAW={shlex.quote(str(iss))}")
PY
)"
# Prefer exact iss claim from token (no forced trailing slash if token omits it)
ISSUER="${ISSUER_RAW:-$ISSUER}"

# JWKS lives on application path even when iss is GLOBAL (origin)
JWKS_URI="${BASE}/application/o/graphql-e2e-customer/jwks/"
if ! curl -fsS "$JWKS_URI" >/dev/null 2>&1; then
  JWKS_URI=$(python3 - <<PY
import json,urllib.request
d=json.load(urllib.request.urlopen("${BASE}/application/o/graphql-e2e-customer/.well-known/openid-configuration"))
print(d["jwks_uri"])
PY
)
fi

cat > "$OUT" <<EOF
AUTHENTIK_E2E=1
OIDC_ISSUER=$ISSUER
OIDC_AUDIENCE=$AUD
OIDC_JWKS_URI=$JWKS_URI
AUTHENTIK_TOKEN_URL=$TOKEN_URL
AUTHENTIK_E2E_CUSTOMER_CLIENT_ID=$CUSTOMER_CLIENT_ID
AUTHENTIK_E2E_CUSTOMER_CLIENT_SECRET=$CUSTOMER_CLIENT_SECRET
AUTHENTIK_E2E_ADMIN_CLIENT_ID=$ADMIN_CLIENT_ID
AUTHENTIK_E2E_ADMIN_CLIENT_SECRET=$ADMIN_CLIENT_SECRET
EOF

echo "==> Wrote $OUT"
cat "$OUT"
echo "==> Done. source $OUT && cargo test --test graphql_oidc_authentik --features graphql,sqlite,metrics"
