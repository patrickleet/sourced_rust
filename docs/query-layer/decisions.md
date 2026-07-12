---
id: 019f53bb-9f5a-7fc1-bf00-89d628a27e7e
slug: specs/query-layer/decisions
title: "Query layer — decisions, parity audit, open questions"
type: spec
status: active
priority: medium
tags: [graphql, query-layer]
---

# Decisions and parity

## Key decisions (with rationale)

1. **Feature on core crate, not a new crate** — stated repo convention;
   reuses `pub(crate)` SQL vocabulary; avoids publish-pipeline wiring.
2. **async-graphql dynamic schema** — only maintained option with runtime
   schema construction; the schema comes from a runtime registry.
3. **SDL renderer is dep-free and always compiled; runtime engine is
   feature-gated** — mirrors DDL rendering vs execution split; gives dctl a
   deterministic artifact without pulling a GraphQL library into the CLI.
4. **Single-statement JSON execution via correlated subqueries** — kills N+1
   structurally and routes around the bare-column-name decode limitation
   (the Hasura engine strategy), while staying dialect-portable: no LATERAL,
   so the same statement tree renders on Postgres (`jsonb_build_object`/
   `jsonb_agg`) and SQLite (`json_object`/`json_group_array`).
5. **SQLite execution ships in v1** — the flagship consumer
   (gitkb-domain-service) runs the SQLite store for its entire local dev
   loop; a Postgres-only engine would make `with_graphql` untestable where
   developers actually work. jsonb filter operators remain the one
   Postgres-gated capability.
6. **Engine owns its own schema catalog and (recommended for Postgres) its
   own pool** — the repository's registry is private and only dev-populated;
   sharing the 5-connection write pool would let reads starve commits.
   Sharing `repo.pool()` is fine on the SQLite dev loop.
7. **Code-first permissions, deny-by-default, schema-per-role** — matches
   framework philosophy (explicit Rust, no sidecar config), and role-shaped
   introspection is a feature Hasura users expect. `from_manifest` +
   deny-by-default makes partial rollout the default posture: registering
   everything exposes nothing until a role grants it.
8. **`Service::with_graphql` as the primary attachment point** — one builder
   step on the existing deployment-level `Service`, like `with_bus`; the
   existing `serve` path then exposes commands and queries from one process.
   Works because axum/matchit resolves static `POST /graphql` ahead of the
   `POST /{command}` wildcard; the `graphql` command name becomes reserved
   (build-time panic, consistent with duplicate-registration behavior). The
   standalone `graphql_router` remains for split-process deployments.
9. **Gateway-trust identity, no in-process JWT** — exactly the framework's
   documented trust boundary; keeps the engine agnostic to Zitadel/Hasura
   header vocabularies (claims are opaque `Session` keys).
10. **`TableKind` with `#[serde(default)]` instead of schema_version 2** —
    follows the crate's own backward-compat precedent.
11. **Hide `_sourced_version`; exclude `skipped` columns** — adapter-owned
    and explicitly-skipped data must not leak into a public API.
12. **PK-ascending default ordering** — deterministic pagination without
    client effort; matches the store's only existing ORDER BY convention.
13. **Subscriptions are live queries with commit-path invalidation** — the
    subscription document is the query document (the audited client's usage
    pattern), and refresh triggers come from the framework's own
    `ReadModelWritePlan` commit path (broadcast + Postgres NOTIFY) instead
    of interval polling: idle data costs zero queries, and the mutation
    ban stays absolute. Claims fix at connection init (Hasura semantics);
    re-auth is reconnect.
14. **Many-to-many via `target_foreign_key` + inference** — the far-side
    join column becomes first-class `RelationshipDef` metadata
    (serde-defaulted, batched into the phase-1 breaking release) and is
    inferred from the join table's column-level FKs when unambiguous, so
    well-formed join tables need no extra annotation; the GraphQL compiler
    traverses m2m with one extra JOIN in the same correlated-subquery
    shape, without touching the ORM include loader.
15. **Command mutations via `CommandRequest`, typed by derives** — the
    mutation root is an RPC facade over `Service::dispatch_request` (the
    envelope documented for query-layer actions), so table mutations stay
    structurally impossible; `GraphqlInput`/`GraphqlOutput` derives
    generate the type surface from the same structs handlers deserialize,
    eliminating Hasura's hand-maintained-actions.graphql drift; role
    gating is coarse (Hasura action-permission semantics) and handlers
    remain authoritative. Reinstated 2026-07-11 after initially being
    dropped the same day.
