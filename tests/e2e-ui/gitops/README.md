# e2e-ui local gitops

```text
gitops/
  cluster/          # fixture CP tree (real meta repos: put this at meta root)
  envs/
    local/          # app Applications → worktree namespaces
```

```bash
# Terminal 1: cluster apply/watch
hops local gitops cluster ./gitops/cluster \
  --cluster-provider kind --docker-provider dory \
  --cluster-name hops --context kind-hops

# Terminal 2: per-workspace apps apply/watch
hops local gitops worktree ./gitops/envs/local --name alice \
  --cluster-provider kind --docker-provider dory \
  --cluster-name hops --context kind-hops

# One-shot (CI / scripts): add --once
# hops local gitops cluster ./gitops/cluster --once
```

On multi-project metas: put `gitops/cluster` at the **meta root**; projects keep
`.gitops/deploy` charts; env Application YAMLs point at those charts.
