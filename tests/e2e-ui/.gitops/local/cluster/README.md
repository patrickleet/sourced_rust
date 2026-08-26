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
  platform/        # local platform KRM
  shared/          # cluster-shared resources
  rbac/            # cluster-level access needed by local packages
```

The pinned Crossplane seed is declared in `../cluster.yaml`; it is the only
imperative substrate prerequisite. Application charts live under their own
`.gitops/local` and `.gitops/deploy` roots.
