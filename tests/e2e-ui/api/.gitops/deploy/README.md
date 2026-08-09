# e2e-ui API chart (workbench)

Renders:

- **Deployment / Service** (`appRuntime: cluster-dev` \| `package`)
- **Optional PSQLCluster** when `database.enabled: true`

OIDC project / demo humans / web app live on the **UI** chart
(`ui/.gitops/deploy` `identity.*`). This chart only consumes residual OIDC
settings (`OIDC_ISSUER`, `OIDC_AUDIENCE`, shared `e2e-ui-oidc` secret keys) so
Bearer tokens map engine roles.

Platform AuthStack + ProviderConfig stay under `gitops/cluster/`.
