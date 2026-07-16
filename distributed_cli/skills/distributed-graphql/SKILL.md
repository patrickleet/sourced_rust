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
use distributed::graphql::{read, col, claim, ModelPermissions};
use crate::read_models::OrderView;

pub type Model = OrderView;

pub fn permissions() -> ModelPermissions<OrderView> {
    ModelPermissions::new()
        .grant("user", read()
            .all_columns()
            .rows(col("customer_id").eq(claim("x-user-id"))))
        .grant("anonymous", read().columns(["order_id", "status"]))
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
let engine = query::build_engine(pool)?; // scaffold enables GraphiQL unless GRAPHIQL=0
let service = Service::new()
    .routes(routes)
    .with_graphql(engine);
// POST /graphql  — queries / mutations
// GET  /graphql  — GraphiQL IDE when `.graphiql(true)`
```

### GraphiQL (local visual explorer)

```bash
# Framework playground (seeded orders, no scaffold needed):
cargo run --example graphiql --features "graphql,sqlite"
# → http://127.0.0.1:4000/graphql

# Or run your scaffolded service and open GET /graphql
GRAPHIQL=0 cargo run   # disable IDE in production
```

Default GraphiQL headers: `x-role: user`, `x-user-id: demo`. Edit in the IDE
Headers panel. See `GitKB specs/query-layer/index`.

### Identity (public GraphQL)

Scaffold **always** wires `public_oidc_identity_from_env()` → **`OidcBearer`** +
`require_auth=true` (D6). Set `OIDC_ISSUER` + `OIDC_AUDIENCE` (or `OIDC_CLIENT_ID`).
If unset, placeholder issuer still uses OidcBearer (401 without Bearer) — **never**
ambient `DevHeaders` on the public scaffold path.

| Mode | Use |
|---|---|
| `OidcBearer` | Public API — validate `Authorization: Bearer` JWT (access token) |
| `Hybrid` | Bearer preferred; else gateway-injected headers |
| `TrustedProxy` | Mesh — strip client identity denylist at process |
| `DevHeaders` | Local GraphiQL / unit tests only (explicit opt-in) |

Wire via `.identity(distributed::graphql::public_oidc_identity_from_env())`.
Never trust raw client `x-user-id` / `x-role` on a public edge.

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

- Framework docs: `GitKB specs/query-layer/index` in the Distributed repo
- Spec: `specs/query-service-graphql` (normative)
