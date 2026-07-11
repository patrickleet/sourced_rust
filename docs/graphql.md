# GraphQL query service

Auto-generated, **read-only** GraphQL over relational read models — Hasura-style
filtering, ordering, pagination, relationships, role RBAC, live subscriptions
via commit-path invalidation, and command mutations that dispatch through
`CommandRequest`.

## Scope

| In | Out |
|---|---|
| `SELECT`-only query surface from `TableSchema` | Table mutations / write-to-projection |
| Role-based column allowlists + row filters | Authentication (trusted gateway injects claims) |
| SQLite + Postgres dialects | Cross-service federation |
| Command mutations (RPC facade) | Event-stream subscriptions |
| `dctl schema --format graphql` SDL artifact | ORM include-loader m2m (separate follow-up) |

## Enable

```toml
distributed = { version = "…", features = ["graphql", "sqlite"] } # or postgres
```

## Service layout (`src/query/`)

```
src/query/
  roles.rs           # role vocabulary constants
  orders.rs          # one file per exposed model: permissions()
  players.rs
  commands.rs        # GraphqlCommands registration
  mod.rs             # build_engine(pool) -> GraphqlEngine
```

## Quickstart

```rust
use distributed::graphql::{select, col, claim, GraphqlEngine};
use distributed::microsvc::{Service, Session};

let engine = GraphqlEngine::from_manifest(&manifest, pool)?
    .roles(&["user", "anonymous"])
    .grant_all("user") // or .model::<OrderView>(perms)
    .graphiql(true)    // GET /graphql → GraphiQL IDE (local only)
    .build()?;

let service = Service::new()
    .routes(routes)
    .with_graphql(engine);

// POST /graphql  — queries / mutations
// GET  /graphql  — GraphiQL when `.graphiql(true)` (default headers: x-role=user)
```

## GraphiQL (visual explorer)

Run the seeded playground (in-memory SQLite + sample orders):

```bash
cargo run --example graphiql --features "graphql,sqlite"
# open http://127.0.0.1:4000/graphql
```

Bind address: `GRAPHIQL_ADDR` (default `127.0.0.1:4000`).

Scaffolded services (`dctl scaffold --query-api`) enable GraphiQL by default.
Disable in production with `GRAPHIQL=0`.

Identity for the IDE is carried as HTTP headers (same trust model as the rest of
the HTTP surface — no built-in auth). GraphiQL pre-fills:

| Header | Default | Meaning |
|---|---|---|
| `x-role` | `user` | Role for deny-by-default grants |
| `x-user-id` | `demo` | Claim used by row filters |

Change them in GraphiQL’s **Headers** panel to exercise other roles.

## Permissions (deny by default)

```rust
use distributed::graphql::{select, col, claim, ModelPermissions};

ModelPermissions::new()
    .role("user", select()
        .all_columns()
        .filter(col("customer_id").eq(claim("x-user-id"))))
    .role("anonymous", select().columns(["order_id", "status"]));
```

## SDL artifact

```bash
dctl schema --format graphql --out schema.graphql
git diff --exit-code schema.graphql   # CI gate
```

`--dialect` is ignored with `--format graphql` (SDL is dialect-independent).

## Non-Goals (explicit)

- Writing to read-model tables through GraphQL
- Event streaming (`_stream` cursors)
- Remote schemas / joins
- Querying operational tables (`outbox_messages`, event store, …)
