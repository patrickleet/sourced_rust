### What's changed in v3.3.3

* ci: add reusable quality and github-release workflows (by @patrickleet)

  Introduce Distributed-owned shared Actions for library/domain crates:
  - quality.yaml: fmt + clippy -Dwarnings + build + test (no runs-on.com)
  - github-release.yaml: tag → GitHub Release only (no crates.io)

  Wire this repo's PR and main entry workflows to the new quality gate.
  Keep vnext-tag and crates.io publish helpers as-is for framework releases.

  Document the domain-crate recipe in .github/workflows/README.md. Callers
  can pin @feat/shared-workflows until a release tag is cut.

* ci: drop custom github-release; use workflow-simple-release (by @patrickleet)

  Domain crates and docs should create GitHub Releases via
  unbounded-tech/workflow-simple-release. Shared surface in this repo
  stays quality (and framework-specific integration/publish entrypoints).

* ci(quality): measure coverage with llvm-cov and sticky PR comment (by @patrickleet)

  When cargo_coverage is true (default), run tests under cargo-llvm-cov,
  upload lcov, write a job summary, and post a sticky PR comment with the
  coverage summary (requires pull-requests: write on the caller job).

  Callers should grant pull-requests: write. Framework PR/main entrypoints
  pass --workspace for multi-crate coverage.

* ci(quality): post clean TOTAL coverage % on PRs (by @patrickleet)

  Stop teeing cargo llvm-cov compile/test logs into the sticky comment.
  Run tests + lcov first, then a quiet `cargo llvm-cov report --summary-only
  --color never` for the PR body. Headline is TOTAL percentage; full table
  is collapsible; link to the workflow run for the lcov artifact.

* ci: keep framework entry pipelines off consumer quality.yaml (by @patrickleet)

  quality.yaml is the reusable contract for Distributed domain crates and
  other consumers (fmt/clippy/test/coverage). This monorepo's on-pr-quality
  and on-push-main stay on unbounded-tech quality plus integration jobs —
  dogfooding the consumer workflow here was never the intent and fails on
  the framework workspace shape.


See full diff: [v3.3.2...v3.3.3](https://github.com/hops-ops/distributed/compare/v3.3.2...v3.3.3)
