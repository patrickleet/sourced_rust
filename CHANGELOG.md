### What's changed in v2.2.3

* test: compile scaffold output e2e; fix route-bundle template drift (#103) (by @patrickleet)

  * fix: update scaffold templates to the route-bundle Service API

  The scaffolded service.rs still used the removed register_handlers! macro,
  the pre-route-bundle generic Service<ServiceRepo>, and
  Service::new().with_repo(repo) — none of which exist since the typed
  microsvc route bundles landed — so every scaffolded project failed to
  compile. Rebuild the service around distributed::routes!(
  Routes::new().with_dependencies(repo), ...) and Arc<Service>, and pin the
  Knative scaffold's axum to 0.8 to match the Router the distributed crate
  hands to axum::serve.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * test: compile scaffold output and cover CLI variants and error paths

  - cli_scaffold_compile.rs: scaffold into a temp dir pointing the generated
    distributed dependency at this workspace, then cargo check the output
    (ignored by default like the manifest-harness tests). Covers the
    http+postgres+model+read-models surface and the knative+in-memory branch.
  - cli_scaffold.rs: matrix-lite assertions for every --store, --transport,
    --bus, and --gitops-promote variant, plus error paths (non-empty output
    dir without/with --force, describe with a missing manifest).
  - cli_manifest.rs: schema --dialect sqlite happy path.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * refactor: extract manifest-harness codegen from cli.rs into its own module

  Pure move: HarnessOptions/HarnessMode, run_manifest_harness, the inline
  harness Cargo.toml/main.rs templates, cargo-metadata package resolution,
  and entrypoint validation now live in src/manifest_harness.rs beside
  generate/, with their unit tests. cli.rs keeps the clap surface and the
  scaffold side effects; the shared path helpers it owns are pub(crate) for
  the harness. No behavior change.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  ---------

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v2.2.2...v2.2.3](https://github.com/hops-ops/distributed/compare/v2.2.2...v2.2.3)
