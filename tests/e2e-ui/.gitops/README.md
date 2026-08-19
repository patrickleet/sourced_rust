# e2e-ui local GitOps

This directory is the project-level Local Workbench contract:

```text
.gitops/
  local/
    cluster.yaml       # durable named Cluster
    environment.yaml   # reusable definition for this checkout/worktree
    cluster/           # shared Cluster desired state
```

Application roots own their own charts:

```text
api/.gitops/local/       # editable local workload (default deploy chart)
api/.gitops/deploy/      # packaged cloud workload
api/.gitops/promote/     # optional cloud promotion action
ui/.gitops/local/
ui/.gitops/deploy/
ui/.gitops/promote/
ui/.gitops/test-users/   # explicit optional Environment deploy
```

```bash
# Terminal 1
hops local gitops cluster ./.gitops/local/cluster.yaml

# Terminal 2
hops local gitops environment ./.gitops/local/environment.yaml --name e2e
```

Both definitions are Kubernetes-shaped YAML. The Cluster controller watches
the Cluster definition and tree, registered Environment definitions, each
resolved deploy chart, and source paths emitted by local workloads. A deploy
path names an application root; its chart defaults to `.gitops/local`.

Another Git worktree uses the same committed `environment.yaml` and a distinct
runtime name:

```bash
cd .worktrees/feature-auth/tests/e2e-ui
hops local gitops environment ./.gitops/local/environment.yaml --name feature-auth
```

Unregister it without editing committed files:

```bash
hops local gitops environment --name feature-auth --down
```
