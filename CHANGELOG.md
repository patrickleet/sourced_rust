### What's changed in v1.6.0

* feat: distributed_cli (dsvc) — service CLI + Atlas schema output (#74) (by @patrickleet)

  * feat: render Atlas Operator AtlasSchema resources from desired-state SQL

  Add distributed_tooling::render_atlas_schema — a pure producer that wraps
  desired-state schema SQL (e.g. DistributedProjectManifest::sql_statements) into an
  AtlasSchema (db.atlasgo.io/v1alpha1) custom resource. DB URL via a Secret
  reference (GitOps) or inline (dev), optional devURL, SQL as a literal block
  scalar. The caller prints/redirects the result anywhere (stdout → any file or a
  separate schema repo); the crate deliberately does not pick a .gitops/ location.

  Implements [[tasks/atlas-operator-schema-gitops]]

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>

  * feat: distributed_cli (dsvc) — fold generation in, add schema --format atlas

  Make Distributed's service tooling a single in-workspace crate that is both a
  binary (`dsvc`) and a library, eliminating cross-repo release coordination:

  - Fold the former `distributed_tooling` crate (pure scaffold + Atlas generation)
    into `distributed_cli` as internal `generate`/`atlas` modules; the public
    generation API is re-exported from the crate root.
  - Add the command surface (`cli` module): scaffold / describe / schema, ported
    from hops-cli's service adapter, with `run(&ServiceArgs)` as the dispatcher.
  - `dsvc schema --format atlas` renders an AtlasSchema resource to stdout
    (flag-configured: --name/--namespace/--db-secret/--db-secret-key/--db-url/
    --dev-url); default --format sql is unchanged.
  - The library exposes `ServiceArgs` + `run` so another CLI (hops) can mount the
    commands under `hops service` and dispatch — re-exporting, not reimplementing,
    so a new flag here reaches hops on a plain `cargo update`.
  - Publish workflow: replace publish-tooling with publish-cli.

  distributed_tooling 1.5.0 stays on crates.io for the already-merged hops-cli until
  it migrates to depend on distributed_cli.

  Implements [[tasks/atlas-operator-schema-gitops]]

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>

  * test: distributed_cli integration tests + CI job

  Drive the real `dsvc` binary end-to-end:
  - cli_scaffold.rs: `dsvc scaffold` to a temp dir, assert the generated tree
    (fast; pure generation + filesystem; runs in normal `cargo test`).
  - cli_manifest.rs: `#[ignore]`d harness e2e — `dsvc describe`,
    `schema --dialect postgres`, and `schema --format atlas` against a committed
    `orders-service` fixture (a standalone crate with its own `[workspace]` and a
    `#[derive(ReadModel)]` registered in `distributed_manifest()`). Ignored by
    default because they compile the fixture via nested cargo.
  - integration-distributed-cli.yaml: reusable workflow running
    `cargo test -p distributed_cli -- --include-ignored`; referenced from the
    push-to-main pipeline and gating version-and-tag.

  Implements [[tasks/distributed-cli-integration-tests]]

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>

  * fix: validate Kubernetes name format for AtlasSchema name/namespace

  render_atlas_schema now rejects names that aren't RFC-1123 labels (lowercase
  letters, digits, hyphens; no leading/trailing hyphen) for both metadata.name and
  metadata.namespace, instead of only checking non-empty. This fails at generation
  with a clear message rather than emitting YAML the API server would reject — and
  guards against characters (newlines, colons, quotes) that would break the
  document itself. Addresses CodeRabbit review on PR #74.

  Implements [[tasks/atlas-operator-schema-gitops]]

  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>

  ---------

  Co-authored-by: Claude Opus 4.8 <noreply@anthropic.com>


See full diff: [v1.5.7...v1.6.0](https://github.com/hops-ops/distributed/compare/v1.5.7...v1.6.0)
