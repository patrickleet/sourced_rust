### What's changed in v1.0.0

* chore: use high level macros in most tests (#46)

  * feat: use high level macros in most tests

  * chore: db mapping

* refactor: remove old pattern

* feat(transport): async transport foundation in microsvc::transport

  Establishes the shared async transport layer in three reviewed slices:

  - Core contracts: TransportError (retryable/permanent), FailurePolicy/FailureAction,
    RunOptions/ConsumerDeliveryMode/InboxHook, TransportCapabilities, stable-id rules.
  - Source runner: AsyncMessageSource/ReceivedMessage + run_source (ack after handler
    success, retryable->nack, permanent->failure policy, ack-and-ignore unhandled,
    graceful stop, no swallowed errors).
  - Publisher/outbox bridge: AsyncMessagePublisher, OutboxMessage->Message mapping
    (reserved x-sourced- metadata namespace), OutboxDispatcher (dispatch_ids/
    dispatch_batch sharing claim->publish->complete), claim-by-id across
    HashMap/SQLite/Postgres, From<RepositoryError> for TransportError.

  Not feature-gated; executor-agnostic (no tokio dependency). Verified with
  cargo test --all-features (242 lib unit tests + integration/conformance suites),
  fmt, clippy, and doc-link checks.

  Implements [[tasks/async-transport-core-contracts]],
  [[tasks/async-message-source-runner]], and
  [[tasks/async-message-publisher-outbox]] under
  [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(transport): reusable conformance harness + in-memory reference run

  Adds tests/transport_conformance/mod.rs (adapter-neutral fakes: FakeSource,
  FakeReceived, FakePublisher, recording service, plus source-runner and outbox
  dispatcher contract fns) and the tests/transport_in_memory target that runs the
  full contract against the in-memory reference. Concrete adapters reuse the
  harness via #[path]. Adds OutboxDispatcher::publisher()/store() accessors.

  Implements [[tasks/async-transport-conformance-tests]] under
  [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): Postgres durable receive via OutboxSource

  OutboxSource<S: AsyncOutboxStore> turns any outbox store into an
  AsyncMessageSource (claim -> Message -> settle by row status:
  ack=complete, nack=release-for-retry, dead-letter/park=fail).
  OutboxSource<PostgresOutboxStore> is the Postgres starter transport
  (outbox-backed mode, FOR UPDATE SKIP LOCKED + lease, no new table).

  Adds tests/postgres_transport integration tests (verified against real
  Postgres: drain, concurrent SKIP-LOCKED claim safety, retry, dead-letter),
  wires postgres_transport into the Postgres CI job, and adds RabbitMQ/Kafka/
  NATS services to compose.yaml for local integration testing.

  sqlxmq was evaluated (owner suggestion) and not adopted: its push-based
  JobRegistry conflicts with our pull-based AsyncMessageSource/run_source
  boundary; patterns borrowed per the spec, not the crate.

  Implements [[tasks/postgres-transport-adapter-first-pass]] under
  [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): NATS JetStream adapter + integration tests + CI

  NatsPublisher (publish ack threshold) and NatsJetStreamSource (durable pull
  consumer; ack/nak/term settle) behind the nats feature, over the shared
  AsyncMessagePublisher/AsyncMessageSource/run_source boundary. Stable id +
  metadata ride as headers (Nats-Msg-Id is also the JetStream dedup key).

  Adds tests/nats_transport (verified against nats:2.10 -js: round-trip +
  metadata preservation), a nats CI job, and the nats compose service.

  Implements [[tasks/nats-transport-adapter]] under
  [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): RabbitMQ (AMQP) adapter + integration tests + CI

  RabbitPublisher (publisher-confirm threshold) and RabbitSource (basic_get;
  ack/nack-requeue/reject settle) behind the rabbitmq feature, over the shared
  transport traits. Stable id via the AMQP message_id property, metadata+kind via
  headers.

  Adds tests/rabbitmq_transport (verified against rabbitmq:3.13), a rabbitmq CI
  job (service container), and updates the module docs for the NATS/RabbitMQ
  adapters.

  Implements [[tasks/rabbitmq-transport-adapter]] under
  [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): Knative CloudEvents HTTP ingress

  cloud_events_router parses binary + structured CloudEvents into the canonical
  Message and calls Service::dispatch_message (the same boundary as run_source).
  HTTP response is the ack: 200 success, 503 retryable, 422 permanent, 400
  malformed. knative_triggers() renders Trigger YAML from subscription_plan().
  Retry/DLQ is platform-managed by Knative here (not this crate's FailurePolicy).

  Adds tests/knative_cloudevents (6 in-process HTTP integration tests). Behind the
  http feature.

  Implements [[tasks/knative-cloudevents-ingress]] under
  [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): Kafka adapter + integration tests + CI

  KafkaPublisher (acks=all producer ack threshold) and KafkaSource (consumer
  group, auto-commit off; ack=commit offset, nack=seek-back, dead-letter/park=
  commit-skip) behind the kafka feature (rdkafka/librdkafka via cmake). recv rides
  through transient broker-transport errors within a fetch-timeout budget.

  Adds tests/kafka_transport (verified against apache/kafka:3.8.0 KRaft), a kafka
  CI job (KRaft service container), and the kafka compose service.

  Implements [[tasks/kafka-transport-adapter]] under
  [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* ci: extract integration tests to reusable workflows; run on push-to-main

  Moves the postgres/nats/rabbitmq/kafka integration jobs into reusable
  workflow_call files (.github/workflows/integration-*.yaml) referenced via local
  ./ paths from both on-pr-quality and on-push-main-version-and-tag. The push-to-
  main pipeline now runs all broker integration tests and gates version-and-tag on
  them. Validated with actionlint.

  Relates to [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* docs(transport): add async transports guide

  docs/async-transports.md documents the transport layer: core contracts, the
  two confirmation thresholds, the source runner, the publisher/outbox dispatcher,
  all five adapters (in-memory, Postgres, NATS, RabbitMQ, Kafka, Knative), and how
  to run the conformance + broker integration tests.

  Progresses [[tasks/transport-docs-examples-cutover]] under
  [[tasks/async-transport-implementation]].

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): add Bus/BusConsumer facade + InMemoryBus

  Introduce the ergonomic servicebus-style surface over the async
  transport traits: `Bus` (produce — send/publish/send_message/
  publish_message) and `BusConsumer` (consume — listen/subscribe,
  generic over the service data, deriving message names from the
  service's command/event handlers and running through run_source).

  Knative will implement only `Bus` (it consumes via generated Triggers
  + the HTTP ingress); pull transports implement both.

  InMemoryBus is the dev/test reference implementation: competing-
  consumer queues back send/listen (point-to-point, each message popped
  once) and retained per-subscriber-cursor logs back publish/subscribe
  (fan-out — every subscriber sees every event), the in-memory shape of
  the Postgres-as-log fan-out model. 5 unit tests cover both semantics
  plus unknown-command-ignored and handler-error-via-failure-policy.

  Implements [[tasks/build-transport-bus-facade]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): add NatsBus (send/listen + publish/subscribe)

  NatsBus implements Bus + BusConsumer over one JetStream stream bound to
  `{namespace}.>`:

  - send  → subject `{ns}.cmd.{name}` (command)
  - publish → subject `{ns}.evt.{name}` (event)
  - listen → durable pull consumer `{group}_cmd` filtered to the service's
    command subjects; replicas sharing a group share the durable, so
    JetStream load-balances — point-to-point / competing-consumer.
  - subscribe → durable `{group}_evt`; each distinct group gets its own
    durable on the shared stream, so every group sees every event — fan-out.

  Adds NatsJetStreamSource::with_strip_prefix (default off, backwards
  compatible) so the dispatched message name is the bare name once the
  `{ns}.cmd.`/`{ns}.evt.` subject prefix is removed.

  Two integration tests prove competing-across-a-group (each command
  handled exactly once by concurrent replicas) and fan-out-across-groups
  (every group sees every event) against a live JetStream server. Also
  makes tests/nats_transport unique() process-unique so re-runs against a
  persistent server don't collide with leftover stream/consumer state.

  Implements [[tasks/build-transport-bus-facade]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): add PostgresBus (work queue + log/offset fan-out)

  PostgresBus implements Bus + BusConsumer as a complete single-DB bus:

  - send/listen (point-to-point): a bus_queue work table claimed
    FOR UPDATE SKIP LOCKED under a lease, so replicas sharing a group
    compete — each command handled once (ack=delete, nack=release,
    dead_letter/park=delete).
  - publish/subscribe (fan-out): Postgres as a log — append-only bus_log
    (monotonic seq, retained) + per-consumer bus_offset (consumer →
    last_seq). publish appends; each group reads seq > last_seq for its
    event names in order and advances its own offset, so every group sees
    every event. ack advances the offset (the effectively-once point),
    nack leaves it for redelivery, dead_letter/park skips it.

  ensure_tables() provisions the three tables (mirrors NatsBus::ensure_stream).

  Implementation note: uses the spec's sanctioned claim-lease backend, not
  sqlxmq. Decision #8 listed sqlxmq as recommended with claim-lease as the
  no-dependency alternative; sqlxmq owns an always-on push JobRunner loop
  that doesn't compose with the facade's uniform drain-to-idle run_source
  model (a claim-lease source returns Ok(None) when empty and stops). The
  module header records this; sqlxmq stays a viable future backend.

  Two integration tests prove competing-across-a-group (each command once
  via concurrent replicas) and fan-out-across-groups (each group reads
  every event), against a live Postgres.

  Implements [[tasks/build-transport-bus-facade]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): add RabbitBus (send/listen + publish/subscribe)

  RabbitBus implements Bus + BusConsumer over two AMQP exchange shapes:

  - send/listen (point-to-point): the default exchange routes to a durable
    queue {ns}.cmd.{name}; send declares the queue and publishes with a
    publisher confirm. Replicas sharing a queue compete (AMQP round-robin).
  - publish/subscribe (fan-out): a durable topic exchange {ns}.events;
    publish routes by event name; each subscriber declares its own queue
    {ns}.evt.{group} bound to the exchange for its event names, so every
    group receives every event.

  The message name is resolved from the delivery routing key (stripping
  the {ns}.cmd. prefix for commands). Exposes pub(super) connect_channel /
  message_properties / RabbitReceived::from_delivery_with_name from the
  adapter, and a public RabbitBus::ensure_subscription so a producer can
  bind all subscriber queues before publishing (a topic exchange drops
  events with no matching binding).

  Two integration tests prove competing-across-a-group and
  fan-out-across-groups against a live broker. Makes tests/rabbitmq_transport
  unique() process-unique so durable queue/exchange names don't collide
  across runs.

  Implements [[tasks/build-transport-bus-facade]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): add KafkaBus (send/listen + publish/subscribe)

  KafkaBus implements Bus + BusConsumer; point-to-point vs fan-out is a
  consumer-group choice:

  - send/listen (point-to-point): commands → topics {ns}.cmd.{name};
    listen joins a shared group {ns}.{group}.cmd, so Kafka distributes
    partitions across members — each record handled by one replica.
  - publish/subscribe (fan-out): events → topics {ns}.evt.{name};
    subscribe joins a per-service group {ns}.{group}.evt. Kafka delivers
    every record to every group, so each distinct group sees every event.

  Adds KafkaSource::with_strip_prefix (default off) so the dispatched
  message name is the topic minus its {ns}.cmd./{ns}.evt. prefix.

  Two integration tests against a live broker: point-to-point is proven
  deterministically (a second consumer in the same group reads nothing —
  the group's offset is committed past the end), which avoids the
  rebalance-redelivery flakiness a concurrent two-replica race would have;
  fan-out is proven across two distinct groups each reading from earliest.

  Implements [[tasks/build-transport-bus-facade]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(transport): add KnativeBus (Bus produce + manifest generation)

  Per the locked spec (Decision #6), KnativeBus implements only Bus and
  NOT BusConsumer — Knative is a GitOps/HTTP transport with no in-process
  consume loop:

  - produce: send/publish POST a binary-mode CloudEvent to a broker-ingress
    URL ({ingress_base}/{namespace}/{broker}); publish targets the service's
    own {source}-events broker, send targets a downstream {commands_broker}.
    A message without an id is rejected (CloudEvents mandates `id`).
  - consume = deploy-time artifacts: manifests(&plan, &subscriptions)
    renders role-based Broker + per-name Trigger YAML — own {source}-commands
    broker + command triggers if it handles commands, own {source}-events
    broker if publishes_events, and a Trigger per subscribed event on its
    producer's broker, with /cloudevent/<type> subscriber URIs. A .local(addr)
    builder switches subscribers to a kubefwd address.

  Adds the per-type /cloudevent/{type} route to cloud_events_router
  (Decision #7) and reqwest (optional, default-features off) under the http
  feature for the POST.

  Four tests: produce round-trips through a local cloud_events_router into
  dispatch_message; missing id rejected; manifests render brokers/triggers;
  pure-consumer (publishes_events=false) owns no broker and uses the local
  subscriber URI.

  Implements [[tasks/build-transport-bus-facade]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* docs(transport): document the bus facade + transport-swap example

  Add a "Bus facade" section to docs/async-transports.md: the Bus /
  BusConsumer surface, a transport-swap example (same service + handlers,
  one constructor line changes), the per-transport competing-vs-fan-out
  topology table, and the Knative Bus-only + manifests note. Refresh Status.

  Implements [[tasks/build-transport-bus-facade]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* ci: install libcurl dev headers for the rdkafka build

  The kafka, postgres (--all-features), and coverage (--all-features) jobs
  compile rdkafka-sys, whose bundled librdkafka cmake build enables curl
  (OAUTHBEARER OIDC) when it finds the runner's libcurl runtime, then fails
  with `curl/curl.h: No such file or directory` because the dev headers
  aren't installed. Install libcurl4-openssl-dev in those three jobs.
  nats/rabbitmq use narrow feature sets, build no rdkafka, and were green.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* docs(read-model): clarify ReadModelCommitOutcome is an intentional stub

  Address the CodeRabbit review on PR #47. was_applied() is hardcoded true
  not by accident: the read_model_processed_messages dedupe table and the
  skipped_duplicate outcome were deliberately removed (see
  specs/consumer-inbox-design.md, 2026-05-28) because coupling delivery
  dedupe to the read-model projection contract was the wrong boundary.
  Replay safety is now a projection convention (idempotent handlers +
  per-row ExpectedVersion OCC); a first-class replay barrier returns with
  the consumer inbox as a CommitBatch participant. Document this on the
  type so the always-true was_applied() isn't misread as a lost signal.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* fix(transport): address CodeRabbit review on PR #47

  Three valid review findings:

  - outbox_source: reject zero lease / batch_size in the builder. A zero
    batch makes recv() return Ok(None) forever; a zero lease makes claimed
    rows immediately re-claimable. Both are silent misconfigurations on a
    public builder — assert up front. (+ #[should_panic] tests)

  - rabbitmq: preserve Message.content_type across the AMQP round-trip.
    message_properties now sets AMQP content_type, and from_delivery reads
    it back instead of letting Message::new hardcode application/json. Also
    fixes the same loss for RabbitBus (shares both paths). (+ round-trip
    test asserting a non-JSON content type survives)

  - knative: sanitize generated Trigger names to RFC 1123 (lowercase,
    alphanumeric/'-', no leading/trailing '-', <=63 chars) via a shared
    sanitize_k8s_name helper, applied in both knative_triggers and
    KnativeBus::trigger_yaml. CloudEvent types can contain dots/uppercase
    that are invalid in k8s resource names. (+ unit tests)

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(inbox): add InboxReceipt/InboxOutcome + CommitBatch participant field

  First slices of the consumer inbox (tasks/model-consumer-inbox-across-
  persistence-implementations, per specs/consumer-inbox-design):

  - New src/repository/inbox.rs: InboxReceipt { consumer, message_id,
    processed_at } and InboxOutcome { Processed, Duplicate } (Duplicate is
    success, never an error). Relocates the naming/semantics of the removed
    read_model_processed_messages into a first-class, non-read-model type.
  - Add inbox_receipts: Vec<InboxReceipt> to both CommitBatch (sync) and
    AsyncCommitBatch (async), defaulted empty in new()/empty() and at every
    literal construction site. Trait signatures unchanged; empty everywhere
    so behavior is unchanged (257 lib tests green).

  The receipt is a commit-batch participant so it commits atomically with
  handler effects (the effect fence); storage writes + runner wiring follow
  in subsequent slices.

  Implements [[model-consumer-inbox-across-persistence-implementa]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(inbox): persist consumer_inbox receipts across all backends

  Storage model for the consumer inbox (per specs/consumer-inbox-design):

  - Migration: consumer_inbox operational table (PK (consumer, message_id),
    processed_at default) in both Postgres and SQLite. In-memory backend
    gains an inbox_store set.
  - commit_batch / commit_batch_async write batch.inbox_receipts inside the
    existing commit transaction (the effect fence). The (consumer, message_id)
    primary key is the dedupe gate: a duplicate receipt rolls the whole batch
    back via the new RepositoryError::DuplicateInboxReceipt, so a redelivery's
    effects are never double-applied (in-memory checks the staged set; SQL maps
    the unique violation).
  - New AsyncInboxStore::inbox_contains_async pre-check, implemented for
    in-memory / SQLite / Postgres, so a consumer can skip an already-processed
    message before opening a transaction.

  Tests prove record + pre-check + dedupe + atomic rollback (a batch with a
  duplicate and a fresh receipt rolls back whole) on all three backends:
  in-memory unit test, SQLite, and live Postgres.

  Implements [[model-consumer-inbox-across-persistence-implementa]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* : 

* test(matrix): generic transport×persistence harness + in-memory cell

  First slice of the distributed read-model matrix (async bus facade only,
  no sync path). Ungate the generic async flow helpers so they are the
  primary path, and add run_checkout_over_bus<B: Bus + BusConsumer, R>:
  drive the seat-checkout domain flow + read-model projection + query on
  persistence R, route the events over transport B, and assert the
  projected checkout screen. Validated cell: HashMapRepository × InMemoryBus.

  Refs [[tasks/transport-persistence-matrix]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(matrix): full transport×persistence matrix over the bus facade

  The distributed read-model seat-checkout scenario now runs across every
  async transport × persistence backend, all green against live brokers:

    transports : InMemoryBus, NatsBus, RabbitBus, KafkaBus, PostgresBus, Knative
    persistence: HashMapRepository, SqliteRepository, PostgresRepository

  12 matrix cells (broker/DB cells skip when their env var is unset):
  in-memory & sqlite over each of InMemory/NATS/Rabbit/Kafka/Knative,
  in-memory & postgres-persistence over a Postgres bus / in-memory bus.

  Knative is a first-class transport cell: KnativeBus POSTs CloudEvents to a
  local cloud_events_router serving the projection sink (the HTTP/gRPC command
  ingress is this same Knative surface) — no broker needed. RabbitMQ binds the
  subscription before publishing (topic exchange drops unrouted events); NATS
  ensures the stream; Postgres bus ensures its tables.

  Shared helpers: build_collector (the transport sink), run_checkout_over_bus
  (pull buses), run_checkout_over_knative (HTTP), project_and_assert_checkout.
  All on the async bus facade — no sync path.

  Refs [[tasks/transport-persistence-matrix]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(matrix): complete transport×persistence grid + refactor gold-standard test onto the async bus

  Refactor (not delete) the gold-standard seat_checkout_saga test onto the
  async InMemoryBus: same services, choreography, projection, query, and
  assertions — the legacy InMemoryQueue/OutboxWorkerThread/Subscribable
  wiring is replaced by publish_pending_outbox (claim→publish→complete bridge)
  + bus.subscribe hops. The projection_service/query_service modules are kept.

  Complete the matrix to the full 6×3 grid (18 cells), all green against live
  brokers: { HashMap, SQLite, Postgres } persistence × { InMemoryBus, NatsBus,
  RabbitBus, KafkaBus, PostgresBus, Knative } transport. Postgres-persistence
  fixtures + Postgres-bus pairings added; broker/DB cells skip without env.

  Full distributed_read_model suite: 23 passed (refactored sync test + 18
  matrix cells + 2 async flow tests + HTTP/gRPC command tests).

  Refs [[tasks/transport-persistence-matrix]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(cutover): migrate transport_subscribe onto the async InMemoryBus

  First step of Phase 1 (legacy sync bus removal): the pub/sub transport test
  now publishes events to InMemoryBus and drains them via bus.subscribe,
  instead of Bus::from_queue(InMemoryQueue) + microsvc::subscribe. Proves the
  migration pattern; the legacy bus src stays until all consumers are migrated.

  Refs [[tasks/async-only-consolidation]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(read-model): async ReadModelWorkspace (load_async/commit_async parity)

  The load -> mutate -> sync -> commit workspace ergonomic existed only over
  the sync store traits; the async path used the bare write-plan builder. This
  restores parity: the mutation/sync/diff surface is store-independent, so the
  same `ReadModelWorkspace` now gains `load_async`/`commit_async` over the
  `Async{ReadModelWritePlanStore,RelationalReadModelQueryStore}` traits, plus
  `AsyncReadModelLoadBuilder` and `AsyncReadModelWorkspaceExt::workspace_async()`.

  No struct extraction or duplicated diff logic: `load`/`commit` move to small
  sync- and async-bound impl blocks; everything else stays shared and unbounded.

  Proven with async mirrors of the include-hydration and sync-roundtrip tests
  on `InMemoryReadModelStore` (impls both async store traits). Sync workspace
  API and its tests unchanged.

  Part of [[tasks/async-only-consolidation]] (Phase 2).

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(queued-repo): async QueuedRepository — per-aggregate serialization over the async surface

  Async paths previously bypassed QueuedRepository entirely (AsyncCommitBuilder
  commits straight through commit_batch_async), so two concurrent async commits
  to the same aggregate could interleave. This restores the queueing ability for
  async: `repo.queued_async().async_aggregate::<T>()` serializes per-aggregate
  get/commit exactly like the sync `.queued().aggregate::<T>()`.

  Lock primitive (runtime-agnostic — no tokio dep, matching the crate's RPITIT
  async surface):
  - AsyncLock / AsyncLockManager traits + InMemoryAsyncLock / InMemoryAsyncLockManager,
    a hand-rolled waker-based async mutex (try_lock/unlock stay sync; only acquire awaits).

  QueuedRepository<R, AsyncLockManager> (struct/Clone bound moved to the impls so an
  async lock manager is accepted):
  - AsyncGetStream / AsyncTransactionalCommit with the sync locking contract:
    reads acquire+hold the per-stream lock, commit releases on success and holds on
    error, multi-locks acquired in sorted/deduped order. Keyed by StreamIdentity::storage_key
    consistently across get/commit/unlock.
  - Non-locking forwards (drop-in completeness): AsyncSnapshotStore,
    AsyncReadModelWritePlanStore, AsyncRelationalReadModelQueryStore, AsyncInboxStore.
  - AsyncGetWithOpts / AsyncGetAllWithOpts (no_lock opt-out) + AsyncUnlockableRepository.
  - Queueable::queued_async() / queued_async_with(); AsyncAggregateRepository gains
    get_with/peek/get_all_with/peek_all/abort/unlock mirroring the sync layer.

  Adversarial review (3 lenses) found two latent defects in unlock(), both fixed:
  waking wakers while holding the std Mutex guard could (1) poison/brick the lock if
  a waker panics and (2) deadlock if a waker synchronously re-polls. unlock() now
  drains under the guard and wakes outside it; regression tests cover both.

  Tests: async lock unit tests (incl. re-entrant + panicking waker regressions) and
  tests/queued_repo_async (mutual exclusion, per-aggregate granularity, no_lock peek,
  abort release). Sync QueuedRepository API and its tests unchanged.

  Part of [[tasks/async-only-consolidation]] (Phase 2).

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(cutover): migrate transport_listen onto the async InMemoryBus

  Replaces the legacy `Bus::from_queue`/`microsvc::listen` queue tests with the
  async `InMemoryBus` + `BusConsumer::listen` (competing-consumer queues keyed by
  command name). The legacy `stats.handled`/`stats.failed` handle has no async
  analogue, so:
  - success is asserted via domain outcomes (committed aggregate state), not counts;
  - failure tolerance is asserted by showing the consumer drains past a failing
    message and still processes the rest;
  - metadata->Session is verified through `whoami` over the bus (works via
    run_source -> dispatch_message -> message_to_session), with a negative control
    under FailurePolicy::Stop;
  - arbitrary queue names ("counters"/"creates") become command-name routing, so
    two services on one bus consume disjoint command queues without competing.

  Confirms Phase 1 needs no new runtime capability — metadata->Session already
  works and the stats gap is a test-rewrite. microsvc crate: 15 passed.

  Part of [[tasks/async-only-consolidation]] (Phase 1).

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(cutover): migrate microsvc_saga distributed test onto the async InMemoryBus

  Replaces the threaded legacy-bus choreography (InMemoryQueue +
  OutboxWorkerThread::spawn_routed + microsvc::listen per service + sleep-poll)
  with a deterministic, thread-free drive over the async InMemoryBus:

  - `publish_pending_outbox` claims each service's outbox and forwards messages by
    destination — worker-addressed messages are point-to-point commands
    (send_message → consumed via `listen`), saga-addressed messages are events
    (publish_message → consumed via `subscribe`).
  - Each round uses a FRESH bus (the in-memory topic log is retained across reads,
    so a shared bus would re-deliver every prior event to the saga), forwards the
    pending outbox backlog, then drains the consumers. The loop ends when no
    service has pending work — i.e. the saga reached Completed.

  The `stats.handled` assertions (no async analogue) are dropped in favor of the
  existing domain assertions (saga/order Completed, inventory 95 available / 5
  reserved, payment successful). Test 1 (saga_orchestrated) was already bus-free
  and is unchanged. Both tests pass.

  Part of [[tasks/async-only-consolidation]] (Phase 1).

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(cutover): remove superseded raw-legacy-bus saga tests (distributed.rs)

  tests/sagas/distributed.rs drove the order-fulfillment saga over the raw
  `bus::Bus`/`Subscribable` API with hand-spawned threads and manual aggregate
  handling (bus.subscribe(&[names]) -> events.recv() loops). The async InMemoryBus
  has no raw-receiver equivalent — listen/subscribe are Service-driven — so the
  file cannot be faithfully migrated; a rewrite would duplicate the async
  microsvc_saga::saga_distributed test (same saga) plus the matrix metadata
  coverage. Removed as superseded (owner-confirmed): no coverage is lost.

  Also drops the now-unused event payloads in tests/sagas/order/events.rs (only
  distributed.rs constructed them). sagas crate: 7 passed, no warnings.

  Part of [[tasks/async-only-consolidation]] (Phase 1).

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(cutover): decouple projection handlers from bus::Event; migrate board onto InMemoryBus

  The projection handlers of BOTH the gold-standard matrix and the board still
  decoded via bus::Event (`Event::try_from(ctx.message())` + event.decode/
  json_decode + event.id) — so the legacy bus could not be removed without
  touching the gold-standard test. Decoupled both (refactor, not delete):

  - decode straight from ctx.message().payload(): serde_json::from_slice for the
    matrix (JSON), BitcodePayloadCodec::decode for the board (bitcode — identical
    bytes to the old event.decode()); match on ctx.message().name(); event id from
    ctx.message().id(). Dropped the bus::Event `event()` helper from both
    projection handlers/mod.rs.
  - board main.rs: replaced InMemoryQueue + OutboxWorkerThread + the threaded
    start_board_projection_service + wait_for_* polling with publish_pending_outbox
    (fan-out events) + a single bus.subscribe; the projection's monotonic
    source_version guard makes the per-event-type drain order-independent. Added
    projections_service::load_board (direct read) replacing the poll loop.

  matrix: 2 passed (in-memory cell + refactored saga, both exercise the decode);
  board: 3 passed; sagas: 7 passed. clippy/fmt clean.

  Part of [[tasks/async-only-consolidation]] (Phase 1 — last test migration before
  the src removal).

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor: remove the legacy sync bus

  BREAKING CHANGE: The legacy sync bus is fully superseded by the async bus facade (InMemoryBus +
  the BusConsumer listen/subscribe + OutboxSource) and had no remaining consumers
  after the test migrations. Removed:

  - `src/bus/` entirely (Bus/Subscribable/InMemoryQueue/Listener/Sender/EventBus/
    Event/Publisher, ~1.4k lines).
  - `OutboxWorkerThread` + WorkerStats + OutboxWorkerJoinError (the threaded
    outbox->bus bridge) and `src/outbox_worker/thread.rs`.
  - The bus-gated `microsvc::service` surface: `dispatch_event`,
    `dispatch_listened_event`, `subscribe`/`listen`, `TransportHandle` +
    `TransportStats`/`TransportJoinError`, and the `From<&Event> for Message` /
    `TryFrom<&Message> for Event` / `from_bus_event` bridges (+ their unit tests).
  - The `bus` Cargo feature (out of `default`); `http`/`grpc` no longer depend on
    it — they use the unconditional `microsvc::Message`, confirmed by building
    `--features http,grpc`.
  - The bus-gated crate-root re-exports (`InMemoryQueue`, `bus::Message`,
    the threaded-worker types).

  All consumers were migrated first (transport_subscribe/listen, microsvc_saga,
  the board) or removed as superseded (sagas/distributed.rs), and both projection
  handlers were decoupled from `bus::Event`. Default test sweep: 238 lib + all
  integration crates green; `--features http,grpc` builds; clippy/fmt clean.

  Closes Phase 1 of [[tasks/async-only-consolidation]] — one async bus facade,
  no sync bus path.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* feat(microsvc): async handler model (core) — handlers become async fn

  BREAKING CHANGE: Converts the microsvc handler model from sync to async, the foundation for
  dropping the sync repository API so all backends are async-only (the sync/async
  mix was the source of subtle bugs).

  Core (lib green; integration test crates migrated in follow-up commits):
  - HandlerFn is now `dyn for<'a> Fn(&'a Context<'a, D>) -> Pin<Box<dyn Future<
    Output=Result<Value, HandlerError>> + Send + 'a>>`, with an `AsyncHandler<'a,D>`
    HRTB helper trait so `async fn handle(ctx: &Context<D>)` registers directly.
    Guards stay synchronous.
  - Service::dispatch / dispatch_message / dispatch_request / invoke are async.
  - dependencies.rs: HasRepo/HasReadModelStore now resolve via the ASYNC repo +
    read-model traits (+ HasRepo for AsyncAggregateRepository / AsyncSnapshotAggregateRepository).
  - run_source + the http/grpc/knative transports await dispatch.
  - src unit tests converted (async-closure handlers + awaited dispatch).

  Handler authors write `async fn handle`; closures need an explicit ctx type
  annotation and must extract owned values before the `async move` (the future
  cannot borrow ctx across the await — an HRTB-closure limitation).

  cargo build (default + --features http,grpc) green; 238 lib tests pass.
  NOTE: tests/ integration crates still use the sync handler API and are migrated
  in the following commits (all-or-nothing handler switch).

  Part of [[tasks/async-only-consolidation]] (Phase 3).

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test(microsvc): migrate all integration test crates to async handlers

  BREAKING CHANGE: Completes the integration half of the async handler switch: all 21 test crates
  (microsvc, sagas, the gold-standard distributed_read_model matrix, the board,
  the transport conformance crates, and the ~15 direct-repo crates) now use the
  async handler + async repo API exclusively:

  - handlers are `async fn handle(ctx: &Context<'_, D>)` with awaited
    ctx.repo().get/commit/peek and ctx.repo().outbox(msg).commit(&mut a).await;
    read-model handlers use workspace_async()/load_async()/commit_async().await.
  - services build with .queued_async().async_aggregate(); inline handler closures
    use the `|ctx: &Context<D>| { extract ctx reads; async move { ... } }` form.
  - test bodies await dispatch and the now-async repo reads.

  Guards stay synchronous. Assertions and domain logic unchanged. The sync repo
  trait surface is still present (deleted next); 502 default tests pass, the gold
  -standard matrix's gated cells compile, http/grpc/sqlite cells pass.

  Part of [[tasks/async-only-consolidation]] (Phase 3).

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor: remove the sync repository API — the crate is now async-only

  BREAKING CHANGE: Deletes the entire synchronous repository/read-model/snapshot trait surface,
  now unused after the async handler switch. This eliminates the sync/async mix
  that was the source of subtle combination bugs: there is exactly one (async)
  path for every backend.

  Removed (traits + all backend impls + re-exports):
  - repository: Get/Commit/Repository (repository.rs), GetOne/GetMany/Gettable
    (gettable.rs), the TransactionalCommit trait (batch.rs; CommitBatch kept).
  - snapshot: sync SnapshotStore + sync SnapshotAggregateRepository/SnapshotOutboxCommit.
  - read_model: sync ReadModelWritePlanStore/RelationalReadModelQueryStore, the sync
    ReadModelWorkspace load/commit impl, ReadModelLoadBuilder, ReadModelWorkspaceExt,
    and ReadModelWritePlanBuilder::commit (async equivalents kept).
  - aggregate: GetAggregate/GetAllAggregates/CommitAggregate + the sync
    AggregateRepository/AggregateBuilder (AsyncAggregateRepository/Builder kept).
  - commit_builder: SyncCommitBuilder/SyncStagedCommitBuilder/exts.
  - outbox: SyncOutboxCommit/SyncOutboxCommitExt (outbox_sync/commit_sync).
  - hashmap/postgres/sqlite/in-memory backends: their sync impls.
  - queued_repo: the sync QueuedRepository impls + sync Queueable::queued; the sync
    lock module (Lock/LockManager/InMemoryLock/InMemoryLockManager) is now fully
    unused and deleted (Async lock variants kept; LockError kept).
  - src/ unit tests that exercised the removed sync surface, converted to async.

  Also converted 5 remaining fully-sync integration crates the earlier sweep
  missed (bomberman [19 files], read_model_relationship_includes,
  read_model_commit_bridge, sourced_upcasting, transport_conformance's store_outbox).

  cargo test: 490 passed / 0 failed; --features http,grpc / postgres / sqlite all
  build; clippy clean; no sync trait remains in src/.

  Completes Phase 3 of [[tasks/async-only-consolidation]] — HashMap/SQLite/Postgres
  all async-only and consistent.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test: gate matrix table_schema_registry helper to postgres/sqlite

  It is only used by the postgres/sqlite-gated matrix cells, so it (and its
  `TableSchemaRegistry` import) tripped a dead-code warning on the default build.
  Gate both with cfg(any(feature = "postgres", feature = "sqlite")) to match the
  call sites. Default clippy is now fully clean.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* test: address CodeRabbit review (block_on, handler panic, weak assertion)

  - src/ unit tests (outbox/commit, snapshot/in_memory, snapshot/repository,
    read_model/in_memory, commit_builder, outbox_worker/store, hashmap_repo):
    replace the custom busy-poll `block_on` (no-op waker, ignores Poll::Pending —
    would spin on any yielding future) with `#[tokio::test]`. Transport modules
    keep their intentionally runtime-free block_on.
  - board projection handler: `event_version` returns Result<_, HandlerError>
    instead of panicking on a malformed message id; the handler propagates with
    `?`. Its unit test now asserts the error path.
  - tests/todos: the bulk-commit roundtrip now asserts the commit succeeds and that
    exactly 3 todos are present (was: ignored result + an `if !empty` that masked
    failures). The concurrency-race commits (deliberately may lose the lock) keep
    their `let _ =`.

  490 tests pass; clippy --all-targets clean.

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>


See full diff: [v0.6.0...v1.0.0](https://github.com/patrickleet/sourced_rust/compare/v0.6.0...v1.0.0)
