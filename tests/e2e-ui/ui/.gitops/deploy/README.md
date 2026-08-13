# e2e-ui UI chart (workbench)

Renders:

- **Deployment / Service** (`appRuntime: cluster-dev` \| `package`)
- **Optional identity** when `identity.enabled: true`:
  - **Cluster-shared:** Project, roles (`user` / `admin`), demo humans, grants
  - **Per worktree:** OIDC web app only (redirects + Login V2 `baseUri`)

The OIDC web app + Login V2 `baseUri` belong with the UI (browser sign-in),
not the API.

## Identity scope

| Resource | Scope | K8s name example |
|----------|--------|------------------|
| Project | **Cluster** | `e2e-ui` |
| Roles | **Cluster** | `e2e-role-user`, `e2e-role-admin` |
| HumanUsers | **Cluster** | `e2e-alice` (login `alice`) |
| Grants | **Cluster** | `e2e-alice-e2e-ui` (`user`) |
| Password secret | **Cluster** | `e2e-human-passwords` |
| OIDC app | **Worktree** | `e2e-ui-dogfood-web` |

Demo login (every local env): **alice / bob / admin · Password1!**

## Values

| Value | Purpose |
|-------|---------|
| `identity.enabled` | Gate identity templates |
| `identity.projectName` | Shared Project name (default `e2e-ui`) |
| `identity.workspace` | Worktree id for OIDC app names only |
| `identity.oidcGeneration` | Bump to rotate generated OIDC client credentials through GitOps prune |
| `identity.demoUsers` | Cluster-shared alice / bob / admin |
| `identity.projectNamespace` | Namespace for Project + Role MRs |
| `identity.humansNamespace` | Namespace for HumanUser MRs |
| `identity.mrNamespace` | OIDC app ns (empty = release / hops `--name`) |
| `identity.instanceLoginV2` | Gitops instance Features (global; primary worktree only) |
| `identity.seedLocalOidcSecret` | Optional explicit residual seed; disabled by default so GitOps cannot erase the login PAT |
| `identity.uiBaseURL` | Optional; else `http://e2e-ui-ui.<namespace>.svc…:5180` |
| `identity.passwordSecret.name` | Shared password secret (default `e2e-human-passwords`) |

```yaml
# gitops/envs/local/ui.yaml
identity:
  enabled: true
  demoUsers: true
  providerConfigRef:
    name: default
    kind: ClusterProviderConfig
```

The Project keeps `projectRoleCheck` enabled. Grant XRs assign `user` to alice
and bob, and `user + admin` to admin. Each Grant resolves its HumanUser and
Project by name from `status.atProvider`; the referenced resources and Grant
must share `identity.projectNamespace`. The chart adds the auth-stack's stable
reference-name label to the Project and HumanUsers. Prefer a
**ClusterProviderConfig** so all identity resources share credentials.

Project, role, human, Grant, and OIDC resources omit live
org/project/user/client UUIDs.
The provider credential selects the organization; `projectIdRef` selects the
shared Project. The OIDC client id/secret come from the per-workspace connection
secret written by the Oidc managed resource.

If a generated OIDC client secret becomes stale, bump
`identity.oidcGeneration`. With the Application's `syncPolicy.prune: true`,
worktree GitOps deletes the previous OIDC MR from its exact-object inventory,
creates the new generation, and rolls the UI onto the matching
`e2e-ui-<workspace>-oidc-conn-g<generation>` Secret. The generation-specific
name makes the replacement pod wait for Crossplane's new credentials instead
of starting with data from the previous generation's Secret.

`OIDC_AUDIENCE` stays empty for generic Zitadel role scopes. The residual
`e2e-ui-oidc` secret contains only `AUTH_SECRET` and
`ZITADEL_SERVICE_USER_TOKEN`.
