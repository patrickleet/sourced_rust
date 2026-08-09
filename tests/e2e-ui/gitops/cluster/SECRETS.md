# Secrets (local gitops)

Gitops YAML is **non-secret**. Live `Secret` objects stay on the cluster (or
later ESO/SOPS). Config only **references** names/keys.

## AuthStack / Zitadel (`ns: auth`)

| Secret | Purpose | How to create |
|--------|---------|----------------|
| `zitadel-masterkey` | 32-byte Zitadel at-rest key (`AuthStack` `masterkey.secretRef`) | `kubectl -n auth create secret generic zitadel-masterkey --from-literal=masterkey='…'` (exactly 32 bytes) |
| `iam-admin-pat` | Chart FirstInstance PAT (auto after setup) | AuthStack / helm hook |
| `iam-admin` | Machine key JSON | AuthStack / helm hook |
| `login-client` | Login client PAT | AuthStack / helm hook |

ProviderConfig for MRs (after PAT exists):

```bash
hops local zitadel --source-context dory --source-namespace auth \
  --domain zitadel-zitadel.auth.svc.cluster.local --port 8080 --insecure
# creates default/zitadel-credentials + ProviderConfig secretRef
```

Helm default fullname is **`zitadel-zitadel`** — use that Service FQDN for
`domain` / OIDC issuer (no alias Service). Optional later:
`chartValues.fullnameOverride: zitadel`.

## Identity (UI chart)

Renders from **`ui/.gitops/deploy`** when `identity.enabled: true`
(see `gitops/envs/local/ui.yaml`).

| Scope | Resources |
|-------|-----------|
| **Cluster** | Project `e2e-ui`, roles, humans `alice`/`bob`/`admin` |
| **Worktree** | OIDC web app only (`e2e-ui-<ws>-web` — redirects + Login V2 baseUri) |

| Secret | Purpose |
|--------|---------|
| `e2e-human-passwords` | Keys `alice`, `bob`, `admin` (cluster-shared) |
| `zitadel-credentials` | Provider PC / ClusterProviderConfig (`hops local zitadel`) |

```bash
kubectl -n default create secret generic e2e-human-passwords \
  --from-literal=alice=Password1! \
  --from-literal=bob=Password1! \
  --from-literal=admin=Password1!
```

Demo login (every env): **alice / bob / admin · Password1!**

## e2e-ui app (`ns: hops-wt-*`)

| Secret | Keys | Used by |
|--------|------|---------|
| `e2e-ui-oidc` | `AUTH_SECRET`, `OIDC_CLIENT_ID`, `OIDC_CLIENT_SECRET`, `ZITADEL_SERVICE_USER_TOKEN` | UI chart `secretEnv` → `secretKeyRef` |

```bash
# OIDC client secret: regenerate via Management API _generate_client_secret
# Login-client PAT: AuthStack residual secret auth/login-client key pat
kubectl -n hops-wt-dogfood create secret generic e2e-ui-oidc \
  --from-literal=AUTH_SECRET='local-workbench-dogfood-auth-secret-not-for-prod' \
  --from-literal=OIDC_CLIENT_ID='…from Oidc app…' \
  --from-literal=OIDC_CLIENT_SECRET='…from Oidc app create…' \
  --from-literal=ZITADEL_SERVICE_USER_TOKEN="$(kubectl -n auth get secret login-client -o jsonpath='{.data.pat}' | base64 -d)"
```

Non-secret OIDC config (issuer, AUTH_URL) stays in `ui/.gitops/deploy/values.yaml`.

### Login V2 (required for browser sign-in)

Custom e2e-ui `/login` pages own Login V2 (Oidc `loginVersion.loginV2.baseUri`
on the **worktree** OIDC app). Prefer app-level baseUri over instance-wide
defaults so multiple worktrees can coexist.

## SecretStack / ESO + Vault (`secrets/`)

Local fixture uses **`backend: vault`** with **`vault.install: true`** (in-cluster
Vault Helm + ESO). No AWS PodIdentity on dory.

| Resource | Purpose |
|----------|---------|
| `secrets/stack.yaml` | SecretStack XR — ESO + Vault Helm + ClusterSecretStore |
| `secrets/vault-auth-delegator.yaml` | CRB: Vault SA → `system:auth-delegator` (TokenReview) |
| Helm `vault` `server.postStart` | Init/unseal (once), KV mount, k8s auth + ESO role |
| PVC (`dataStorage` 1Gi) | File storage under `/vault/data` (survives pod restarts) |
| ClusterSecretStore `vault` | ESO → Vault (Ready after postStart + CRB) |

### Persistence

`server.dev` is **off**. Vault runs standalone with `storage "file"` on a PVC
(default StorageClass, dory: `local-path`). First start initializes and writes:

```text
/vault/data/.hops-init   # Unseal Key 1 + Initial Root Token (chmod 600)
```

Later starts: unseal from that file, re-apply k8s auth (idempotent). **KV data
survives pod restarts**; deleting the PVC is a full reset.

Root token is **not** the fixed string `root` (that was dev-only). For CLI writes:

```bash
export VAULT_TOKEN="$(
  kubectl -n vault exec vault-0 -- \
    awk '/^Initial Root Token:/{print $NF}' /vault/data/.hops-init
)"
export VAULT_ADDR=http://127.0.0.1:8200
# kubectl port-forward -n vault svc/vault 8200:8200   # if needed
hops secrets sync vault -y
```

### Declarative trust path

1. Apply cluster gitops (SecretStack + CRB).
2. postStart: init (once) → unseal → `secret/` KV v2 → k8s auth for ESO.
3. ClusterSecretStore `vault` Ready.
4. **Write** with `hops secrets sync vault`.
5. **Read** via ExternalSecrets (auth masterkey, UI oidc/humans).

Install package from source before first apply:

```bash
hops config install --path <meta>/xrs/stacks/aws/secret --context dory
hops local gitops cluster ./gitops/cluster
```

Switching an existing **dev/inmem** Vault release to file storage may need a
one-time recreate of the Helm release / PVC (data was ephemeral anyway).

Do **not** put the SecretStack claim under meta `local/` — that folder is for
ad-hoc colima-style one-off claims, not the workbench cluster gitops path.

## Rules

1. **Never** put passwords, PATs, client secrets, or masterkeys in committed YAML.
2. Gitops may name Secrets and keys (`secretRef` / `secretEnv`).
3. Create Secrets before the XRs/Deployments that need them (or accept CrashLoop until present).
4. Cloud: prefer ESO / SOPS; local: `kubectl create` or `hops local zitadel|aws|github`.
5. Platform secrets engine for local CP lives in **`gitops/cluster/secrets/`**, not `local/`.
