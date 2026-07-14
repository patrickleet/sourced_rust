# SvelteKit + Distributed GraphQL — Developer Experience Spec

**Status:** design / DX contract for follow-on implementation  
**Reference app:** `tests/e2e-ui/ui` (this fixture)  
**Audience:** authors of SvelteKit apps against a Distributed service with GraphQL enabled  

This document answers: what is the best DX today, what boilerplate should become `$lib` then an npm package, and how to type queries / mutations / subscriptions from Distributed backend definitions.

---

## 1. Goals

1. **One mental model** for talking to the API: documents + auth + result shape, whether the call is SSR, browser, or WebSocket.
2. **Command mutations are client → GraphQL**, not SvelteKit form actions that re-implement RPC.
3. **SSR only seeds reads** (same query documents the client reuses for reconcile / cache).
4. **Types flow from the backend** so `todos_create` variables and selection results are not hand-maintained twice.
5. **Boilerplate becomes a package** (`@distributed/sveltekit-graphql` working name) so greenfield apps do not re-copy Auth.js OIDC, Vite proxy, or fetch wrappers.

Non-goals for this spec: publishing the package to npm in this iteration; full CI codegen wiring; redesigning Auth.js / Zitadel.

---

## 2. Current state inventory (e2e-ui)

All paths below exist under `tests/e2e-ui/ui/src/` unless noted.

| Module | Role | Exists |
|--------|------|--------|
| `lib/gql/documents.ts` | Shared GraphQL document strings (`TODOS_QUERY`, `TODOS_CREATE`, …) | yes |
| `lib/gql/client.ts` | **Browser** `browserGraphql()` → `fetch('/graphql')` + Bearer / DevHeaders | yes |
| `lib/server/graphql.ts` | **SSR** `serverGraphql()` → `fetch(apiBase() + '/graphql')` + same auth shape | yes |
| `lib/graphql-ws.ts` | `subscribe()` — `graphql-transport-ws`, Bearer in `connection_init` | yes |
| `lib/roles.ts` | Pure mapping of OIDC groups → engine role (`user` / `admin`) | yes |
| `lib/session.ts` | Thin session re-export helpers | yes |
| `auth.ts` | Auth.js OIDC (Zitadel), tokens in encrypted session JWT | yes |
| `hooks.server.ts` | Protect `/todos`, `/chat`, `/session`; Auth handle sequence | yes |
| `routes/todos/+page.server.ts` | SSR seed only via `TODOS_QUERY` | yes |
| `routes/todos/+page.svelte` | Optimistic UI; mutations via `browserGraphql` | yes |
| `routes/chat/+page.server.ts` | SSR seed via shared chat query | yes |
| `routes/chat/+page.svelte` | Subscription + `CHAT_POST` mutation in browser | yes |
| Vite `server.proxy['/graphql']` | Dev same-origin proxy to API | yes |
| **Unified app-level GraphQL client** (one factory used by SSR + browser + WS) | — | **no** (dual HTTP clients + separate WS helper) |
| **Generated TS types** from Distributed SDL / `GraphqlInput` | — | **no** (hand types in routes) |

### Data flow today (recommended product shape, already partially implemented)

```text
SSR load(+page.server.ts)
  └─ serverGraphql(TODOS_QUERY, { accessToken })  →  absolute API origin

Browser mutation
  └─ browserGraphql(TODOS_CREATE, auth, vars)     →  POST /graphql (Vite → API)

Browser subscription
  └─ subscribe(CHAT_SUB, auth, handlers)          →  WS /graphql/ws + connection_init Bearer

API (Distributed)
  └─ OidcBearer → role schema → query / command mutation → handlers / projectors
```

### Boilerplate still repeated in the fixture

| Concern | Duplication today |
|---------|-------------------|
| Auth header construction | Nearly identical in `browserGraphql`, `serverGraphql`, `subscribe` |
| `GqlResult` / error / 401 messaging | Duplicated between browser + server |
| Base URL resolution | Server only; browser hardcodes relative `/graphql` |
| Per-page `gqlAuth` derived from session | Hand-rolled in todos + chat |
| Document strings | Shared file (good) but no typed operations object |
| Role from groups | Shared `roles.ts` (good) but not wired into a client factory |
| Env quote cleanup | Server-only; Auth.js has parallel `cleanEnvValue` |
| Optimistic merge / projector lag | Route-local; not packaged |
| OIDC Auth.js provider block | Entire `auth.ts` is copy-paste for every app |

