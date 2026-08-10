#!/usr/bin/env bash
# Chart contract tests for multi-workspace identity scopes.
# No cluster required — helm template only.
#
#   cd tests/e2e-ui && ./scripts/helm-contract-test.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHART="$ROOT/ui/.gitops/deploy"
fail=0

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required tool: $1" >&2
    exit 2
  }
}
need helm
need rg

render() {
  local ns="$1"
  local features="${2:-true}"
  helm template "e2e-ui-ui-${ns}" "$CHART" \
    --namespace "$ns" \
    --set local=true \
    --set appRuntime=cluster-dev \
    --set "namespace=${ns}" \
    --set identity.enabled=true \
    --set identity.orgId=test-org \
    --set identity.projectName=e2e-ui \
    --set identity.projectNamespace=default \
    --set identity.humansNamespace=default \
    --set identity.mrNamespace= \
    --set "identity.instanceLoginV2=${features}" \
    --set identity.providerConfigRef.name=default \
    --set identity.providerConfigRef.kind=ClusterProviderConfig
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
  assert_contains "$out" "name: e2e-role-user" "shared role e2e-role-user"
  assert_contains "$out" "name: e2e-alice" "shared human e2e-alice"
  assert_contains "$out" "oidc-local-seed" "local OIDC residual Secret seeded"
  # quoted or bare name both acceptable from helm
  if ! printf '%s' "$out" | rg -q 'name: "?e2e-ui-oidc"?'; then
    echo "FAIL: local OIDC secret name e2e-ui-oidc missing" >&2
    fail=1
  else
    echo "ok: local OIDC secret name e2e-ui-oidc"
  fi
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

if [ "$fail" -ne 0 ]; then
  echo "helm-contract-test: FAILED" >&2
  exit 1
fi
echo "helm-contract-test: OK"
