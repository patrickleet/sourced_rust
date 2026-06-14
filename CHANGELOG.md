### What's changed in v1.9.0

* refactor: public API hygiene — non_exhaustive, re-export pruning, crate docs (#86) (by @patrickleet)

  * refactor: public API hygiene — non_exhaustive, re-export pruning, crate docs

  API-affecting cleanup that is free at 0.1.0 and expensive later (2026-06-10
  review). Root re-export pruning and #[non_exhaustive] additions change the
  public surface; reachability is preserved under module paths.

  - #[non_exhaustive] sweep on the remaining growing types: ReadModelError,
    HandlerError, ConsumerAckKind, KnativeIntegrationKind, TransportCapabilities,
    and OutboxMessage. Added TransportCapabilities::new(..) so external transport
    authors can still build a custom capability profile now that the struct
    literal is blocked. OutboxMessage keeps its new()/create()/encode()/Default
    constructors. NOTE: RepositoryError and LockError were intentionally NOT
    re-touched — #[non_exhaustive] was already added to them in #83.

  - Crate-level docs: #![doc = include_str!("../README.md")] so docs.rs renders
    the README. README rust fences are tagged `rust,ignore` (the snippets are
    deliberately abbreviated fragments referencing app-specific types, per the
    README's own "Example Conventions" note); the Project Structure tree is now
    `text`. cargo test --doc compiles clean. The real drift-catching fix is in
    the in-source doctests: the stale microsvc examples that showed a SYNC
    dispatch / `fn handle` are corrected to async (`dispatch(..).await`,
    `pub async fn handle`, closures returning a future) in microsvc/mod.rs,
    context.rs, and service.rs.

  - Pruned crate-root re-exports: the low-level outbox row plumbing
    (outbox_message_insert_plan, outbox_message_row_values) and the whole
    table::* surface are no longer re-exported at the crate root. They stay
    reachable under distributed::outbox::* and distributed::table::* (the outbox
    and outbox_worker modules are now `pub`). The documented quick-start API is
    unchanged. Internal/test references were repointed to the module paths.

  - Made BusTopologyConfig and the namespace/consumer-group validators
    (validate_namespace, validate_consumer_group, resolve_consumer_group,
    DEFAULT_BUS_NAMESPACE, MAX_TOPOLOGY_NAME_LEN) pub and re-exported from `bus`,
    matching validate_stable_message_id. Third-party transports can now reuse the
    portable naming rules, and this removes the dead-code warnings that appeared
    under --no-default-features and partial feature combos.

  - Resolved the dual outbox publisher story by documenting the boundary (no
    parallel hierarchies introduced): AsyncMessagePublisher (BusPublisher over a
    Bus, wired by service.with_bus) is THE production extension point; the sync
    OutboxPublisher/OutboxWorker/LogPublisher trio is rustdoc'd as the dev/test
    drain path. Folding/retiring the sync trait was deferred — it is exercised by
    tests/todos and retiring it is out of scope for a hygiene PR.

  Verified: cargo fmt --all --check; cargo build for default, --no-default-features,
  postgres+nats, sqlite+rabbitmq+http (all zero warnings, no topology dead code);
  cargo test --doc; cargo test and cargo test --features sqlite (all green).

  Implements [[tasks/public-api-docs-hygiene]]

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

  * fix: keep TableSchemaRegistry re-exported at crate root

  The root re-export pruning in this PR was too aggressive: it dropped
  `table::*` (including `TableSchemaRegistry`) and the outbox row-plumbing
  fns from the crate root, but `TableSchemaRegistry` is consumed by the
  crate's own integration tests via `distributed::TableSchemaRegistry`
  (tests/postgres_repository/main.rs) and is the entry point downstream
  users build before bootstrapping table migrations. A symbol that the
  integration tests import through the crate root is effectively public
  API, so it must stay re-exported.

  The local verification used `--features sqlite`, which does not compile
  the postgres/all-features test crates, so the unresolved-import breaks
  were missed in CI (coverage `--all-features` and postgres jobs).

  Re-add only `TableSchemaRegistry` to the crate root. The remaining
  table primitives stay reachable under `distributed::table::*` (no
  test/external root consumers), and the genuinely internal outbox row
  plumbing (`outbox_message_insert_plan`, `outbox_message_row_values`)
  stays pruned — confirmed zero consumers outside `src/outbox/`.

  All test targets now compile, including the postgres and all-features
  (librdkafka) coverage targets that previously failed.

  Refines [[tasks/public-api-docs-hygiene]]

  ---------

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>

* refactor: deduplicate postgres/sqlite read-model layer into sqlx_repo (#89) (by @patrickleet)

  * refactor: hoist pure read-model helpers into sqlx_repo (phase 1)

  Move the 17 byte-identical, sqlx-free read-model/validation functions
  (plus the shared `IncludeSpec`) out of postgres_repo and sqlite_repo into
  a new `sqlx_repo::read_model` submodule and import them from both backends.

  These functions (schema resolution, write-plan validation, row-version
  arithmetic, key/patch reconciliation, identifier quoting, etc.) carried no
  database types, so each fix previously had to land twice and could drift.

  Net ~283 lines removed (two ~316-line copies deleted, one ~336-line shared
  copy added). No public API change; no behavior change — every function was
  verified byte-identical before the move.

  * refactor: make the relational write path generic over sqlx::Database (phase 2)

  The upsert/patch/delete/insert/update/row_version `*_in_tx` functions were
  byte-identical between postgres_repo and sqlite_repo modulo the `Postgres`/
  `Sqlite` type parameter — `QueryBuilder<DB>` already renders each dialect's
  placeholder style. Hoist them into `sqlx_repo::read_model` as free functions
  over `DB: SqlxReadModelBackend`.

  Only the genuinely dialect-specific bits stay per-backend, behind the one
  `SqlxReadModelBackend` trait (two impls, one line per method): value binding
  (`Bool`/typed-`NULL` — Postgres binds native `bool` + `::jsonb`/`::timestamptz`
  casts; SQLite stores bools as `i64` and collapses integer/bool NULLs), the
  `rows_affected` accessor (sqlx exposes it only as an inherent method per
  backend), and the backend/storage label strings. The unavoidable sqlx
  executor/encode/decode bounds are stated explicitly on each write function —
  no abstraction layer, no wrapper types.

  Net ~263 lines removed (two ~470-line copies replaced by one generic copy plus
  two small trait impls). No public API change; behavior verified identical on
  real Postgres and SQLite.

  * refactor: make the relational load path generic over sqlx::Database (phase 3)

  The relational read path (load_relational_row_by_key, the relationship/
  has_many/belongs_to loaders, the SELECT builder, ORDER BY, and row→Versioned
  mapping) was mirrored between postgres_repo and sqlite_repo. Hoist it into
  `sqlx_repo::read_model` as free functions over `DB: SqlxReadModelBackend`.

  The two genuinely dialect-specific reads stay per-backend as trait methods:
  `row_value` (Postgres has a native BOOLEAN; SQLite stores booleans as INTEGER
  and decodes `value != 0`) and `push_select_column` (Postgres casts JSON/
  Timestamp to `::text` on SELECT so they decode as String; SQLite stores them as
  text already). The fetch/decode sqlx bounds are stated explicitly on each load
  function, mirroring the write path — no abstraction layer.

  Net ~126 lines removed. No public API change; behavior verified identical on
  real Postgres and SQLite.

  Implements [[tasks/sqlx-read-model-dedup]]

* feat: DRY code (#91) (by @patrickleet)

  * refactor: reduce duplicated source code

  * refactor: make outbox APIs async-only

  * docs: align async-only API references

  * fix: address outbox review comments

* feat: Add SQLite bus transport (#90) (by @patrickleet)

  * feat: add sqlite bus transport

  * fix: run cli integration on prs

  * fix: address sqlite bus review hardening

  Implements [[tasks/sqlite-bus-transport-impl]]

  * docs: include sqlite bus in transport status

  Implements [[tasks/sqlite-bus-transport-impl]]


See full diff: [v1.8.2...v1.9.0](https://github.com/hops-ops/distributed/compare/v1.8.2...v1.9.0)
