# Async Microservice Transports

Distributed (published from the `distributed` crate) keeps the synchronous
in-memory bus intact and adds an async transport layer under
`bus`. The design line is:

- **`microsvc`** owns handler registration, guards, typed input decoding,
  dispatch, and handler metadata;
- **transport adapters** own how messages are received, acknowledged, retried,
  published, and mapped to external topics/subjects/queues/routes.

The shared vocabulary lives in `bus` and depends on no concrete
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
use distributed::bus::{run_source, RunOptions};

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

## Bus facade: `send`/`listen` + `publish`/`subscribe`

The adapters above are the low-level boundary. The **bus facade** is the
ergonomic surface on top: a produce trait [`Bus`] (`send` a command, `publish` an
event) and a consume trait [`BusConsumer`] (`listen` for commands, `subscribe` to
events), implemented by a per-transport `*Bus` type. `listen`/`subscribe` derive
the message names from the service's registered handlers
(`command_names()`/`event_names()`), build the transport's source with the right
topology, and run it through the shared `run_source` — handler code and
`dispatch_message` never change.

The app surface is identical across transports; only the constructor changes:

```rust
use std::sync::Arc;
use distributed::bus::{Bus, BusConsumer, InMemoryBus, RunOptions};

// Built once — handlers are transport-agnostic.
let service = Arc::new(build_service());

// Dev/test: in-memory.
let bus = InMemoryBus::new();
bus.send("place.bet", payload).await?;          // point-to-point command (1:1)
bus.publish("seat.reserved", payload).await?;   // fan-out event (1:N)
bus.listen(service.clone(), RunOptions::idempotent()).await?;     // competing
bus.subscribe(service.clone(), RunOptions::idempotent()).await?;  // fan-out

// Production: swap the one constructor line — send/listen/publish/subscribe
// and the handlers are unchanged.
//   let bus = NatsBus::connect("nats://localhost:4222", "orders", "app").await?;
//   let bus = PostgresBus::new(pool, "orders");
//   let bus = RabbitBus::connect("amqp://localhost:5672/%2f", "orders", "app").await?;
//   let bus = KafkaBus::connect("localhost:9092", "orders", "app").await?;
```

Point-to-point vs fan-out is consistently a **consumer-group/identity** choice in
each transport's native topology — same `group` competes, different `group`s
fan out:

| `*Bus` | Feature | `send` / `listen` (competing) | `publish` / `subscribe` (fan-out) |
| --- | --- | --- | --- |
| `InMemoryBus` | (always) | named queue, popped once | retained log + per-subscriber cursor |
| `NatsBus` | `nats` | shared durable `{group}_cmd` on the stream | durable `{group}_evt` per group |
| `PostgresBus` | `postgres` | `bus_queue`, `FOR UPDATE SKIP LOCKED` | `bus_log` + `bus_offset` per `group` (Kafka-style) |
| `RabbitBus` | `rabbitmq` | default exchange → durable queue `{ns}.cmd.{name}` | topic exchange → queue `{ns}.evt.{group}` per group |
| `KafkaBus` | `kafka` | shared consumer group `{ns}.{group}.cmd` | consumer group per service `{ns}.{group}.evt` |
| `KnativeBus` | `http` | POST CloudEvent → `{target}-commands` broker-ingress | POST → own `{source}-events` broker; consume via generated Triggers |

`KnativeBus` implements only [`Bus`] (produce → broker-ingress POST). It has no
in-process consume loop: `KnativeBus::manifests(&plan, &subscriptions)` renders
the role-based `Broker` + per-name `Trigger` YAML (subscriber URIs
`/cloudevent/<type>`, with a `.local(addr)` kubefwd variant), and the service
mounts `cloud_events_router` so those Triggers reach `dispatch_message`.

`PostgresBus` uses the claim-lease work queue (not `sqlxmq`) for the same reason
the low-level adapter does — sqlxmq's always-on push runner doesn't compose with
the uniform drain-to-idle `run_source` model the facade shares; its `bus_log` +
`bus_offset` fan-out gives single-DB transactional effectively-once (the offset
advances with the effects). See `specs/transport-bus-facade`.

## Testing

The reusable conformance harness (`tests/transport_conformance/`) proves the
contract with adapter-neutral fakes; `tests/transport_in_memory/` runs it as the
in-memory reference. Real-broker integration tests are feature-gated and skip
when their env var is unset:

```sh
docker compose up -d   # postgres, rabbitmq, kafka, nats (see compose.yaml)

DATABASE_URL=postgres://sourced:sourced@localhost:5432/distributed \
  cargo test --test postgres_transport --features postgres
NATS_URL=nats://localhost:4222   cargo test --test nats_transport --features nats
AMQP_URL=amqp://guest:guest@localhost:5672/%2f \
  cargo test --test rabbitmq_transport --features rabbitmq
KAFKA_BROKERS=localhost:9092     cargo test --test kafka_transport --features kafka
```

Each transport's integration binary also covers its `*Bus`: a competing-consumer
case (one delivery across a shared group) and a fan-out case (every group sees
every event), verified against the real broker. Each broker has a matching GitHub
Actions job (reusable `.github/workflows/integration-*.yaml`) that runs on PRs and
on push to `main`.

## Status

Implemented and verified: the core contracts, the source runner, the publisher /
outbox dispatcher, the conformance harness, the Postgres / NATS / RabbitMQ /
Kafka adapters, the Knative ingress, and the **bus facade** (`Bus` +
`BusConsumer` with `InMemoryBus` / `NatsBus` / `PostgresBus` / `RabbitBus` /
`KafkaBus` / `KnativeBus`, each with real-broker competing-vs-fan-out tests).
Still open: migrating the in-repo examples to showcase these APIs and removing the
legacy synchronous bus paths (a breaking change). See
`tasks/transport-docs-examples-cutover`.
