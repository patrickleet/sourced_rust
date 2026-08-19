#!/usr/bin/env bash
# Render-only contract for the Local Workbench fixture. No cluster required.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
fail=0

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required tool: $1" >&2
    exit 2
  }
}
need helm
need rg

assert_contains() {
  local hay="$1" needle="$2" message="$3"
  if printf '%s' "$hay" | rg -q --fixed-strings -- "$needle"; then
    echo "ok: $message"
  else
    echo "FAIL: $message (missing: $needle)" >&2
    fail=1
  fi
}

assert_not_contains() {
  local hay="$1" needle="$2" message="$3"
  if printf '%s' "$hay" | rg -q --fixed-strings -- "$needle"; then
    echo "FAIL: $message (unexpected: $needle)" >&2
    fail=1
  else
    echo "ok: $message"
  fi
}

assert_render_fails() {
  local message="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    echo "FAIL: $message" >&2
    fail=1
  else
    echo "ok: $message"
  fi
}

render_local_ui() {
  local namespace="$1" preview="${2:-false}" generation="${3:-0}"
  helm template "e2e-ui-ui-${namespace}" "$ROOT/ui/.gitops/local" \
    --namespace "$namespace" \
    --set local=true \
    --set "preview=${preview}" \
    --set "environment.name=${namespace}" \
    --set "environment.namespace=${namespace}" \
    --set identity.enabled=true \
    --set "identity.oidcGeneration=${generation}"
}

render_test_users() {
  local namespace="$1" features="${2:-true}" generation="${3:-0}"
  helm template "e2e-ui-test-users-${namespace}" "$ROOT/ui/.gitops/test-users" \
    --namespace "$namespace" \
    --set local=true \
    --set preview=false \
    --set "environment.name=${namespace}" \
    --set "environment.namespace=${namespace}" \
    --set identity.enabled=true \
    --set "identity.oidcGeneration=${generation}" \
    --set "identity.instanceLoginV2=${features}" \
    --set identity.providerConfigRef.name=default \
    --set identity.providerConfigRef.kind=ClusterProviderConfig
}

echo "=== project definitions ==="
cluster_definition="$(<"$ROOT/.gitops/local/cluster.yaml")"
environment_definition="$(<"$ROOT/.gitops/local/environment.yaml")"
assert_contains "$cluster_definition" "kind: Cluster" "Cluster definition is Kubernetes-shaped"
assert_contains "$cluster_definition" "path: tests/e2e-ui/.gitops/local/cluster" "Cluster selects the project manifest tree from the checkout root"
assert_contains "$cluster_definition" "mountRoot: ../../../.." "Cluster mounts the Distributed checkout root"
assert_contains "$environment_definition" "kind: Environment" "Environment definition is Kubernetes-shaped"
assert_contains "$environment_definition" "- path: api" "Environment deploys the API application root"
assert_contains "$environment_definition" "- path: ui" "Environment deploys the UI application root"
assert_contains "$environment_definition" "chart: .gitops/test-users" "test users are an explicit deploy chart"
assert_not_contains "$environment_definition" ".gitops/promote" "local Environment does not require promotion"
assert_not_contains "$environment_definition" "appRuntime" "Environment has no runtime mode switch"
assert_not_contains "$environment_definition" "sourceGeneration" "Environment has no manual restart counter"

echo "=== direct local charts ==="
ui_local="$(render_local_ui alice false 1)"
ui_preview="$(render_local_ui alice true 1)"
api_local="$(helm template e2e-ui-api-alice "$ROOT/api/.gitops/local" \
  --namespace alice \
  --set local=true \
  --set preview=false \
  --set environment.name=alice \
  --set environment.namespace=alice \
  --set identity.enabled=true \
  --set identity.oidcGeneration=1)"

for rendered in "$ui_local" "$ui_preview" "$api_local"; do
  assert_contains "$rendered" "kind: Deployment" "local chart renders its workload directly"
  assert_contains "$rendered" "kind: Service" "local chart renders its Service directly"
  assert_not_contains "$rendered" "kind: Application" "local chart does not require an Application wrapper"
  assert_not_contains "$rendered" "kind: PSQLCluster" "application charts do not own the shared PSQLCluster"
  assert_not_contains "$rendered" "source-generation" "source changes do not use restart annotations"
  assert_not_contains "$rendered" "appRuntime" "render has no mixed runtime selector"
