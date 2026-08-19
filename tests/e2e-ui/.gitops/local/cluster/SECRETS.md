# Local secrets

Committed GitOps YAML is non-secret. The Cluster definition selects the
gitignored `secrets/vault/` directory:

```yaml
spec:
  secretSync:
    path: secrets/vault
```

No SOPS step is required locally. Hops reads only that bounded input, writes
values into the Cluster's local Vault, and does not copy secret values into its
state or logs. Environment charts consume them through normal ExternalSecret
resources.

Expected logical values for this fixture include:

| Vault value | Kubernetes consumer |
|---|---|
| Zitadel masterkey | `auth/zitadel-masterkey` ExternalSecret |
| demo human passwords | `default/e2e-human-passwords` ExternalSecret |
| Environment session/login credentials | `<environment>/e2e-ui-oidc` ExternalSecret |

The Zitadel provider's generated OIDC client ID and secret do not enter the
local secret directory. The Oidc managed resource writes those directly to its
generation-specific connection Secret (`attribute.client_id` and
`attribute.client_secret`).

The Cluster tree owns `SecretStack`, Vault/ESO RBAC, and the masterkey
ExternalSecret under `.gitops/local/cluster/{secrets,auth}`. The explicit
`ui/.gitops/test-users` deploy owns test identity ExternalSecrets and managed
resources. The API and UI workload charts only consume Secrets.

Rules:

1. Never commit passwords, PATs, client secrets, masterkeys, or Vault exports.
2. Keep local secret input below the configured gitignored directory.
3. Keep secret names and key references declarative in GitOps.
4. Use SOPS or an external secret manager for cloud workflows; it is not part
   of the Local Workbench input path.
