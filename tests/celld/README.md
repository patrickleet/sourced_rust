# celld 0.4 aggregate cells, Queue, and bus relay

This fixture exercises the write side of the Distributed flow:

```text
command -> AggregateCell -> domain event -> same-cell outbox
        -> celld EVENTS Queue -> Worker relay -> Distributed Bus -> NATS
```

The aggregate Worker owns one SQLite Durable Object per Todo or Chat shard.
`AggregateCell::durable_state` assembles the event log, command ledger,
snapshot/sealed state, and outbox into one versioned envelope. The Worker saves
that envelope with one SQLite upsert, so the aggregate mutation and its outbox
are one cell commit. `AggregateCell::outbox_dispatcher` then publishes through
`CelldQueuePublisher`. celld's output gate holds Queue egress until the cell
write is durable. Queue acceptance then settles the outbox row with a second
single-state upsert. If that settlement is interrupted, the same stable event
id may be delivered again; consumers must deduplicate by that id.

Queue is intentionally not modeled as a full Distributed `Bus`: it has one
consumer and no fanout. `CelldQueueRelay` is generic over `MessagePublisher`,
and `BusPublisher<B>` can route to any Distributed `Bus`. The included Worker
consumer POSTs the canonical envelope to the authenticated native relay route.
The e2e-celld host wires that one route to `BusPublisher<NatsBus>`. Kafka,
RabbitMQ, and Knative use the same Queue consumer and relay contract; only the
native `BusPublisher<B>` changes.

## Local celld

Prerequisites: Docker, `worker-build`, and the `wasm32-unknown-unknown` Rust
target.

```sh
make -C tests/celld up
make -C tests/celld test
```

celld 0.4 development mode supplies its own local object store; Azurite, MinIO,
and an external S3 endpoint are not used. Docker stores `PROJECT/.celld/dev` in
the named `celld-dev-data` volume. `make down` and `make reload` preserve it.
Only the explicit `make reset` target deletes cell and Queue data.

celld permits only one consumer script per Queue, and that script cannot also
export `fetch`, so it is deliberately a separate project. `celld dev` serves
one project at a time; the live local proof uses the same `.celld/dev` store
while switching from the aggregate producer project to
`worker/relay.wrangler.jsonc`. The retained Queue row is then delivered to the
e2e-celld native host, accepted by NATS JetStream, and applied by the normal Todo
projection. A deployed fleet can keep the producer and consumer deployments as
separate scripts without changing the relay code.

Without `CELLD_URL`, `cargo test --test celld` checks the fixtures and skips the
live HTTP round trips.

Every non-health aggregate route requires `DISTRIBUTED_INTERNAL_SECRET`. The
checked-in value is loopback test data only; production requires a separately
provisioned secret plus the normal network policy.