done
assert_contains "$ui_local" "http://e2e-ui-ui.alice.svc.cluster.local:5180" "UI AUTH_URL follows the Environment namespace"
assert_contains "$ui_local" 'name: "e2e-ui-alice-oidc-conn-g1"' "UI consumes the Environment OIDC connection Secret"
assert_contains "$api_local" 'name: "e2e-ui-alice-oidc-conn-g1"' "API consumes the same OIDC connection Secret"
assert_contains "$api_local" "cargo watch" "API process owns Rust source reload"
assert_render_fails "local UI chart rejects local=false" \
  helm template invalid "$ROOT/ui/.gitops/local" --set local=false
assert_render_fails "local API chart rejects local=false" \
  helm template invalid "$ROOT/api/.gitops/local" --set local=false

echo "=== separate test identities ==="
alice_users="$(render_test_users alice true 1)"
bob_users="$(render_test_users bob false 0)"
assert_contains "$alice_users" "kind: Project" "test-users chart renders the shared Project"
assert_contains "$alice_users" "kind: HumanUser" "test-users chart renders demo humans"
assert_contains "$alice_users" "kind: Grant" "test-users chart renders role grants"
assert_contains "$alice_users" "kind: Oidc" "test-users chart renders the Environment OIDC app"
assert_contains "$alice_users" "kind: Features" "test-users chart may own Login V2 features"
assert_not_contains "$bob_users" "kind: Features" "features can be disabled for a second Environment"
assert_contains "$alice_users" "name: e2e-ui-alice-web-g1" "alice identity is Environment-scoped"
assert_contains "$bob_users" "name: e2e-ui-bob-web" "bob identity is Environment-scoped"
assert_not_contains "$alice_users" "kind: Deployment" "test-users chart contains no workload Deployment"
assert_not_contains "$alice_users" "kind: Service" "test-users chart contains no workload Service"

echo "=== independent cloud charts ==="
ui_cloud="$(helm template e2e-ui-ui "$ROOT/ui/.gitops/deploy" --namespace staging --set local=false --set preview=false)"
api_cloud="$(helm template e2e-ui-api "$ROOT/api/.gitops/deploy" --namespace staging --set local=false --set preview=false)"
for rendered in "$ui_cloud" "$api_cloud"; do
  assert_contains "$rendered" "kind: Deployment" "cloud chart renders the packaged workload"
  assert_contains "$rendered" "kind: Service" "cloud chart renders the packaged Service"
  assert_not_contains "$rendered" "hostPath:" "cloud chart has no local source mount"
  assert_not_contains "$rendered" "kind: HumanUser" "cloud workload chart has no test identities"
done
assert_render_fails "cloud UI chart rejects local=true" \
  helm template invalid "$ROOT/ui/.gitops/deploy" --set local=true
assert_render_fails "cloud API chart rejects local=true" \
  helm template invalid "$ROOT/api/.gitops/deploy" --set local=true

echo "=== optional cloud promotion ==="
ui_promote="$(helm template promote-ui "$ROOT/ui/.gitops/promote" \
  --set local=false \
  --set preview=true \
  --set environment.name=preview-42 \
  --set environment.namespace=preview-42 \
  --set source.repoURL=https://example.invalid/distributed.git \
  --set source.targetRevision=revision-sentinel \
  --set deploy.values.image.tag=preview-42 \
  --set workflowOnly=do-not-forward)"
api_promote="$(helm template promote-api "$ROOT/api/.gitops/promote" \
  --set local=false \
  --set preview=false \
  --set environment.name=staging \
  --set environment.namespace=staging \
  --set source.repoURL=https://example.invalid/distributed.git \
  --set deploy.values.image.tag=release-1)"
assert_contains "$ui_promote" "path: tests/e2e-ui/ui/.gitops/deploy" "UI promotion targets only the cloud deploy chart"
assert_contains "$api_promote" "path: tests/e2e-ui/api/.gitops/deploy" "API promotion targets only the cloud deploy chart"
assert_contains "$ui_promote" "tag: preview-42" "promotion forwards deploy.values"
assert_not_contains "$ui_promote" "workflowOnly" "promotion workflow values do not leak into workload values"
assert_render_fails "promotion chart rejects local=true" \
  helm template invalid "$ROOT/ui/.gitops/promote" \
    --set local=true \
    --set environment.name=local \
    --set environment.namespace=local \
    --set source.repoURL=https://example.invalid/distributed.git

if [[ "$fail" -ne 0 ]]; then
  echo "helm-contract-test: FAILED" >&2
  exit 1
fi
echo "helm-contract-test: OK"
