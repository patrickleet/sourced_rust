### What's changed in v1.8.2

* refactor: shared-helpers dedup batch (commit validation, bus helpers, test helpers, macro codegen) (#88) (by @patrickleet)

  * refactor: move commit-batch validation to ungated repository home

  The six commit-batch validation fns (reject_duplicate_streams,
  reject_duplicate_outbox_messages, validate_entity_id_matches_identity,
  validate_prepared_appends, validate_supported_event_codec,
  validate_snapshot_identity) were duplicated byte-for-byte between
  src/sqlx_repo/mod.rs and src/hashmap_repo/repository.rs. They have no
  sqlx dependency; only the cfg gate forced the copy.

  Move them to a single ungated home, src/repository/validation.rs, since
  they validate repository-level types (StreamWrite, PreparedEventAppend,
  OutboxMessage, CommitBatch). All three backends (hashmap, sqlite,
  postgres) now import the one copy via crate::repository, so the
  definition of a valid CommitBatch cannot drift between backends.

  No behavior change: the moved fns are byte-identical to the originals;
  the error taxonomy work from #83 in sqlx_repo is untouched.

  Implements [[tasks/shared-helpers-dedup-batch]]

  * refactor: dedup bus kind/retryable/strip-prefix helpers

  Three small transport helpers were copy-pasted across the bus adapters:

  - kind_str/kind_from_str (5x: nats, kafka, rabbitmq, postgres_bus,
    knative_bus) -> MessageKind::as_str / MessageKind::from_str_lossy on
    the type's owning module, src/bus/message.rs.
  - retryable(context, err) (5x: nats, kafka, rabbitmq, nats_bus,
    rabbit_bus) -> one pub(crate) fn in src/bus/error.rs, where
    TransportError lives.
  - subject/topic/routing-key prefix stripping (3x: nats, kafka,
    rabbit_bus) -> strip_address_prefix free fn next to Message.

  Each helper now has one definition in the module that owns the concept.
  The crate-internal re-exports and the retryable/strip fns are gated to
  the transport features that consume them so non-transport builds stay
  warning-free. No behavior change: the string tokens, the
  event-default-on-unknown parse, and the "{context}: {err}" format are
  identical to the originals; #80's postgres corrupt-row handling and the
  in-flight NATS publish path are untouched beyond the helper swap.

  Implements [[tasks/shared-helpers-dedup-batch]]

  * refactor: extract shared aggregate_impl_tokens for macros

  aggregate! (expand_aggregate) and #[sourced] (expand_sourced) emitted a
  byte-for-byte identical 'impl distributed::Aggregate' block: same
  ReplayError = String, same entity/entity_mut/replay_event bodies, and
  the same optional aggregate_type and upcasters methods. Only the replay
  match arms are built differently upstream.

  Extract one aggregate_impl_tokens(type_name, entity_field,
  aggregate_type_method, replay_arms, upcasters_method) helper that emits
  the impl block; both macros now call it. This prevents the replay
  semantics of the two entry points from drifting. Works with the post-#82
  expand_* -> syn::Result shape; each caller still places #upcaster_wrappers
  where it already did.

  Verified byte-identical: a golden dump of both macros' to_string() output
  (covering no-payload, single-arg, multi-arg replay arms and aggregate_type)
  matched the pre-refactor output exactly. The large line delta is rustfmt
  re-indenting the two replay-arm closures after collecting them to a Vec.

  Implements [[tasks/shared-helpers-dedup-batch]]

  * refactor: hoist shared broker-test helpers into transport_conformance

  The four broker test mains each redefined the same scaffolding:
  recording_for / named_recording_for (kafka, rabbitmq, postgres),
  run_token, and the run-token-based unique (nats, rabbitmq). Move the
  genuinely-identical helpers into tests/transport_conformance/mod.rs and
  include it via the established #[path] mechanism (the in-memory harness
  already does this).

  The per-transport scenarios stay in their own files, and load-bearing
  differences are preserved: kafka keeps its own nanos-based unique (it
  persists topics across runs), and each transport keeps its own env-skip
  helper (the env var name and message differ). #80's postgres corrupt-row
  test is untouched.

  Validated against real brokers via docker compose: postgres 9, rabbitmq
  5, nats 5, kafka 5 tests pass; transport_in_memory still passes (12).
  Net ~150 duplicated lines removed in favor of one shared copy.

  Implements [[tasks/shared-helpers-dedup-batch]]

  * refactor(test): use shared recording helpers in NATS transport test

  The NATS test still defined local recording_service/named_recording_service
  that were byte-identical to the shared recording_for/named_recording_for in
  transport_conformance — import them (aliased to the local names) and delete the
  duplicates, completing the test-helper dedup.

  Addresses CodeRabbit review on PR #88.

  Refines [[tasks/shared-helpers-dedup-batch]]

  * refactor(test): call shared recording helpers by canonical name in NATS test

  Drop the alias and call recording_for/named_recording_for directly at the call
  sites, matching the kafka/rabbitmq/postgres transport tests for consistency.

  Refines [[tasks/shared-helpers-dedup-batch]]


See full diff: [v1.8.1...v1.8.2](https://github.com/hops-ops/distributed/compare/v1.8.1...v1.8.2)