16. **Filesystem-as-policy: `src/query/`, one file per exposed model** —
    mirrors the `handlers/` + `routes!` convention on the read side;
    `ls src/query/` is the exposure list under deny-by-default, `roles.rs`
    is the single role vocabulary, and `graphql_models!` wires modules the
    way `routes!` does. Permissions deliberately do NOT live on the
    read-model derive: claims are gateway vocabulary, the ORM slice
    excludes authorization policy by charter, and shared readmodel crates
    serve multiple deployments with different policies.

## Hasura parity audit (internet-game season 1)

Audited a real prior system built on the same patterns with Hasura as the
query layer (`internet-game-meta`: ~26 read-model tables across game/
tournament/auth domains, Hasura metadata + SvelteKit client). What it
actually used, versus this design:

**Confirmed by the audit (already in this spec):**

- Zero `insert_permissions`/`update_permissions`/`delete_permissions`,
  zero event triggers, zero computed fields, zero remote schemas — their
  Hasura was read-only over projections. The read-only invariant matches
  real usage, not just principle.
- Anonymous role used heavily (public leaderboards, tournament state) —
  first-class here.
- Per-role row limits (`limit: 5/25/1` per select permission) — phase 3.
- Permission filters via `_exists` against a membership table keyed on
  `X-Hasura-User-Id` (backstab: "player in this game") — our
  relationship-EXISTS permission filters (`rel()`), phase 2.
- Relationship predicates in client `where`
  (`where: {players: {address: {_eq: $address}}}`) — pervasive in the
  client, on nearly every game query. **Consequence: EXISTS compilation
  should move from phase 3 into phase 2**; flat-only filtering would not
  have served this system.
- Nested relationship args (`messages(order_by: …, limit: 100)`) — phase 2.

**Gaps this audit exposes (dispositions):**

1. **Subscriptions are the backbone of the client** — 21 subscription
   operations: live game state by PK, live message feeds, live
   tournaments/leaderboards. Every game screen is a subscription, not a
   query. **Now in scope** (phase 4): see the Real-time subscriptions
   design section — commit-path invalidation beats Hasura's blind
   1s-interval re-polling because we own the projection write path.
2. **Aliased re-use of one relationship field with different args**
   (`my_scores: scores(where: …)` next to `scores`) and **nested
   relationship aggregates** (`tournament { nfts_aggregate { count } }`) —
   both used in anger. Aliases must work in phase 2 (the SQL compiler keys
   subqueries by alias, not field name); relationship-level `_aggregate`
   fields join root `_aggregate` in phase 3.
3. **Commands rode the same GraphQL endpoint** — every write was a Hasura
   action (`command_backstab_game_vote(...)`) proxying to the model
   service's `POST /{command}` handler, with per-role action permissions.
   Disposition: initially dropped (2026-07-11), **reinstated by decision
   later the same day** — now in scope as Command mutations (phase 5),
   dispatching through the `CommandRequest` envelope that was designed
   for exactly this shape, with typed signatures generated from the
   handlers' own input structs instead of a hand-maintained
   `actions.graphql`.
4. **Multiple databases under one endpoint** (readmodel + web3auth +
   event-store DBs in one Hasura). By-design difference: one engine = one
   database here. Their layout maps cleanly anyway — all game models
   shared one read DB (= one engine), web3auth was a separate service
   (= its own endpoint or gateway-level stitching); the event-store DB
   exposure (admin debugging) is deliberately out of scope.
5. **RESTified endpoints** (`GET /tournament-game-tokens/:nftId` for NFT
   marketplace metadata, backed by a saved query). Workaround today: a
   custom axum GET route composed beside the routers (note: microsvc has
   no GET data routes). Roadmap candidate only.
6. **Query allow-lists** (persisted/approved queries in production).
   Depth/complexity/limit clamps cover part of the risk; persisted-query
   allow-listing is a cheap future hardening flag on the engine.
7. Minor: Postgres `uuid` scalar (their ids) — `ColumnType` has no Uuid,
   so ids surface as `String`; cosmetic, no capability loss.

## Out-of-scope items: the supported idiom for each

Each exclusion has a positive answer, not just a "no". Assessed against
both audited systems (2026-07-11):

