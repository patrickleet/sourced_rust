# celld 0.4 aggregate cells, Queue, and bus relay

This fixture exercises the write side of the Distributed flow:

```text
command -> AggregateCell -> domain event -> same-cell outbox
        -> celld OUTBOX Queue binding -> Worker relay -> Distributed Bus -> NATS
```

The aggregate Worker owns one SQLite Durable Object per Todo or Chat shard.
`AggregateCell::durable_state` assembles the event log, command ledger,
snapshot/sealed state, and outbox into one versioned envelope. The Worker saves
that envelope with one SQLite upsert, so the aggregate mutation and its outbox
are one cell commit. The host attaches `CelldOutbox::from_env(&env, "OUTBOX")`
to the `AggregateCell`, then `persist_and_drain_outbox` owns the complete
persist, watchdog, Queue dispatch, settlement persistence, and alarm-rearming
lifecycle. celld's output gate holds Queue egress until the cell write is
durable. If settlement persistence is interrupted, the same stable event id may
be delivered again; consumers must deduplicate by that id.

```rust,ignore
let cell = AggregateCell::<Todo>::new_with_snapshots(shard, 1)?
    .mount(create())
    .mount(complete())
    .with_celld_outbox(CelldOutbox::from_env(&env, "OUTBOX")?);

cell.persist_and_drain_outbox(&env, &storage, |state| {
    persist_cell_state(&sql, state)
})
.await?;
```

The same `persist_and_drain_outbox` call runs after a command and from the
Durable Object alarm. It persists the full state before Queue egress, arms the
watchdog before publishing, persists any settlements even when a later store
operation errors, and clears the alarm only when no retryable rows remain.
Released Queue outcomes stay pending for the alarm and do not turn an already
committed command into an HTTP error.

Queue is intentionally not modeled as a full Distributed `Bus`: it has one
consumer and no fanout. `CelldQueueRelay` is generic over `MessagePublisher`,
and `BusPublisher<B>` can route to any Distributed `Bus`. The included Worker
consumer POSTs the canonical envelope to the authenticated native relay route.
The e2e-celld host wires that one route to `BusPublisher<NatsBus>`. Kafka,
RabbitMQ, and Knative use the same Queue consumer and relay contract; only the
native `BusPublisher<B>` changes.

## Queue naming and sharding

The producer binding name is local to the Worker and may be any valid binding
name. This fixture calls it `OUTBOX` because it is the aggregate's durable
egress port. The fleet-wide Queue resource is `distributed-outbox` because its
role is the durable handoff from those aggregate outboxes to the bus:

```json
{
  "queues": {
    "producers": [
      { "binding": "OUTBOX", "queue": "distributed-outbox" }
    ]
  }
}
```

Use one Queue per bounded context or independently deployed write service by
default. That keeps configuration, relay deployments, DLQs, redrive, and
monitoring proportional to operational boundaries rather than to the number of
domain types. A temporary downstream-bus outage backs up every Queue targeting
that bus, so additional Queue shards do not by themselves improve that failure
mode. Start with consumer concurrency, batching, retry/DLQ policy, queue-depth
monitoring, and idempotent downstream handlers.

Split a bounded context into one Queue per aggregate type only when the types
need independent throughput, availability, pause/purge/redrive controls,
security policy, or downstream transports. Each binding then names its own
Queue, for example `TODO_OUTBOX` -> `todo-outbox` and `CHAT_OUTBOX` ->
`chat-outbox`. This prevents one aggregate type's backlog from consuming
another type's relay capacity, at the cost of another Queue, consumer
registration, DLQ, and set of operational signals for every shard.

Do not create a Queue per aggregate instance. That makes Queue and consumer
configuration grow with domain cardinality, and it does not guarantee
end-to-end ordering once concurrent relay delivery reaches the downstream bus.
Preserve stable message ids and aggregate versions so projectors can deduplicate
and reject stale transitions instead of relying on Queue topology for
correctness.

## Local celld

Prerequisites: Docker (for NATS), celld 0.4, `worker-build`, and the
`wasm32-unknown-unknown` Rust target.

```sh
make -C tests/celld up
make -C tests/celld test
make -C tests/celld down
```

celld 0.4 development mode supplies its own local object store; Azurite, MinIO,
and an external S3 endpoint are not used. The canonical profile stores cell and
Queue data in `tests/celld/worker/.celld/dev`. `make down` and `make reload`
preserve that directory.

celld permits only one consumer script per Queue, and that script cannot also
export `fetch`, so it is deliberately a separate project. `celld dev` serves
one project at a time. `make up` delegates to the e2e-ui profile, which registers
`worker/relay.wrangler.jsonc`, stops that process, and then starts the aggregate
Worker from the same persistent `.celld/dev` store. The retained Queue row is
then delivered to the e2e-celld native host, accepted by NATS JetStream, and
applied by the normal Todo projection. A deployed fleet can keep the producer
and consumer deployments as separate scripts without changing the relay code.

Without `CELLD_URL`, `cargo test --test celld` checks the fixtures and skips the
live HTTP round trips.

Every non-health aggregate route requires `DISTRIBUTED_INTERNAL_SECRET`. The
checked-in value is loopback test data only; production requires a separately
provisioned secret plus the normal network policy.
