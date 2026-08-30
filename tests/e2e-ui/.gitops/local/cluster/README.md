# e2e-ui Cluster GitOps tree

This is the project-level Cluster desired state. The directory names describe
ownership only; Hops resolves Kubernetes readiness and exact object identity,
then records an inventory before pruning a removed source.

```text
cluster/
  registry/        # local package registry KRM (TLS bytes stay external)
  providers/       # Provider, ProviderConfig, DRC, and provider RBAC
  configurations/  # Crossplane Configuration packages
  functions/       # Crossplane Function packages
  platform/        # local platform KRM (PSQLStack + serving PSQLCluster + AuthStack)
  shared/          # cluster-shared resources
  rbac/            # cluster-level access needed by local packages
```

The pinned Crossplane seed is declared in `../cluster.yaml`; it is the only
imperative substrate prerequisite. Application charts live under their own
`.gitops/local` and `.gitops/deploy` roots.

The environment's identity chart uses the auth-stack `HumanUser` and `Grant`
XRs. The installed auth-stack package must provide those APIs; while that
package is being developed locally, install it from its source checkout with
`hops config install` before reconciling `../environment.yaml`.
