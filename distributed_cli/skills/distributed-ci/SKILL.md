---
name: distributed-ci
description: Set up CI, release workflows, and GitOps promotion for a Distributed service with dctl scaffold flags (--github, --gitops, --gitops-promote). Use when configuring pipelines, previews, releases, or deploy automation.
---

# CI and GitOps for Distributed services

`dctl scaffold` generates the whole delivery pipeline — Helm deploy chart,
GitHub Actions workflows, and promotion charts — from flags. Prefer
regenerating/extending these artifacts over hand-writing pipeline YAML.

## Flag → artifact map

| Flag | Generates |
|---|---|
| `--gitops` | `.gitops/deploy/` Helm chart (Deployment + Service for `--transport http`; Knative Service + Brokers + Triggers for `--transport knative`) |
| `--gitops-promote <argo\|flux>` | `.gitops/promote/` chart with an Argo CD `Application` or Flux `GitRepository` + `HelmRelease` |
| `--github OWNER/REPO` | `.github/workflows/version.yaml` + `release.yaml`, and a post-create `gh repo create` (private) if the repo does not exist |
| `--github-preview OWNER/REPO` | `.github/workflows/preview.yaml` + `.gitops/preview/helm` promotion chart |
| `--github-promote OWNER/REPO` | `.github/workflows/promote.yaml` + `.gitops/promote/helm` promotion chart |

The `.gitops/deploy` chart is emitted whenever **any** of the flags above is
set, because the promotion charts target `.gitops/deploy`.

## How the release flow fits together

1. **`version.yaml`** (push to `main`) — `unbounded-tech/workflow-vnext-tag`
   computes the next semver from conventional commits, tags `v*.*.*`, and
   yq-patches the deploy chart in the same commit:
   `.gitops/deploy/values.yaml` `.image.tag` → `v<version>`, and
   `.gitops/deploy/Chart.yaml` `.version`/`.appVersion`.
   Consequence: **use Conventional Commits** (`feat:`, `fix:`, ...) or no
   version tag is produced.
2. **`release.yaml`** (on `v*.*.*` tags) — creates the GitHub release via
   `unbounded-tech/workflow-simple-release`.
3. **`promote.yaml`** (on `v*.*.*` tags) — promotes the tagged version into the
   permanent environment repo via
   `unbounded-tech/workflows-gitops/argocd-promote-helm`, rendering
   `.gitops/promote/helm` with `image.tag = <tag>` and opening a PR against the
   environment repository.
4. **`preview.yaml`** (PRs labeled `preview`) — promotes
   `pr-<number>-<head-sha>` image tags into the preview environment repo as a
   promotion PR. The label gate means previews are opt-in per PR.

## Requirements the generated workflows assume

- **Secrets**: preview/promote workflows need
  `secrets.GH_ORG_ACTIONS_REPO_WRITE_PACKAGES` (a PAT with repo write +
  packages) passed as `GH_PAT`; `version.yaml` uses `secrets: inherit` and a
  deploy key (`useDeployKey: true`).
- **Image repository**: defaults to `ghcr.io/<owner>/<repo>` (lowercased) when
  `--github` is set, else `ghcr.io/hops-ops/<name>`. The deploy chart's
  `values.yaml` and the promotion workflows must agree — they do when
  generated together.
- **Environment repositories** (`--github-preview` / `--github-promote`) are
  Argo CD-watched GitOps repos; the workflows open promotion PRs against them
  rather than pushing directly.
- The generated Deployment/Knative Service sets `BIND_ADDR=0.0.0.0:3000` and,
  when `--bus <kind>` was given, `HOPS_BUS=<kind>` plus a `bus.kind` entry in
  `values.yaml` — keep these in sync if you rename or add env plumbing.

## Gotchas

- `dctl scaffold` **refuses a non-empty directory** without `--force`; adding
  CI to an existing service means running scaffold with `--force` in a clean
  worktree and reviewing the diff, or copying the generated workflows in.
- The three GitHub repos gate independent slices — you can adopt
  `--github` (version/release) without preview/promote environments, and add
  them later.
- Knative transport changes the deploy chart shape entirely (Service +
  Brokers + Triggers, one Trigger per command/event, deduped names). Do not
  hand-add Triggers with names that normalize identically — `kubectl apply`
  rejects duplicates.
- Promotion chart `values.yaml` ships placeholder `repoUrl`/`destination`
  values (`example.invalid`) for the plain `--gitops-promote` variant — set
  them before pointing Argo/Flux at the chart. The `--github-preview`/
  `--github-promote` variants are fully parameterized by the workflows.
- Rendering schema artifacts in CI (SQL / Atlas) is covered by the
  `distributed-schema` skill.

## Reference

Full flag list: `dctl scaffold --help`. The same commands ship as
`hops service scaffold ...` when using the `hops` CLI.
