# e2e-ui API chart (workbench)

Renders:

- **Deployment / Service** (`appRuntime: cluster-dev` \| `package`)
- **Optional PSQLCluster** when `database.enabled: true`

OIDC project / demo humans / web app live on the **UI** chart
(`ui/.gitops/deploy` `identity.*`). When `identity.enabled`, this chart derives
the same generation-specific OIDC connection Secret from the workspace
namespace. Both `OIDC_AUDIENCE` and `OIDC_CLIENT_ID` read its generated
`attribute.client_id`; only the service-user token remains in the residual
`e2e-ui-oidc` Secret. Keep `identity.oidcGeneration` aligned with the UI
Application so an OIDC rotation rolls both workloads onto matching credentials.

Platform AuthStack + ProviderConfig stay under `gitops/cluster/`.
