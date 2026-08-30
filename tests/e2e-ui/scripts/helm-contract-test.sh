#!/usr/bin/env bash
# Chart contract tests for multi-workspace identity scopes.
# No cluster required — helm template only.
#
#   cd tests/e2e-ui && ./scripts/helm-contract-test.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UI_CHART="$ROOT/ui/.gitops/local"
IDENTITY_CHART="$ROOT/ui/.gitops/test-users"
API_CHART="$ROOT/api/.gitops/local"
fail=0

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required tool: $1" >&2
    exit 2
  }
}
need helm
need rg

render_ui() {
  local ns="$1"
  local oidc_generation="${2:-0}"
  helm template "e2e-ui-ui-${ns}" "$UI_CHART" \
    --namespace "$ns" \
    --set local=true \
    --set preview=false \
    --set "environment.name=${ns}" \
    --set "environment.namespace=${ns}" \
    --set identity.enabled=true \
    --set "identity.oidcGeneration=${oidc_generation}" \
    --set identity.projectName=e2e-ui \
    --set identity.projectNamespace=default \
    --set identity.humansNamespace=default \
    --set identity.providerConfigRef.name=default \
    --set identity.providerConfigRef.kind=ClusterProviderConfig
}

render_identity() {
  local ns="$1"
  local features="${2:-true}"
  local oidc_generation="${3:-0}"
  helm template "e2e-ui-test-users-${ns}" "$IDENTITY_CHART" \
    --namespace "$ns" \
    --set local=true \
    --set preview=false \
    --set "environment.name=${ns}" \
    --set "environment.namespace=${ns}" \
    --set identity.enabled=true \
    --set "identity.oidcGeneration=${oidc_generation}" \
    --set identity.projectName=e2e-ui \
    --set identity.projectNamespace=default \
    --set identity.humansNamespace=default \
    --set "identity.instanceLoginV2=${features}" \
    --set identity.providerConfigRef.name=default \
    --set identity.providerConfigRef.kind=ClusterProviderConfig
}

render() {
  local ns="$1"
  local features="${2:-true}"
  local oidc_generation="${3:-0}"
  render_ui "$ns" "$oidc_generation"
  render_identity "$ns" "$features" "$oidc_generation"
}

assert_contains() {
  local hay="$1" needle="$2" msg="$3"
  if ! printf '%s' "$hay" | rg -q --fixed-strings "$needle"; then
    echo "FAIL: $msg (missing: $needle)" >&2
    fail=1
  else
    echo "ok: $msg"
  fi
}

assert_not_contains() {
  local hay="$1" needle="$2" msg="$3"
  if printf '%s' "$hay" | rg -q --fixed-strings "$needle"; then
    echo "FAIL: $msg (unexpected: $needle)" >&2
    fail=1
  else
    echo "ok: $msg"
  fi
}

assert_not_matches() {
  local hay="$1" pattern="$2" msg="$3"
  if printf '%s' "$hay" | rg -q "$pattern"; then
    echo "FAIL: $msg (unexpected match: $pattern)" >&2
    fail=1
  else
    echo "ok: $msg"
  fi
}

