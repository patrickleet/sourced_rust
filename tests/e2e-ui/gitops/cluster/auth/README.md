# Auth (local)

| File | Kind |
|------|------|
| `stack.yaml` | `AuthStack` XR — Zitadel + embedded `PSQLCluster` |

## Local addressing (no ingress)

| | Value |
|--|--------|
| Install namespace | **`auth`** (not `zitadel`) |
| AuthStack / release name | **`zitadel`** |
| Service FQDN | **`zitadel.auth.svc.cluster.local:8080`** |
| OIDC issuer | `http://zitadel.auth.svc.cluster.local:8080` |
| Gateway / ingress | disabled — ClusterIP only for now |

## Apply order

```bash
hops local start --backend dory --gitops ./gitops/cluster
# or after packages:
hops local gitops cluster ./gitops/cluster

kubectl get authstack zitadel -n default
kubectl get svc -n auth
# expect: zitadel.auth.svc.cluster.local
```

## e2e-ui identity

See [`../e2e-identity/`](../e2e-identity/) for Project / humans / OIDC app / Grant XRs.
ProviderConfig residual:

```bash
hops local zitadel --source-context dory --source-namespace auth \
  --domain zitadel.auth.svc.cluster.local --port 8080 --insecure \
  --gitops ./gitops/cluster
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
