# `gitops/cluster` — local control-plane configuration

> **Real meta repos:** put this tree at the **meta root** (`gitops/cluster/`),
> not inside a single project. One local CP (dory) serves every worktree; apps
> only get different **namespaces**. This e2e-ui copy is a self-contained
> fixture — same files, nested under the test app. Point hops with
> `hops local gitops cluster ./gitops/cluster` or `HOPS_LOCAL_CLUSTER` when
> applying a meta-root cluster tree.

Platform / CP resources for the **shared** local control plane:

```text
<meta>/
  gitops/cluster/           # ← this tree (AuthStack, PSQLStack, configurations)
  gitops/envs/local/        # Application YAMLs → namespace = --name
  clients/foo/.gitops/deploy/
  platform/api/.gitops/deploy/
```

| Path | Contents |
|------|----------|
| `configurations/` | Crossplane `Configuration` installs (psql-stack, auth-stack, secret-stack) |
| `stacks/psql.yaml` | **`PSQLStack` XR** — CNPG + SC `psql` (local-path) |
| `stacks/auth.yaml` | **`AuthStack` XR** — Zitadel + embedded Postgres |
| `secrets/` | **`SecretStack`** + vault-auth-delegator CRB (ESO + Vault; k8s auth via Helm postStart) |
| `auth/` | Auth residuals (e.g. masterkey ExternalSecret), not the XR claim |
| `providers/` | Provider installs + per-provider DRCs (`helm.yaml` / `helm-drc.yaml`, …) |
| `providerconfigs/` | ProviderConfig shapes (`secretRef` only) |
| `e2e-identity/` | **Moved** — worktree UI chart `identity.*` values (pointer README only) |

## Apply + watch (local gitops vibes)

`hops local start --gitops PATH` is bootstrap then **`hops local gitops cluster
PATH`** (apply + watch). Start stays in the foreground until Ctrl+C. Day-to-day
without re-bootstrap: `hops local gitops cluster` alone. Use `--once` for a
single reconcile (CI/scripts).

```bash
# Bootstrap + cluster apply/watch (Ctrl+C to stop)
hops local start --backend dory --gitops ./gitops/cluster

# Cluster-only if CP already up
# hops local gitops cluster ./gitops/cluster

# Per-worktree apps (other terminal)
hops local gitops worktree ./gitops/envs/local --name dogfood
```

Crossplane reconciles XRs (`PSQLStack`, `AuthStack`, `SecretStack`, …) after each apply.
Edit `stacks/auth.yaml`, `stacks/psql.yaml`, or `secrets/stack.yaml` → saved → applied → CP converges.

**SecretStack (local):** install the Configuration from source first
(`hops config install --path …/xrs/stacks/aws/secret`), then apply this cluster tree
so `secrets/stack.yaml` is not stuck Unpacking.

## vs `gitops/envs/`

| Tree | Purpose |
|------|---------|
| **cluster/** | CP packages + platform XRs (PSQLStack, AuthStack) |
| **envs/\<name\/** | App `Application` YAMLs (`hops local gitops worktree`) |

```bash
# Apps after AuthStack is Ready — watches by default
hops local gitops worktree ./gitops/envs/local --name dogfood
```

## Secrets

See [SECRETS.md](./SECRETS.md):

- AuthStack local uses **inline masterkey** (dev only) — no ESO
- Cloud ProviderConfigs still reference live Secrets (`hops local aws|github|zitadel`)
- No local External Secrets path yet
