# Diagnostics

Distributed diagnostics are a private JSON snapshot for operators and agents.
They summarize service inventory, readiness, telemetry capabilities, outbox
backlog, recent sanitized failures, causal hints, and suggested next checks.

Diagnostics are disabled by default. `microsvc::router(...)` and
`microsvc::serve(...)` do not expose the endpoint. A service must explicitly
compose it:

```rust
let app = distributed::microsvc::router_with_diagnostics(
    service,
    distributed::diagnostics::DiagnosticsOptions::new()
        .with_bearer_token("management-token"),
);
```

The default path is `GET /_distributed/diagnostics`. Responses always include
`Cache-Control: no-store`, even though snapshots are internally cached for a
short TTL.

## Privacy Boundary

Treat diagnostics as reconnaissance-grade private data. It includes command and
event names, transport names, readiness state, backlog summaries, recent failure
timing, correlation IDs, trace IDs, and runbook hints. Do not expose it on the
same unauthenticated scrape surface as `/metrics`.

Use one of these controls:

- localhost or management-only side port;
- cluster-internal service with NetworkPolicy;
- mTLS or service-mesh authorization;
- trusted proxy authentication;
- the `DiagnosticsOptions::with_access_check(...)` hook or
  `with_bearer_token(...)` helper.

The snapshot intentionally omits payloads, decoded handler input, session
variables, arbitrary metadata, auth headers, cookies, environment variables, DB
URLs, and secrets. Recent failures are sanitized before entering the in-memory
ring buffer and are bounded by capacity, per-response limits, dropped counts,
and truncation metadata.

## Backlog Provider

Diagnostics can include outbox backlog state when the service supplies an
outbox store:

```rust
let diagnostics = distributed::diagnostics::DiagnosticsOptions::new()
    .with_outbox_store(repo.outbox_store());
```

The snapshot uses `OutboxStore::backlog_stats`, so it reports count and oldest
age summaries rather than raw rows.
