# `.gitops/local/cluster` — shared Cluster desired state

This tree is reconciled once for the named `e2e-ui` Cluster. It owns shared
platform resources; registered Environments own application workloads in their
own namespaces.

```text
.gitops/local/
  cluster.yaml           # Cluster identity, providers, mountRoot, manifest path
  environment.yaml       # reusable checkout/worktree Environment
  cluster/               # this shared desired-state tree
    providers/           # Crossplane Provider packages and runtime configs
    providerconfigs/     # non-secret provider configuration
    configurations/      # Crossplane Configuration packages
    stacks/              # shared platform XRs
    psql/                 # one shared PSQLCluster
    secrets/              # Vault/ESO stack and RBAC
    auth/                 # auth residual resources
```

Start or resume the Cluster from `tests/e2e-ui`:

```bash
hops local gitops cluster ./.gitops/local/cluster.yaml
```

The Cluster definition declares `clusterProvider: kind`,
`dockerProvider: dory`, and a same-path `mountRoot` for this project. The
Cluster controller applies this tree, waits for dependencies, watches changes,
and owns exact-inventory pruning. No committed apply waves, source generation,
or restart counters are required.

Register an application Environment separately:

```bash
hops local gitops environment ./.gitops/local/environment.yaml --name alice
```

`environment.yaml` lists application roots. Each root defaults to its
`.gitops/local` chart; the explicit test-user deploy selects
`ui/.gitops/test-users`. Cloud `.gitops/promote` and `.gitops/deploy` charts do
not participate in ordinary local reconciliation.

## Ownership boundary

- Cluster: Providers, Configurations, ProviderConfigs, shared Auth/Secret/PSQL
  stacks, and the shared PSQLCluster.
- Environment: rendered local application charts and explicit optional charts.
- Hops substrate: container engine, bare Kubernetes API, controller process,
  kubeconfig, project mount, inotify, and host networking.

Local credentials come only from the gitignored `secrets/vault/` input selected
by `Cluster.spec.secretSync`. See [SECRETS.md](./SECRETS.md).
