---
name: distributed-graphql
description: Expose auto-generated read-only GraphQL over Distributed read models (permissions, dctl schema --format graphql, with_graphql, command mutations). Use when adding a GraphQL query API, roles, model exposure, subscriptions, or command mutations.
---

# GraphQL query service

Enable the framework features and mount one endpoint:

```toml
distributed = { path = "...", features = ["graphql", "sqlite"] } # or postgres
```

## Service layout (`src/query/`)

```
src/query/
  mod.rs          # build_engine(pool) -> GraphqlEngine
  roles.rs        # role name constants
  commands.rs     # GraphqlCommands (optional)
  <model>.rs      # one file per exposed model: permissions()
```

### Add a model exposure

1. Ensure the read model is in `distributed_manifest()`.
2. Add `src/query/<table>.rs`:

```rust
use distributed::graphql::{select, col, claim, ModelPermissions};
use crate::read_models::OrderView;

pub type Model = OrderView;

pub fn permissions() -> ModelPermissions<OrderView> {
    ModelPermissions::new()
        .role("user", select()
            .all_columns()
            .filter(col("customer_id").eq(claim("x-user-id"))))
        .role("anonymous", select().columns(["order_id", "status"]))
}
```

3. Register it in `src/query/mod.rs` via `graphql_models!(builder, order_view)` or
   `.model::<OrderView>(order_view::permissions())`.

### Roles

Declare the vocabulary with `.roles(&["user", "anonymous"])`. Deny-by-default:
a role sees nothing until granted via `.model(...)` / `.grant_all(role)` /
`.permission(...)`.

## Wire the service

```rust
let engine = query::build_engine(pool)?;
let service = Service::new()
    .routes(routes)
    .with_graphql(engine);
// POST /graphql  (microsvc::router mounts it)
```

Pass `change_stream(repo.read_model_changes())` for live subscriptions.

Command mutations: register `GraphqlCommands` on the builder and use
`graphql_router_with_service` / `with_graphql` so `Request::data` carries `Service`.

## SDL artifact (CI gate)

```bash
dctl schema --format graphql --out schema.graphql
git diff --exit-code schema.graphql
```

`--dialect` is ignored with `--format graphql` (dialect-independent core surface).

## Scaffold

```bash
dctl scaffold my-service --query-api --read-models --store sqlite
```

Emits `src/query/` skeleton + `graphql` feature + `DATABASE_URL` / pool wiring notes.

## Reference

- Framework docs: `docs/graphql.md` in the Distributed repo
- Spec: `specs/query-service-graphql` (normative)
