# e2e-ui API local chart

This standalone chart renders the editable API Deployment and Service for a
Local Workbench Environment. Hops supplies source delivery and watches this
chart; `cargo watch` owns process reload for Rust source changes.

OIDC project / demo humans / web app live in the explicit **UI test-users**
chart (`ui/.gitops/test-users`). When `identity.enabled`, this chart derives
the same generation-specific OIDC connection Secret from the workspace
namespace. Both `OIDC_AUDIENCE` and `OIDC_CLIENT_ID` read its generated
`attribute.client_id`; only the service-user token remains in the residual
`e2e-ui-oidc` Secret. Keep `identity.oidcGeneration` aligned with the UI
Application so an OIDC rotation rolls both workloads onto matching credentials.

Platform AuthStack, ProviderConfig, and the shared PSQLCluster stay under the
project `.gitops/local/cluster/` tree. Cloud images belong to the independent
`.gitops/deploy` chart.
