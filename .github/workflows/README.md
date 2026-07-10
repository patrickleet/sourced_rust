# Distributed shared GitHub Actions workflows

Reusable workflows for the **Distributed** framework and for **domain crates**
that implement Distributed aggregates. Prefer these over generic
`unbounded-tech/workflows-rust` quality pipelines so Distributed workloads get
fmt + clippy + test without a third-party runs-on.com dependency.

## Catalog

| Workflow | `workflow_call` | Purpose |
|----------|-----------------|---------|
| [`quality.yaml`](./quality.yaml) | yes | **fmt → clippy → build → test + coverage** (PR sticky comment via cargo-llvm-cov) |
| [`test-all-features.yaml`](./test-all-features.yaml) | yes | Workspace `--all-features` (framework repo) |
| [`integration-*.yaml`](./) | yes | Broker / DB / CLI integration (framework repo) |
| [`on-pr-quality.yaml`](./on-pr-quality.yaml) | entry | This repo’s PR gate |
| [`on-push-main-version-and-tag.yaml`](./on-push-main-version-and-tag.yaml) | entry | This repo’s main → **vnext** tag |
| [`on-v-tag-publish.yaml`](./on-v-tag-publish.yaml) | entry | This repo’s crates.io + `dctl` binary release |

**Version tagging:** `unbounded-tech/workflow-vnext-tag`  
**GitHub Release only (domain crates):** `unbounded-tech/workflow-simple-release` (not owned here)

## Domain crate recipe (private libs)

```yaml
# .github/workflows/on-pr-quality.yaml
name: Rust Quality Pipeline for PRs
on:
  pull_request:
    branches: [main]
jobs:
  quality:
    permissions:
      contents: read
      pull-requests: write  # coverage sticky comment on PRs
    uses: hops-ops/distributed/.github/workflows/quality.yaml@main  # or @feat/shared-workflows while landing
    with:
      cargo_build_args: "--verbose --locked"
      cargo_test_args: "--verbose --locked"
```

```yaml
# .github/workflows/on-push-main-version-and-tag.yaml
name: On Push to Main, Version and Tag For Release
on:
  push:
    branches: [main]
permissions:
  contents: write
  packages: write
jobs:
  quality:
    uses: hops-ops/distributed/.github/workflows/quality.yaml@main
    with:
      cargo_build_args: "--verbose --locked"
      cargo_test_args: "--verbose --locked"
  version-and-tag:
    needs: [quality]
    uses: unbounded-tech/workflow-vnext-tag/.github/workflows/workflow.yaml@v1.21.5
    secrets:
      DEPLOY_KEY: ${{ secrets.DEPLOY_KEY }}
    with:
      useDeployKey: true
      rust: true
```

```yaml
# .github/workflows/on-v-tag-release.yaml
name: On Version Tagged, Create GitHub Release
on:
  push:
    tags: ["v*.*.*"]
permissions:
  contents: write
jobs:
  release:
    uses: unbounded-tech/workflow-simple-release/.github/workflows/workflow.yaml@v2.1.3
    with:
      tag: ${{ github.ref_name }}
      name: ${{ github.ref_name }}
```

**Secrets:** write `DEPLOY_KEY` via `vnext generate-deploy-key --owner … --name …`.

**Pinning:** prefer a released tag once cut (`@vX.Y.Z` or git SHA). During development of quality, `@feat/shared-workflows` is fine.

## Out of scope (for now)

- crates.io publish reusable (framework still uses `unbounded-tech` publish helpers in `on-v-tag-publish.yaml`)
- image / GitOps promote (service scaffolds via `dctl`)