check_workspace() {
  local ns="$1"
  local features="$2"
  echo "=== workspace ${ns} (instanceLoginV2=${features}) ==="
  local out
  out="$(render "$ns" "$features")"

  assert_contains "$out" "name: e2e-ui-${ns}-web" "OIDC app name is worktree-scoped"
  assert_contains "$out" "namespace: ${ns}" "worktree resources reference ${ns}"
  assert_contains "$out" "baseUri: \"http://e2e-ui-ui.${ns}.svc.cluster.local:5180\"" \
    "Login V2 / OIDC baseUri uses release namespace"
  assert_contains "$out" "http://e2e-ui-ui.${ns}.svc.cluster.local:5180/auth/callback/oidc" \
    "OIDC redirect uses release FQDN"
  assert_contains "$out" "namespace: default" "shared identity keeps default namespace"
  assert_contains "$out" "name: e2e-ui" "shared Project name e2e-ui"
  assert_contains "$out" "projectRoleCheck: true" \
    "Project requires a GitOps-owned role grant"
  assert_contains "$out" "name: e2e-role-user" "shared role e2e-role-user"
  assert_contains "$out" "name: e2e-alice" "shared human e2e-alice"
  assert_contains "$out" "apiVersion: auth.hops.ops.com.ai/v1alpha1" \
    "demo humans use the auth-stack HumanUser XR"
  assert_contains "$out" "name: e2e-alice-e2e-ui" "shared Grant for alice"
  assert_contains "$out" "name: e2e-bob-e2e-ui" "shared Grant for bob"
  assert_contains "$out" "name: e2e-admin-e2e-ui" "shared Grant for admin"
  assert_contains "$out" "userIdRef:" "Grant resolves HumanUser by reference"
  assert_contains "$out" "projectIdRef:" "Grant resolves Project by reference"
  assert_contains "$out" "apiVersion: auth.hops.ops.com.ai/v1alpha1" \
    "Grant resolves the auth-stack HumanUser XR"
  assert_contains "$out" 'roles: ["user","admin"]' "admin receives user and admin roles"
  assert_not_contains "$out" "oidc-local-seed" \
    "GitOps does not overwrite the residual OIDC session/PAT Secret"
  assert_contains "$out" "name: \"e2e-ui-${ns}-oidc-conn\"" \
    "OIDC connection secret is worktree-scoped"
  assert_contains "$out" "key: attribute.client_id" \
    "UI reads client id from Oidc connection secret"
  assert_contains "$out" "key: attribute.client_secret" \
    "UI reads client secret from Oidc connection secret"
  assert_contains "$out" "namespace: default" "projectIdRef targets shared Project ns"
  assert_contains "$out" "projectIdRef:" "OIDC references Project"
  # Project MR must declare default ns (not worktree)
  if ! printf '%s' "$out" | rg -U -q 'kind: Project\nmetadata:\n  name: e2e-ui\n  namespace: default'; then
    # tolerate key reorder from helm
    if ! printf '%s' "$out" | awk '/kind: Project/{p=1} p&&/namespace: default/{found=1} p&&/^---/{exit} END{exit !found}'; then
      echo "FAIL: Project not clearly in default namespace" >&2
      fail=1
    else
      echo "ok: Project in default namespace"
    fi
  else
    echo "ok: Project in default namespace"
  fi

  if [ "$features" = "true" ]; then
    assert_contains "$out" "kind: Features" "Features MR rendered when instanceLoginV2"
    assert_contains "$out" "name: e2e-ui-login-v2" "Features name stable"
  else
    assert_not_contains "$out" "kind: Features" "no Features when instanceLoginV2=false"
  fi

  assert_not_contains "$out" "hops-wt-" "no legacy hops-wt- prefix"
  assert_not_matches "$out" 'orgId:\s*"[0-9]{15,}"' \
    "no generated organization UUID in GitOps"
}

check_workspace alice true
check_workspace bob false

# Distinct OIDC names across workspaces
alice_out="$(render alice true)"
bob_out="$(render bob false)"
assert_contains "$alice_out" "e2e-ui-alice-web" "alice OIDC name"
assert_contains "$bob_out" "e2e-ui-bob-web" "bob OIDC name"
assert_not_contains "$alice_out" "e2e-ui-bob-web" "alice render has no bob OIDC"
assert_not_contains "$bob_out" "e2e-ui-alice-web" "bob render has no alice OIDC"

# Rotation changes the managed OIDC resource and connection Secret generation.
# The new UI pod cannot start until Crossplane publishes that generation's
# credentials, avoiding a race with stale data under a stable Secret name.
rotated_out="$(render alice true 1)"
assert_contains "$rotated_out" "name: e2e-ui-alice-web-g1" "OIDC generation rotates MR name"
assert_contains "$rotated_out" 'hops.ops.com.ai/oidc-generation: "1"' \
  "OIDC generation rolls UI pod template"
assert_contains "$rotated_out" 'name: "e2e-ui-alice-oidc-conn-g1"' \
  "OIDC rotation uses matching connection secret generation"

# API derives the same connection Secret and uses the generated client id as
# both the JWT audience and client id. No Zitadel UUID belongs in values.
api_out="$(helm template e2e-ui-api-alice "$API_CHART" \
  --namespace alice \
  --set local=true \
  --set preview=false \
  --set environment.name=alice \
  --set environment.namespace=alice \
  --set identity.enabled=true \
  --set identity.oidcGeneration=1 \
  --set identity.providerConfigRef.name=default \
  --set identity.providerConfigRef.kind=ClusterProviderConfig)"
assert_contains "$api_out" 'hops.ops.com.ai/oidc-generation: "1"' \
  "API OIDC generation rolls pod template"
assert_contains "$api_out" 'name: "e2e-ui-alice-oidc-conn-g1"' \
  "API uses matching generated OIDC connection secret"
assert_contains "$api_out" "name: OIDC_AUDIENCE" \
  "API audience comes from the generated connection secret"
assert_contains "$api_out" "key: attribute.client_id" \
  "API audience/client id use the generated client id key"

if [ "$fail" -ne 0 ]; then
  echo "helm-contract-test: FAILED" >&2
  exit 1
fi
echo "helm-contract-test: OK"
