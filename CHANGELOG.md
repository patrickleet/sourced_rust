### What's changed in v2.3.2

* fix: backport sqlite bus hardening to PostgresBus (by @patrickleet)

  Port the hardening SqliteBus received but PostgresBus never got:

  - Claim-token fencing: each queue claim mints a claim_token
    (gen_random_uuid()) and every settlement (delete on ack/dead_letter/park,
    release on nack) is scoped to seq AND claim_token, so a worker whose
    lease expired cannot settle a row that was reclaimed under a new token.
  - Strict row decode: corrupt content_type/metadata/kind are permanent
    corruption surfaced through decode_error and the failure policy, instead
    of being silently defaulted (which could ack a garbled message).
  - Error classification: database errors are classified transient vs
    permanent via is_sqlx_transient instead of always retryable, so
    deterministic failures reach the failure policy instead of redelivering
    forever.
  - Schema CHECK constraints (name/kind/content_type/attempts/claim_token)
    and the claim index including locked_until, mirroring the sqlite schema.

  Tests mirror sqlite_transport: corrupt kind/metadata/content_type rows are
  dead-lettered not silently skipped, the schema rejects unsupported kinds,
  and a stale worker cannot settle an expired reclaimed queue claim.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* refactor: make Bus::send/publish default trait methods (by @patrickleet)

  All 7 transports implemented identical send/publish bodies that wrap the
  payload in a Command/Event message and delegate to send_message/
  publish_message. Provide those bodies as trait defaults and delete the 7
  duplicate impls; transports now implement only send_message and
  publish_message.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* refactor: extract shared SqlBus core for the Postgres/SQLite buses (by @patrickleet)

  After the hardening backport, postgres_bus and sqlite_bus were near-clones.
  Mirror the proven lock/sqlx_common pattern: a new bus/sql_bus_common with a
  generic SqlBus<B: SqlBusDialect> owning the builders, Bus/BusConsumer
  impls, queue/log sources, row decoding, and claim-token-fenced settlement.
  Each backend now contributes only its dialect (SCHEMA, statements, claim
  and log-read queries — postgres array-bind vs sqlite IN-list).

  PostgresBus/SqliteBus and the *Received types become type aliases of the
  generic types — same public API, no wrapper types.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* perf: batch SQL bus source reads (by @patrickleet)

  Both SQL sources fetched one row per query, re-running the claim/offset
  subquery for every message. Claim and log_read now fetch up to 16 rows per
  query into a VecDeque buffer (the OutboxSource pattern).

  Queue rows stay independently settleable: each claimed row carries its own
  claim token (gen_random_uuid()/randomblob are evaluated per row), so nacks
  and lease expiry behave exactly as before.

  Log read-ahead preserves the offset contract: the settle handles report
  forward settlement (ack/dead-letter/park) through a shared seq watermark,
  and when the previously delivered entry was nacked (offset unmoved) the
  source discards its buffer and re-reads from the durable offset, so a
  buffered later entry can never advance the offset past a nacked one. The
  runner settles each message before the next recv, so the check is
  race-free.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* perf: cache RabbitMQ topology declarations and make queue polling sticky (by @patrickleet)

  send_message declared its command queue and publish_message its events
  exchange on every call — a broker round-trip per message. Track declared
  names in a per-process set: each queue/exchange is declared once (the
  events exchange eagerly at connect), with later sends/publishes skipping
  the declare entirely. Declarations are idempotent, so the benign
  concurrent-declare race is harmless, and a post-connect namespace() change
  just declares the new names on first use.

  RabbitBusSource::recv polled every queue per message via basic_get. Keep
  basic_get (basic_consume pushes with no broker-side drained signal, which
  would break the Ok(None) drain-to-idle contract) but make polling sticky:
  start each recv at the queue that last yielded, so draining a busy queue
  costs one basic_get per message; a full empty cycle is still required
  before returning None.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* refactor: share wire-to-Message decoding across broker transports (by @patrickleet)

  NATS, Kafka, and RabbitMQ each hand-rolled the same decode tail: route the
  id/kind headers into Message::id/Message::kind and everything else into
  metadata. Extract message_from_wire(name, payload, id_key, kind_key,
  headers) in bus/message.rs; each adapter now contributes only its
  header-pair iterator (RabbitMQ passes id_key: None — its id rides in the
  AMQP message_id property — and keeps its content-type override). Encode
  paths stay per-transport: the header types differ.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* refactor: fold NATS/Kafka listen/subscribe mirrors into one consume path (by @patrickleet)

  listen and subscribe in NatsBus and KafkaBus were line-for-line mirrors
  differing only in the plan half (commands vs events) and the cmd/evt
  suffix used for subjects/topics, durables, and group ids. Each bus now has
  one private consume(router, options, kind) that both delegate to,
  preserving the empty-plan-check-before-group-resolution ordering asserted
  by the kafka_bus tests. Rabbit's pair stays as is (structurally
  different). KafkaBus's run() helper is absorbed into consume.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

* refactor: generalize RabbitSource to multi-queue, delete RabbitBusSource (by @patrickleet)

  RabbitBusSource duplicated RabbitSource's basic_get + settle shape, adding
  only multi-queue polling and routing-key prefix stripping. Fold those into
  RabbitSource (with the sticky polling from the previous perf change) and
  delete RabbitBusSource. The single-queue constructor keeps its public
  signature; naming now derives from the delivery's routing key, which under
  the default exchange equals the queue name, so standalone behavior is
  unchanged.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n


See full diff: [v2.3.1...v2.3.2](https://github.com/hops-ops/distributed/compare/v2.3.1...v2.3.2)
