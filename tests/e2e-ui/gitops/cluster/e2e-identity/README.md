# e2e-ui identity (Zitadel gitops)

Declarative stand-in for most of `scripts/up.sh` Management API bootstrap.

## Layout

| Path | What |
|------|------|
| `project.yaml` | Zitadel **Project** `e2e-ui` |
| `roles.yaml` | Project roles `user` + `admin` |
| `humans.yaml` | **HumanUser** MRs alice / bob / admin |
| `oidc-app.yaml` | OIDC web app for Auth.js (redirects → worktree UI) |
| `grants.yaml.example` | hops **Grant** XRs — copy to `grants.yaml` and fill UUIDs after humans Ready |
| `../providers/zitadel.yaml` | provider-upjet-zitadel pin |
| `../providerconfigs/zitadel.yaml` | PC shape (credentials residual) |

## Apply order

1. **AuthStack Ready** (`../auth/stack.yaml`) — Zitadel in ns **`auth`**, issuer  
   `http://zitadel.auth.svc.cluster.local:8080` (ClusterIP only; no ingress yet).

2. **Credentials residual** (once `iam-admin-pat` exists in ns `auth`):

   ```bash
   hops local zitadel \
     --source-context dory \
     --source-namespace auth \
     --domain zitadel.auth.svc.cluster.local \
     --port 8080 --insecure \
     --gitops ./gitops/cluster
   ```

3. **Password residual** (K8s Secret, not values.yaml):

   ```bash
   kubectl create secret generic e2e-human-passwords -n default \
     --from-literal=alice=Password1! \
     --from-literal=bob=Password1! \
     --from-literal=admin=Password1!
   ```

   Humans already `secretRef` that object. UI OIDC secrets: `e2e-ui-oidc` in the
   worktree namespace (`secretEnv` in `ui/.gitops/deploy/values.yaml`). See
   [../SECRETS.md](../SECRETS.md).

4. `hops local gitops cluster ./gitops/cluster` — project, roles, humans, oidc  

5. Observe IDs, copy `grants.yaml.example` → `grants.yaml`, fill UUIDs, re-apply  

## XRs vs provider MRs

| Resource | Kind |
|----------|------|
| Platform install | **AuthStack** XR |
| Role grants | **Grant** XR (`auth.hops.ops.com.ai`) |
| Project / Role / Human / OIDC app | provider MRs (no thin hops wrapper) |
| Machine identities (optional) | **MachineUser** XR |

## Residuals

- `zitadel-credentials` Secret (PAT JSON for ProviderConfig)  
- `e2e-human-passwords` Secret  
- OIDC **client secret** (only at app create — copy into UI env / Secret)  
- Grant UUID fill-in until composition supports refs from HumanUser  

## App wiring

UI chart values under `ui/.gitops/deploy` point `OIDC_ISSUER` at AuthStack issuer
(`http://auth.localtest.me` browser / in-cluster service for pods as needed).
Client id/secret still residual until connection details are automated.
