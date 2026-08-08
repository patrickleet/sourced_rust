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
  --domain zitadel.auth.svc.cluster.local --port 8080 --insecure \
  --gitops ./gitops/cluster
# creates default/zitadel-credentials + ProviderConfig secretRef
```

## e2e identity (`ns: default` for MRs)

| Secret | Purpose |
|--------|---------|
| `e2e-human-passwords` | Keys `alice`, `bob`, `admin` — HumanUser `initialPasswordSecretRef` |
| `zitadel-credentials` | Provider PC JSON (`access_token`, `domain`, `port`, `insecure`) |

```bash
kubectl -n default create secret generic e2e-human-passwords \
  --from-literal=alice=Password1! \
  --from-literal=bob=Password1! \
  --from-literal=admin=Password1!
```

## e2e-ui app (`ns: hops-wt-*`)

| Secret | Keys | Used by |
|--------|------|---------|
| `e2e-ui-oidc` | `AUTH_SECRET`, `OIDC_CLIENT_ID`, `OIDC_CLIENT_SECRET` | UI chart `secretEnv` → `secretKeyRef` |

```bash
kubectl -n hops-wt-dogfood create secret generic e2e-ui-oidc \
  --from-literal=AUTH_SECRET='local-workbench-dogfood-auth-secret-not-for-prod' \
  --from-literal=OIDC_CLIENT_ID='…from Oidc app…' \
  --from-literal=OIDC_CLIENT_SECRET='…from Oidc app create…'
```

Non-secret OIDC config (issuer, AUTH_URL) stays in `ui/.gitops/deploy/values.yaml`.

## SecretStack / ESO + Vault (`secrets/stack.yaml`)

Local fixture uses **`backend: vault`** with **`vault.install: true`** (in-cluster
Vault Helm + ESO). No AWS PodIdentity on dory.

| Resource | Purpose |
|----------|---------|
| Helm `external-secrets` | ESO operator |
| Helm `vault` | Dev Vault (when `vault.install`) |
| ClusterSecretStore `vault` | ESO → Vault (after both Helms exist) |

Auth for Vault SecretStore is **kubernetes** auth (role `external-secrets`).
Bootstrap Vault k8s auth + role is still manual/dev until automation lands;
stack is Ready for ESO+Vault install even if SecretStore auth needs follow-up.

Install package from source before first apply:

```bash
hops config install --path <meta>/xrs/stacks/aws/secret --context dory
hops local gitops cluster ./gitops/cluster
```

Do **not** put the SecretStack claim under meta `local/` — that folder is for
ad-hoc colima-style one-off claims, not the workbench cluster gitops path.

## Rules

1. **Never** put passwords, PATs, client secrets, or masterkeys in committed YAML.
2. Gitops may name Secrets and keys (`secretRef` / `secretEnv`).
3. Create Secrets before the XRs/Deployments that need them (or accept CrashLoop until present).
4. Cloud: prefer ESO / SOPS; local: `kubectl create` or `hops local zitadel|aws|github`.
5. Platform secrets engine for local CP lives in **`gitops/cluster/secrets/`**, not `local/`.
