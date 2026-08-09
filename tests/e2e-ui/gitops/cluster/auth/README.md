# Auth (local)

| File | Kind |
|------|------|
| [`../stacks/auth.yaml`](../stacks/auth.yaml) | `AuthStack` XR — Zitadel + embedded `PSQLCluster` |
| `external-secret-masterkey.yaml` | Optional ESO materialize of `zitadel-masterkey` from Vault |

## Local addressing (no ingress)

| | Value |
|--|--------|
| Install namespace | **`auth`** |
| AuthStack XR name | **`zitadel`** |
| Helm Service (default fullname) | **`zitadel-zitadel.auth.svc.cluster.local:8080`** |
| OIDC issuer / `domain` | `http://zitadel-zitadel.auth.svc.cluster.local:8080` |
| Gateway / ingress | disabled — ClusterIP only for now |

## Apply order

```bash
hops local start --backend dory --gitops ./gitops/cluster
# or after configurations:
hops local gitops cluster ./gitops/cluster

kubectl get authstack zitadel -n default
kubectl get svc -n auth
# expect: zitadel-zitadel (API) — not a separate short-name alias
```

## e2e-ui identity (worktree)

App Project / humans / OIDC app render from the **UI** chart when
`gitops/envs/local/ui.yaml` sets `identity.enabled: true` (not cluster gitops).

ProviderConfig residual (cluster):

```bash
hops local zitadel --source-context dory --source-namespace auth \
  --domain zitadel-zitadel.auth.svc.cluster.local --port 8080 --insecure
```

## Secrets

Masterkey is **not** inline — `secretRef` to `zitadel-masterkey` in ns `auth`:

```bash
kubectl -n auth create secret generic zitadel-masterkey \
  --from-literal=masterkey='hops-like-basketball-but-for-ops'  # exactly 32 bytes
```

Create **before** AuthStack becomes Ready (or re-apply after). Full table:
[../SECRETS.md](../SECRETS.md).

## Residual

- Host browser access without ingress is later (gateway / map / promote)
