# Async Microservice Transports

Distributed (published from the `sourced_rust` crate) keeps the synchronous
in-memory bus intact and adds an async transport layer under
`microsvc::transport`. The design line is:

- **`microsvc`** owns handler registration, guards, typed input decoding,
  dispatch, and handler metadata;
- **transport adapters** own how messages are received, acknowledged, retried,
  published, and mapped to external topics/subjects/queues/routes.

The shared vocabulary lives in `microsvc::transport` and depends on no concrete
broker. The same application code runs over any transport — selecting one is an
adapter/wiring change, not a handler change.

## Core vocabulary

| Type | Purpose |
| --- | --- |
| `TransportError` / `TransportErrorKind` | Retryable vs permanent classification. Drives redelivery vs the failure policy. |
| `FailurePolicy` / `FailureAction` | What happens to a permanent failure: `Retry`, `DeadLetter`, `Park`, `LogAndAck`, `Stop`. |
| `RunOptions` / `ConsumerDeliveryMode` / `InboxHook` | Idempotent dispatch by default; placeholder hook for the future consumer inbox. |
| `TransportCapabilities` | Per-transport receive durability, publish confirmation, retry ownership, ack kind, Knative integration. |
| `validate_stable_message_id` | Rules an inbox-enabled run uses to reject messages lacking a usable dedup key. |

### Two confirmation thresholds

Producing and consuming have *separate* completion thresholds:

- **Producer publish threshold** — when an outbox row may be marked published:
  Postgres transaction commit, RabbitMQ publisher confirm, Kafka producer ack
  (`acks`), NATS JetStream publish ack, Knative 2xx, in-memory acceptance. Only
  then is the row complete; an unknown outcome stays retryable.
- **Consumer ack threshold** — when the adapter may acknowledge receipt: only
  after the handler (and any inbox receipt) committed. The default never
  silently acks a handler error — retryable failures redeliver, permanent
  failures go through the `FailurePolicy`.

## Receiving: `AsyncMessageSource` + `run_source`

Direct transports implement `AsyncMessageSource` (pull a message) and
`ReceivedMessage` (settle it). `run_source` drives the loop, dispatching through
`Service::dispatch_message` and settling only after the handler completes:

```rust,ignore
use sourced_rust::microsvc::transport::{run_source, RunOptions};

run_source(service, source, RunOptions::idempotent()).await?;
```

The runner acks on success, nacks retryable failures for redelivery, routes
permanent failures through the failure policy, **acks-and-ignores** messages with
no registered handler (so fan-out transports can over-deliver), stops gracefully
when the source drains, and never swallows receive/settle errors. Inbox mode
(`RunOptions::inbox(hook)`) enforces a stable message id before dispatch.

## Publishing: `AsyncMessagePublisher` + outbox

`AsyncMessagePublisher` is the single publish boundary; each adapter documents
its publish threshold. `OutboxDispatcher` bridges durable outbox rows to a
publisher, sharing one claim → publish → complete path between background polling
(`dispatch_batch`) and after-commit immediate dispatch (`dispatch_ids`):

```rust,ignore
let dispatcher = OutboxDispatcher::new(store, publisher, "worker-1", lease, max_attempts);
let outcome = dispatcher.dispatch_ids(&committed_ids).await?; // claim-before-publish
```

A row completes only after `publish()` resolves `Ok`; an unknown/failed publish
leaves it retryable (release until the attempt ceiling, then fail). Outbox rows
map to a canonical `Message` via `From<&OutboxMessage>`; framework-derived
metadata (codec, destination, source aggregate) is namespaced under the reserved
`x-sourced-` prefix so it cannot be shadowed by user metadata.

## Adapters

| Transport | Feature | Source / Publisher | Notes |
| --- | --- | --- | --- |
| In-memory | (always) | conformance fakes | Reference adapter; reused by `transport_conformance`. |
| Postgres | (always) | `OutboxSource<PostgresOutboxStore>` | Outbox-backed durable receive: `FOR UPDATE SKIP LOCKED` + lease, ack→complete, nack→release, dead-letter/park→fail. The starter durable transport. |
| NATS JetStream | `nats` | `NatsJetStreamSource` / `NatsPublisher` | ack/nak/term; stable id rides as `Nats-Msg-Id` (also the dedup key). |
| RabbitMQ | `rabbitmq` | `RabbitSource` / `RabbitPublisher` | Publisher confirms; `basic_get`; ack/nack-requeue/reject. |
| Kafka | `kafka` | `KafkaSource` / `KafkaPublisher` | `acks=all`; consumer-group offset commit on ack, seek-back on nack. |
| Knative / HTTP | `http` | `cloud_events_router` (ingress) | Endpoint-driven, not a polling source; 200 success / 503 retryable / 422 permanent; `knative_triggers()` renders Trigger YAML from `subscription_plan()`. |

Postgres is the low-ops starter: one Postgres cluster can back repositories,
read models, outbox, and durable transport. (`sqlxmq` was evaluated but its
push-based `JobRegistry` does not fit the pull-based `AsyncMessageSource` /
`run_source` boundary, so the proven durable-queue patterns were borrowed rather
than the crate — see `tasks/postgres-transport-adapter-first-pass`.)

Retry/backoff/dead-lettering ownership differs: with Knative it is
**platform-managed** (Delivery/Trigger config); with direct transports the
adapter and this crate own it via the `FailurePolicy` and the outbox lease.

## Testing

The reusable conformance harness (`tests/transport_conformance/`) proves the
contract with adapter-neutral fakes; `tests/transport_in_memory/` runs it as the
in-memory reference. Real-broker integration tests are feature-gated and skip
when their env var is unset:

```sh
docker compose up -d   # postgres, rabbitmq, kafka, nats (see compose.yaml)

DATABASE_URL=postgres://sourced:sourced@localhost:5432/sourced_rust \
  cargo test --test postgres_transport --features postgres
NATS_URL=nats://localhost:4222   cargo test --test nats_transport --features nats
AMQP_URL=amqp://guest:guest@localhost:5672/%2f \
  cargo test --test rabbitmq_transport --features rabbitmq
KAFKA_BROKERS=localhost:9092     cargo test --test kafka_transport --features kafka
```

Each broker has a matching GitHub Actions job (reusable
`.github/workflows/integration-*.yaml`) that runs on PRs and on push to `main`.

## Status

Implemented and verified: the core contracts, the source runner, the publisher /
outbox dispatcher, the conformance harness, the Postgres / NATS / RabbitMQ /
Kafka adapters, and the Knative ingress. Still open: migrating the in-repo
examples to showcase these APIs and removing the legacy synchronous bus paths
(a breaking change), and a long-running poll/notify consumer daemon for the
Postgres source. See `tasks/transport-docs-examples-cutover`.
