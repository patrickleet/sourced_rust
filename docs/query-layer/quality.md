---
id: 019f53ab-e7a9-7c71-b13d-eddf64cc07fe
slug: specs/query-layer/quality
title: "Query layer — quality bar and evidence"
type: spec
status: active
priority: high
tags: [graphql, query-layer, spec]
---

# Quality bar

### Test plan (files, per phase)

| File | Phase | What |
|---|---|---|
| `tests/graphql_sdl/main.rs` + `golden/*.graphql` | 1 | SDL golden files over fixture models: composite PK, jsonb, skip_query absence, has_many/belongs_to, FK-nullability, relationship-target-absent-from-input → field omitted, m2m emitted (target+through present) and omitted (through absent), target_foreign_key inference + ambiguity `Err`, invalid-name and collision `Err` cases |
| `src/table` unit tests | 1 | TableKind serde default + validate untouched |
| `distributed_cli/tests/cli_manifest.rs` | 1 | `--format graphql` e2e (ignored-by-default like existing) |
| `tests/graphql_compile/main.rs` | 2 | golden SQL per (operation, dialect); bind-order assertions; alias duplication; empty `_in`; chunking at >40 fields; m2m join subquery and `rel()`-through-m2m EXISTS |
| `tests/graphql_engine/main.rs` | 2 | build() error cases; role schemas; sdl_for_role⇄SDL conformance (grant_all vs renderer, normalized) |
| `tests/graphql_sqlite/main.rs` | 2 | end-to-end over temp-file SQLite: permissions, filters, relationships, pagination |
| `tests/graphql_postgres/main.rs` | 2 | same over compose Postgres (env-gated, `DATABASE_URL`) |
| `tests/graphql_subscriptions_{sqlite,postgres}/main.rs` | 4 | push-on-commit, debounce coalescing, hash-gating, claim-fixed reconnect |
| `tests/graphql_commands/main.rs` | 5 | derive mapping golden (input/output types incl. Option/Vec/nested/Value); role-shaped Mutation roots (zero-command role → no root); status→error mapping; e2e mutation→handler→projection→query loop; no-dispatcher INTERNAL on standalone router |


---

## Cross-cutting evidence

| Area | Spec |
|---|---|
| AuthZ isolation | [[specs/query-layer/authorization]] |
| Injection / limits | [[specs/query-layer/security]] |
| HTTP / GraphiQL | [[specs/query-layer/http]] |
| Relationships | [[specs/query-layer/relationships]] |
| Metrics | [[specs/query-layer/observability]] |
| Compiler goldens | production `compile_root`, both dialects |
| Postgres | env-gated `DATABASE_URL` |

Drive shipped entry points only.


## Agent seams (compiler goldens + Postgres)

### Compiler goldens (`tests/graphql_compile`)

1. Call **`compile_root`** (same as production) — not only `compile_list_sql_for_test`.
2. Build a minimal `EngineInner` via `GraphqlEngine::builder(pool).…build()` then
   compile through public execute path **or** export/test-only access that shares
   the production `compile_root` function (already `pub fn compile_root` in compile.rs).
3. Assert for **both** dialects (set dialect on engine / inner as shipped):
   - SQL contains bound placeholders (`?` vs `$1`) not raw claim strings
   - permission filter AND client where both present
   - `LIMIT`/`OFFSET` use binds
4. Feature gate: `#![cfg(feature = "graphql")]` (+ sqlite for pool if needed).

### Postgres suite (`tests/graphql_postgres`)

```rust
let url = match std::env::var("DATABASE_URL") {
  Ok(u) if !u.is_empty() => u,
  _ => { eprintln!("skip: DATABASE_URL unset"); return; }
};
// connect, create table, GraphqlEngine::builder, execute one list+where
```

CI without Postgres must skip (exit success).