---

## 3. Dual client vs single GraphQL client — recommendation

### Options

| Approach | Pros | Cons |
|----------|------|------|
| **A. Dual clients** (status quo): `browserGraphql` + `serverGraphql` + `subscribe` | Simple to understand; no isomorphic bundling traps; `$env/dynamic/private` stays server-only | Three places to fix auth/errors; types duplicated; easy for routes to diverge |
| **B. Heavy framework client** (urql / Apollo / TanStack Query GraphQL) | Cache, hooks, ecosystem | Overkill for command+query Distributed surface; WS protocol still custom; pulls large deps into e2e fixture |
| **C. Thin unified client (recommended)** | One `createGraphqlClient({ getUrl, getAuth })`; same `request()` for SSR+browser; `subscribe()` on same auth type; documents stay plain strings or typed ops | Need careful split of private env (getUrl injected from server load, not imported from `$env` in shared modules) |

### Preferred DX: **C — thin unified client**

**Rationale**

1. Distributed apps do not need a normalized entity cache first; they need **identical documents**, **identical auth**, and **predictable errors**.
2. SSR and browser differ only in **URL** (absolute API vs same-origin `/graphql`) and **where the access token comes from** (Auth.js `locals` vs page data / session store). That is an injection problem, not two libraries.
3. WebSocket auth should reuse the same `GqlAuth` type and document module as HTTP.
4. Avoid importing `$env/dynamic/private` from isomorphic modules (breaks client bundle). The **factory** is isomorphic; **adapters** are not:

```ts
// Proposed package surface (isomorphic core)
createGraphqlClient({
  getUrl: () => string,           // browser: '/graphql'; SSR: apiBase()
  getAuth: () => GqlAuth | Promise<GqlAuth>,
}): {
  request<T>(document: string, variables?: Record<string, unknown>): Promise<GqlResult<T>>
  // browser-only helper may live beside, reusing GqlAuth:
  // subscribe(document, handlers)
}

// App wiring (SSR)
const gql = createGraphqlClient({
  getUrl: () => apiBase() + '/graphql',
  getAuth: () => authFromSession(session),
});

// App wiring (browser — once per layout)
const gql = createGraphqlClient({
  getUrl: () => '/graphql',
  getAuth: () => authFromPageData(page.data),
});
```

**Do not** put mutations behind SvelteKit form actions as the primary path: that hides the GraphQL network call and forces a second transport. Forms may remain progressive enhancement only if they call the same `request()` on the server with the same documents (optional).

**Pilot for this fixture:** migrate `browserGraphql` / `serverGraphql` bodies to a shared `requestGraphql(url, auth, document, variables)` (pilot below), then promote to npm package.

---

## 4. Extractable `$lib` → npm package surface

### Package working name

`@distributed/sveltekit-graphql` (or `@hops-ops/distributed-sveltekit` — name TBD at publish time).

### Suggested package layout

```text
@distributed/sveltekit-graphql
  src/
    types.ts          # GqlResult, GqlAuth, GqlError
    request.ts        # requestGraphql(url, auth, document, variables)
    client.ts         # createGraphqlClient({ getUrl, getAuth })
    subscribe.ts      # graphql-transport-ws subscribe (from e2e-ui graphql-ws.ts)
    auth-headers.ts   # bearer vs DevHeaders
    sveltekit/
      load.ts         # helpers: loadQuery(event, document, mapSession)
      session-auth.ts # session → GqlAuth (accessToken, role)
    vite/
      proxy.ts        # default proxy snippet or helper for vite.config
  package.json
```

### What stays in the app (not in the package)

| App owns | Why |
|----------|-----|
| Domain documents (`TODOS_QUERY`, …) or codegen outputs | Product-specific |
| Auth.js / OIDC provider config (or thin template) | Issuer/client differ per deploy |
| Route UI, optimistic policies | Product-specific |
| Engine role names mapping rules | Can share defaults (`user`/`admin`) but app may extend |

