# e2e-ui local gitops

```text
gitops/
  cluster/          # fixture CP tree (real meta repos: put this at meta root)
  envs/
    local/          # app Applications → worktree namespaces
  env/local/        # deprecated alias of envs/local
```

```bash
# Bootstrap + cluster gitops apply/watch (stays in foreground = gitops cluster)
hops local start --backend dory --gitops ./gitops/cluster

# Or cluster watch alone if CP already started
# hops local gitops cluster ./gitops/cluster

# Per-worktree apps (watches by default) — separate terminal
hops local gitops worktree ./gitops/envs/local --name dogfood

# One-shot (CI / scripts): add --once
# hops local gitops cluster ./gitops/cluster --once
```

On multi-project metas: put `gitops/cluster` at the **meta root**; projects keep
`.gitops/deploy` charts; env Application YAMLs point at those charts.
