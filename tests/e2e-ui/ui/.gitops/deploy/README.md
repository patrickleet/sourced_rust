# e2e-ui UI chart (workbench)

Renders:

- **Deployment / Service** (`appRuntime: cluster-dev` \| `package`)
- **Optional identity** when `identity.enabled: true`:
  - **Cluster-shared:** Project, roles (`user` / `admin`), demo humans
  - **Per worktree:** OIDC web app only (redirects + Login V2 `baseUri`)

The OIDC web app + Login V2 `baseUri` belong with the UI (browser sign-in),
not the API.

## Identity scope

| Resource | Scope | K8s name example |
|----------|--------|------------------|
| Project | **Cluster** | `e2e-ui` |
| Roles | **Cluster** | `e2e-role-user`, `e2e-role-admin` |
| HumanUsers | **Cluster** | `e2e-alice` (login `alice`) |
| Password secret | **Cluster** | `e2e-human-passwords` |
| OIDC app | **Worktree** | `e2e-ui-dogfood-web` |

Demo login (every local env): **alice / bob / admin · Password1!**

## Values

| Value | Purpose |
|-------|---------|
| `identity.enabled` | Gate identity templates |
| `identity.orgId` | Residual FirstInstance org id |
| `identity.projectName` | Shared Project name (default `e2e-ui`) |
| `identity.workspace` | Worktree id for OIDC app names only |
| `identity.demoUsers` | Cluster-shared alice / bob / admin |
| `identity.projectNamespace` | Namespace for Project + Role MRs |
| `identity.humansNamespace` | Namespace for HumanUser MRs |
| `identity.mrNamespace` | Namespace for worktree OIDC app MRs |
| `identity.uiBaseURL` | Optional; else `http://e2e-ui-ui.<namespace>.svc…:5180` |
| `identity.passwordSecret.name` | Shared password secret (default `e2e-human-passwords`) |

```yaml
# gitops/envs/local/ui.yaml
identity:
  enabled: true
  orgId: "…"   # residual after AuthStack Ready
  demoUsers: true
  providerConfigRef:
    name: default
    kind: ClusterProviderConfig
```

User grants (human → project roles) are residual once per CP after Project +
humans are Ready. Prefer a **ClusterProviderConfig** so all identity MRs share
credentials.

API still needs residual `OIDC_AUDIENCE` / project id (and app `e2e-ui-oidc`
secret keys) after the shared Project and worktree OIDC app are Ready.