### Optional second package later

`@distributed/authjs-oidc` — Auth.js OIDC provider + refresh + env cleaning, shared with the-website patterns. Not required for GraphQL DX v1.

### Automatic / “batteries” behaviors worth packaging

| Behavior | Automation idea |
|----------|-----------------|
| Vite `/graphql` + `/graphql/ws` proxy | `distributedSveltekitProxy({ apiOrigin })` in `vite.config.ts` |
| 401 messaging | Standardized error codes: `UNAUTHENTICATED`, `BEARER_REJECTED`, `MISSING_TOKEN` |
| Role from groups | Default claim map aligned with Distributed OIDC claim_map |
| SSR seed helper | `export const load = gqlLoad(TODOS_QUERY, (d) => ({ todos: d.todos }))` |
| Document co-location | Enforce via lint: mutations only imported from `documents` / generated ops |
| Codegen script | `distributed-gql codegen --sdl url|file --out src/lib/gql/generated` |

---

## 5. TypeScript types from Distributed backend

### What the backend already has

| Artifact | Location / API | Useful for |
|----------|----------------|------------|
| Per-role **SDL** | `GraphqlEngine::sdl_for_role(role)` | Full query/mutation/subscription schema for that role |
| Table-driven SDL | `graphql_sdl_for_tables` / `dctl schema --format graphql` (see `docs/graphql.md`) | Read-model types without running the service |
| Command input/output defs | `#[derive(GraphqlInput)]` / `GraphqlOutput` → `GraphqlTypeDef` | Mutation variable + payload field names/types |
| Exposed command registry | `GraphqlCommands` + `field_name` (e.g. `todos_create`) | Mutation root field names |
| Runtime GraphiQL | `POST /graphql` with introspection (if enabled) | Dev-time codegen |

There is **no** first-class “emit `.ts` files” pipeline yet. The DX path is: **export SDL (and optionally a commands JSON) → TypeScript via existing industry tools**.

### Recommended typing pipeline (phased)

#### Phase 1 — Near term (fixture / single app)

1. **Export SDL for role `user`** at build or `make gen`:
   - Runtime: small Rust bin or `curl` GraphQL introspection if GraphiQL/introspection on in dev.
   - Prefer offline: boot engine in a `build.rs` / `xtask` with SQLite memory + same `build_graphql_engine` as the service, write `schema.user.graphql`.
2. **graphql-codegen** (or gql.tada / typed-document-node):
   - Input: `schema.user.graphql` + `src/lib/gql/**/*.ts` documents (or `.graphql` files).
   - Output: `src/lib/gql/generated/graphql.ts` with `TypedDocumentNode` + result/variables types.
3. Wrap `createGraphqlClient().request` so `request(TODOS_CREATE, vars)` infers vars and data.

#### Phase 2 — Service package contract

1. Each Distributed service crate exposes:
   - `cargo run -p e2e-runner -- export-sdl --role user > schema.graphql`, **or**
   - `dctl schema --format graphql` for read models + a **commands fragment** generated from `GraphqlCommands` + `GraphqlTypeDef` (new small exporter — design only until implemented).
2. UI repo depends on published schema artifact (git submodule, workspace path, or versioned file in npm `@myco/e2e-api-schema`).

#### Phase 3 — Deeper Distributed → TS (optional)

| Source | Target | Notes |
|--------|--------|-------|
| `GraphqlTypeDef` JSON dump | `interface TodosCreateInput { … }` | Avoids full schema for mutation-only clients |
| ReadModel / `#[table]` | Query result types | Overlaps SDL; prefer SDL as single source |
| Rust `TodoCreateInput` | TS via `typeshare` / `ts-rs` | Useful for non-GraphQL clients; for GraphQL, SDL stays canonical |

**Canonical choice for GraphQL clients: SDL (per role) + codegen.**  
`GraphqlInput`/`GraphqlOutput` should stay aligned with that SDL (they already drive mutation field args/types in the engine).

### Subscription typing

- Same schema includes `Subscription` fields (e.g. `todos`, `chat_messages`).
- Codegen produces subscription document types; `subscribe()` accepts `TypedDocumentNode` and types `onNext` payload as `{ data?: TData }`.

