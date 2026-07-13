# Single-service layout → microservice split

## Start as one service

1. One workspace root with `crates/*-domain`, `crates/readmodels`, `crates/service`.
2. Domain crates: aggregates, events, value objects only (feature-light on `distributed`).
3. `readmodels`: all BC tables in **one** package (group modules by BC).
4. `service`: command handlers + projectors registered as **composable route bundles**.
5. Thin **runner** chooses storage (SQLite/Postgres) and transport (SqliteBus/NATS/…).

```rust
// monolith
Service::new()
  .routes(catalog_routes(...))
  .routes(orders_routes(...))
```

## Split into microservices

Do **not** redefine the domain. Relocate **handler registration**:

```rust
// process A
Service::new().routes(catalog_routes(...))

// process B  
Service::new().routes(orders_routes(...))
```

Share:

- event store / bus (same `DATABASE_URL` or broker)
- read-model tables (or fan-out projections)

Add a **gateway** only for:

- unified GraphQL (both RM tables)
- routing commands by name prefix to the owning process

## Same test suite

Suite speaks HTTP/GraphQL against `WORKSHOP_BASE_URL` only.  
Monolith and multi-service must pass the same case IDs — never fork expected outcomes.
