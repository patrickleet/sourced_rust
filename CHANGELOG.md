### What's changed in v3.0.0

* refactor: API cleanup and renames (pre-release) (#105) (by @patrickleet)

  BREAKING CHANGE: Re-derived on current main (all other review PRs merged) rather than
  rebasing the stale diff, since these mechanical renames touch symbols the
  other PRs restructured. Squashed from the original 7 commits (see PR #105
  history). Pre-release: no compat shims left behind.

  - Delete dead `Committable` trait (zero callers; pre-CommitBatch fossil).
  - Delete `HandlerBuilder` type alias (use `RouteBuilder`).
  - Complete emitter opt-in: delete the `Event` trait; move `LocalEvent`
    into `src/emitter/` behind the feature (now `distributed::emitter::LocalEvent`).
  - Rename `HashMapRepository`/`HashMapOutboxStore` -> `InMemoryRepository`/
    `InMemoryOutboxStore`, module `hashmap_repo` -> `in_memory_repo`, and
    `tests/hashmap_repository_conformance` -> `tests/in_memory_repository_conformance`,
    matching every other in-memory default. Includes the CLI scaffold template.
  - Rename `src/bus/rabbit_bus.rs` -> `src/bus/rabbitmq_bus.rs` to match the
    `rabbitmq` feature and `rabbitmq.rs` sibling.
  - Prune adapter plumbing from the crate root (quick-start API at root,
    plumbing under module path): outbox adapter constants
    (SOURCED_METADATA_PREFIX, DEFAULT_OUTBOX_SOURCE_*) now only under
    `distributed::outbox_worker::*`; read_model load-graph/query plumbing
    (ReadModelLoadGraph/Request, ReadModelQueryCapabilities, ReadModelWorkspace,
    ReadModelIncludeRows, ReadModelLoadBuilder) now only under
    `distributed::read_model::*`. Kept `RelationalReadModelIncludes` at root
    (the ReadModel derive expands to it). Left #109's `table::` root surface
    as-is (its deliberate decision, out of scope here).
  - `Entity::set_replaying` -> pub(crate) (ReplayGuard covers internal use).

  Skipped #![warn(missing_docs)]: surfaces 518 warnings (mechanical-pass
  infeasible), as in the original PR.


  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v2.3.5...v3.0.0](https://github.com/hops-ops/distributed/compare/v2.3.5...v3.0.0)