| Need | Supported idiom (no engine change) |
|---|---|
| Multi-role users (org-admin who is also a member) | Gateway concern by design: the gateway verifies a client-requested role against the token's granted roles and injects it as `x-role` — Hasura's `x-hasura-allowed-roles` pattern translated to the trust boundary this framework already mandates. The engine never sees more than one role per request. |
| Role inheritance (admin ⊇ support ⊇ user) | Permissions are plain Rust: share `fn member_view() -> SelectPermission` (or a whole `ModelPermissions` constructor) across roles in `src/query/`. Hasura needed inherited roles because YAML can't share code; code-first makes inheritance a function call. |
| Cursor pagination (deep OFFSET degrades; unstable under writes) | Keyset pagination is fully expressible, but the cursor must be **composite** (a bare `_lt` on the timestamp skips rows sharing the boundary value): `where: {_or: [{created_at: {_lt: $ts}}, {_and: [{created_at: {_eq: $ts}}, {id: {_gt: $id}}]}]}, order_by: [{created_at: desc}, {id: asc}], limit: N` — every operator used is phase-2 surface. Relay-spec `Connection` types stay out unless a Relay client materializes. |
| `distinct_on` / latest-row-per-group | Project it: in CQRS the "latest per group" view is its own read model, not a query trick. (`DISTINCT ON` is also PG-only — it would break dialect parity; if ever demanded, it follows the jsonb-ops PG-only capability-gate precedent.) |
| Ordering by relationship/aggregate fields | Project the sort key into the row — the audited leaderboards sort by a projected `rank` column, exactly this idiom. |
| `_stream`-style event cursors | Live-query subscriptions cover the audited usage (chat feed = subscription with `order_by: {timestamp: desc}, limit: 100`); true event streams are the bus/outbox's job. Resumable row-cursors could ride the same commit-path invalidation seam later. |

**Named roadmap item — federation subgraph mode**: per-service engines
compose across services via a graph router. Emitting
Apollo-Federation-compatible subgraph SDL is cheap and additive later
(`@key` from the primary key; entity resolver = the existing `_by_pk`
compilation), and the v1 naming/PK-argument design is already
federation-shaped. Not scheduled; recorded so nothing in v1 precludes it.

## Open questions

- ~~ManyToMany metadata gap~~ — **resolved**: `target_foreign_key` on
  `RelationshipDef` (serde-defaulted, inferable from the join table's
  column-level FKs when unambiguous) plus GraphQL m2m traversal are now
  in scope — see "Many-to-many traversal (normative)" in the
  implementation guide (metadata phase 1, traversal phase 2). Only the
  ORM include-loader's m2m support remains a separate follow-up task.
- ~~Timestamp normalization~~ — **resolved (implementation guide)**: expose
  the stored text form as-is in v1; no normalization.
- ~~Aggregate nullability/typing~~ — **resolved**: `count: Int!` (never
  null); per-column op fields all **nullable** (null when no rows match):
  `sum` → `BigInt` for Integer/UnsignedInteger columns, `Float` for Float;
  `avg` → `Float`; `min`/`max` → the column's own scalar, offered on
  numeric, `String`, and `Timestamptz` columns. Aggregate fields cover
  only columns the role can see, and appear only when the role's
  permission has `allow_aggregations(true)`.
- Whether the query endpoint should be declarable in `ServiceManifest`
  (a `QueryApiManifest` alongside the observability manifests) so deployment
  tooling can discover it — nice-to-have, phase 3 at earliest.
- Subscription fan-out topology at scale: table-level `NOTIFY` payloads are
  tiny, but very hot projections could dirty many subscriptions at once —
  whether the debounce window plus per-subscription min-interval suffices,
  or a shared re-execution cache per (compiled query, variables, role) is
  needed, is an implementation-time measurement, not a design blocker.

## References

- `docs/read-models.md` — relational read models, includes API, Non-Goals,
  and the explicit query-gateway positioning this spec fills.
- `src/table/metadata.rs`, `src/table/registry.rs` — the metadata backbone.
- `src/manifest.rs` — manifest envelope (schema_version 1) and builder.
- `src/microsvc/session.rs`, `README.md` Security/Trust Boundary — identity
  model.
- `src/microsvc/knative_ingress.rs` — composable-router precedent.
- `distributed_cli/src/atlas.rs`, `manifest_harness.rs` — artifact-renderer
  and harness precedents for the dctl integration.
- Prior art in-repo: `tests/distributed_read_model/query_service/mod.rs` and
  `tests/distributed_read_model_board/query_service/mod.rs` (hand-written
  read-only query services over includes — the pattern this generalizes).
