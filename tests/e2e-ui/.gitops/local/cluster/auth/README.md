# Auth (shared local Cluster)

The Cluster tree owns the shared AuthStack, Zitadel provider package and
ProviderConfig, and the AuthStack masterkey ExternalSecret. The reusable
Environment owns only per-Environment OIDC/test identity resources through the
explicit `ui/.gitops/test-users` deploy.

```bash
hops local gitops cluster ./.gitops/local/cluster.yaml
hops local gitops environment ./.gitops/local/environment.yaml --name e2e
```

No live organization, project, user, or client IDs are committed. Provider
credentials and the masterkey originate in the gitignored local secret input
and reach Kubernetes through Vault/ExternalSecret resources.