### Auth types (separate from GraphQL schema)

```ts
type GqlAuth = {
  accessToken?: string | null;
  userId?: string | null;  // DevHeaders only
  role?: string | null;
};
```

Role union may be generated from service `roles::ALL` export (`"user" | "admin"`).

---

## 6. Target DX for a new SvelteKit app (story)

```text
1. cargo new service with Distributed GraphQL + OIDC (or copy e2e-ui crates)
2. npm create svelte@latest my-app
3. npm i @distributed/sveltekit-graphql
4. Add vite proxy helper + env: PUBLIC_API_ORIGIN / server API origin
5. Wire Auth.js OIDC (template) — accessToken on session
6. make gen-schema  → schema.user.graphql
7. make gen-gql     → typed documents
8. +page.server.ts:
     const data = await gql.request(TodosDocument)
9. +page.svelte:
     await gql.request(TodosCreateDocument, { todo_id, title })
     // Network: POST /graphql
10. subscribe(ChatMessagesSubDocument, handlers)
```

Developer never hand-writes `fetch` headers or duplicates query strings.

---

## 7. Pilot already done / in progress in this fixture

| Item | Status |
|------|--------|
| Shared `documents.ts` | Done |
| Browser mutations (not form actions) | Done |
| SSR seed with same query | Done |
| WS `connection_init` Bearer | Done |
| Dual `GqlResult` / auth header logic | **Pilot done:** `lib/gql/request.ts` + shared `types.ts` |
| `createGraphqlClient` factory | **Pilot done:** `lib/gql/create-client.ts` (SSR helper `createServerGraphqlClient`) |
| Full codegen from SDL | Spec only (backlog) |

---

## 8. Prioritized backlog

### Must (next implementation PRs)

1. ~~**Unify HTTP GraphQL request path**~~ — **done (pilot):** `lib/gql/request.ts` + wrappers.
2. ~~**App-level client factory**~~ — **done (pilot):** `createGraphqlClient` / `createServerGraphqlClient` (wire into layout still open).
3. **Central `authFromSession` / `authFromPageData`** — kill per-route `gqlAuth` copies; use factory in `+layout`.
4. **SDL export for e2e-ui** — `xtask` or test binary writing `schema.user.graphql` via `build_graphql_engine(...).sdl_for_role("user")`.
5. ~~**Document this layout**~~ — **done:** this file + `layout.md` link.

### Should

6. **graphql-codegen** (or gql.tada) wired in `ui/package.json` scripts against exported SDL.
7. **Typed `subscribe`** reusing `GqlAuth` + generated subscription docs.
8. **npm package scaffold** in-repo (`packages/sveltekit-graphql`) with the unified client + WS; e2e-ui depends via workspace path.
9. **Vite proxy helper** exported from the package.
10. **Error taxonomy** shared with API codes (`UNAUTHENTICATED`, etc.).

### Later

11. Publish package to registry; version with Distributed releases.
12. `dctl` / service `export-sdl` + `export-commands-json` as first-class CLI.
13. Auth.js OIDC template package shared with the-website.
14. Optimistic-list helpers (projector lag merge) as optional utilities.
15. ESLint rule: no raw `fetch('/graphql')` outside client package.

---

## 9. Open questions (non-blocking)

- Package scope: GraphQL-only vs GraphQL + Auth.js template.
- Codegen tool lock-in: graphql-codegen vs gql.tada vs GraphQL Code Generator client-preset.
- Whether introspection stays on in production GraphiQL-off builds (prefer offline SDL export).

---

## 10. References

- Fixture GraphQL modules: `ui/src/lib/gql/*`, `ui/src/lib/server/graphql.ts`, `ui/src/lib/graphql-ws.ts`
- Distributed GraphQL overview: `docs/graphql.md`
- Engine SDL: `GraphqlEngine::sdl_for_role`, `graphql_sdl_for_tables`
- Command typing: `GraphqlInput` / `GraphqlOutput` derives, `exposed_command().input::<T>().output::<U>()`
- Fixture crate layout: `docs/layout.md` (this folder)
