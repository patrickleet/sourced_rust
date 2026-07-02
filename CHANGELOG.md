### What's changed in v2.2.1

* chore(deps): update unbounded-tech/workflow-vnext-tag action to v1.21.5 (#93) (by @renovate[bot])

  Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>

* chore: rename distributed cli binary to dctl (#98) (by @patrickleet)

* ci: gate releases on all-features tests and close coverage gaps (#101) (by @patrickleet)

  The release gate never exercised the sqlite, emitter, http, or grpc
  test suites: the shared quality job runs cargo test with default
  features (= none), so every #![cfg(feature = ...)] test target
  compiled to empty. PostgresLockManager had zero CI execution because
  the postgres integration job omitted the sql_lock_manager target.

  - Add a reusable all-features workflow with two jobs:
    - all-features: cargo test --workspace --all-features --all-targets
      (env-gated broker/db tests self-skip without services)
    - each-feature: cargo hack check --workspace --each-feature to
      catch missing cfg fences (only no-features and all-features
      combos were compiled before)
  - Wire it into the PR quality pipeline and the release gate's needs
  - Add --test sql_lock_manager to the postgres integration job
  - Delete orphaned tests/read_models/ (no main.rs, no #[path] refs;
    cargo silently never compiled it)


  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v2.2.0...v2.2.1](https://github.com/hops-ops/distributed/compare/v2.2.0...v2.2.1)
