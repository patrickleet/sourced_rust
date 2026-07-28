# e2e-ui template

Copyable **Distributed** service + SvelteKit UI. This README is the **code
index**: where each “that feels like a real product” behavior lives, then how
to run and test it.

```bash
cd tests/e2e-ui
make up          # Postgres :5433 + Zitadel :18080 → e2e-ui.env
set -a && source e2e-ui.env && set +a
make run         # API :8791 + UI :5180
```

| URL | What |
|-----|------|
| http://127.0.0.1:5180/todos | Owner-scoped todos |
| http://127.0.0.1:5180/chat | Lobby + live WS |
| http://127.0.0.1:5180/blob | Projected map game |
| http://127.0.0.1:5180/admin | Elevated client surface |
| http://127.0.0.1:5180/login | Custom Login V2 (`alice` / `Password1!`) |
| http://127.0.0.1:8791/graphql | GraphiQL |

Demo users: `alice` / `bob` / `admin` · password `Password1!`

---

## Code index

### UI — routes that teach the pattern

Pages stay thin: co-located GraphQL for reads, generated commands for writes,
one replica for SSR + client + live.

| Path | What to notice |
|------|----------------|
| [`ui/src/routes/todos/+page.graphql`](ui/src/routes/todos/+page.graphql) | `query Todos @load` — compiler-owned SSR seed; no second client document. |
| [`ui/src/routes/todos/+page.svelte`](ui/src/routes/todos/+page.svelte) | `Todos.use()` + `commands.todo.*`. Comment at top: no app cache adapter, no hand optimistic recipe. |
| [`ui/src/routes/chat/+page.graphql`](ui/src/routes/chat/+page.graphql) | `@load @live` on **one** query — SSR, cache, and WS companion from the same declaration. |
| [`ui/src/routes/chat/+page.svelte`](ui/src/routes/chat/+page.svelte) | `ChatMessages.use()` defaults live because the artifact has a companion; post via `commands.chat.post`. |
| [`ui/src/routes/blob/[[gameId]]/+page.graphql`](ui/src/routes/blob/[[gameId]]/+page.graphql) | `blob_games` + `owner { … }` join; `@load` seeds the board list. |
| [`ui/src/routes/blob/[[gameId]]/+page.svelte`](ui/src/routes/blob/[[gameId]]/+page.svelte) | URL selects game; keyboard → `commands.blob.move`; board/`score` from replica (projected payload). |
| [`ui/src/routes/+layout.server.ts`](ui/src/routes/+layout.server.ts) | `createDistributedSvelteKitServer({ routes: DISTRIBUTED_ROUTE_OPERATIONS, … })` — one loader for all user `@load` ops. |
| [`ui/src/routes/+layout.svelte`](ui/src/routes/+layout.svelte) | `provideDistributed` + hydrate on navigation; session source shared by HTTP, WS, commands. |
| [`ui/src/routes/admin/+layout.server.ts`](ui/src/routes/admin/+layout.server.ts) | **Separate** `$distributed/admin` surface; 403 before any elevated GraphQL runs. |
| [`ui/src/routes/admin/+page.graphql`](ui/src/routes/admin/+page.graphql) | Admin-only document — never imported by the user client tree. |
| [`ui/src/auth.ts`](ui/src/auth.ts) | Auth.js + Zitadel scopes/groups; access token on session for GraphQL Bearer. |
| [`ui/src/routes/login/+page.server.ts`](ui/src/routes/login/+page.server.ts) | Custom Login V2 (Session API + CreateCallback), not stock Zitadel login UI. |
| [`ui/src/lib/server/zitadel-session.ts`](ui/src/lib/server/zitadel-session.ts) | Server-only PAT path for login/signup. |
| [`ui/src/lib/roles.ts`](ui/src/lib/roles.ts) | IdP groups → engine `user` / `admin`. |

Generated (do not hand-edit; `make gen-client`):

| Path | Role |
|------|------|
| [`ui/src/lib/generated/user/`](ui/src/lib/generated/user/) | Ops, commands, route registry, SvelteKit adapter for `e2e-ui` |
| [`ui/src/lib/generated/admin/`](ui/src/lib/generated/admin/) | Same for `e2e-ui-admin` |

`$distributed` re-exports the user surface; admin layout imports `$distributed/admin`.

### Rust — domain → inventory → edge

| Path | What to notice |
|------|----------------|
| [`crates/todo-domain/src/models/todo.rs`](crates/todo-domain/src/models/todo.rs) | Plain struct + `ensure_owner` + `@sourced` command methods. |
| [`crates/chat-domain/src/models/chat_message.rs`](crates/chat-domain/src/models/chat_message.rs) | Minimal post aggregate. |
| [`crates/blob-domain/src/models/blob_game.rs`](crates/blob-domain/src/models/blob_game.rs) | Move / level logic; emits facts the projector (or Projected path) maps. |
| [`crates/readmodels/src/models/todo_view.rs`](crates/readmodels/src/models/todo_view.rs) | `#[table("todos")]` read model. |
| [`crates/readmodels/src/models/blob_game_view.rs`](crates/readmodels/src/models/blob_game_view.rs) | Projected row + `belongs_to` `AuthUserView` owner join. |
| [`crates/readmodels/src/models/chat_message_view.rs`](crates/readmodels/src/models/chat_message_view.rs) | Chat row + author join. |
| [`crates/service/src/service.rs`](crates/service/src/service.rs) | **Center of gravity**: dual client surfaces, projectors vs direct projection, RLS grants, typed commands, OIDC claim map. |
| [`crates/service/src/handlers/commands/create.rs`](crates/service/src/handlers/commands/create.rs) | `owner_id` from trusted claim — never from client input. |
| [`crates/service/src/handlers/commands/blob_move.rs`](crates/service/src/handlers/commands/blob_move.rs) | `PreparedCommand<Projected<BlobGameView>>` + fluent direct projection commit. |
| [`crates/service/src/handlers/events/project_todo.rs`](crates/service/src/handlers/events/project_todo.rs) | Eventual projector path (todos). |
| [`crates/service/src/handlers/events/project_chat.rs`](crates/service/src/handlers/events/project_chat.rs) | Eventual projector path (chat → live). |
| [`crates/service/src/handlers/ingestors/zitadel/`](crates/service/src/handlers/ingestors/zitadel/) | Ingress + scrape → `auth_users` ([runbook](docs/zitadel-ingestor.md)). |
| [`crates/runner/src/main.rs`](crates/runner/src/main.rs) | Process wiring: Postgres/SQLite, OidcBearer vs DevHeaders, GraphiQL. |

RLS sketch (from `service.rs` grants):

```text
user  → TodoView / BlobGameView rows: owner_id = claim(x-user-id)
admin → all columns / all rows on the elevated surface
```

Command result shapes:

| Domain | Result | UI effect |
|--------|--------|-----------|
| blob | `Projected<BlobGameView>` | Replica writes map/score with the mutation (no dual-write) |
| todo / chat | Causal + projector | Accept command → projector → optional `@live` push |

### Browser e2e (what CI trusts)

| Spec | Covers |
|------|--------|
| [`e2e/todos.user.spec.ts`](e2e/todos.user.spec.ts) | Create / complete / archive as alice |
| [`e2e/chat.user.spec.ts`](e2e/chat.user.spec.ts) | Post + live list |
| [`e2e/blob.user.spec.ts`](e2e/blob.user.spec.ts) | Moves, projected board, revalidation races |
| [`e2e/admin.admin.spec.ts`](e2e/admin.admin.spec.ts) | Elevated surface + force archive |
| [`e2e/unauth.anon.spec.ts`](e2e/unauth.anon.spec.ts) | Redirects when logged out |
| [`e2e/helpers/login.ts`](e2e/helpers/login.ts) | Shared OIDC + Login V2 flow |

---

## Patterns (one-liners)

| Pattern | Where |
|---------|--------|
| Multi-crate domain | `crates/*-domain`, `readmodels`, `service` |
| Projectors-only for todos/chat | handlers never dual-write those tables |
| Atomic `Projected` for blob | `blob_move.rs` / `blob_start*.rs` |
| GraphQL RLS | `service.rs` `client_grants` + model permissions |
| Live subscriptions | `@live` + ChangeHub |
| Two client surfaces | `DISTRIBUTED_CLIENT_SURFACE` / `_ADMIN_` |
| Real OIDC | Zitadel + Auth.js + custom `/login` |
| SSR without flash | root layout + `DISTRIBUTED_ROUTE_OPERATIONS` |
| Causal replica | `@hops-ops/distributed` via `file:../../../js` |

---

## Architecture sketch

```text
Browser (SSR + client)
  Auth.js → Zitadel /oauth/v2/authorize
         → UI /login?authRequest=V2_…  (Session API + CreateCallback)
         → Auth.js /auth/callback/oidc → access_token
  GraphQL HTTP  Authorization: Bearer …
  GraphQL WS    connection_init.authorization

Zitadel edge (:18080)
  Login V2 baseUri → Todos UI origin

e2e-runner
  OidcBearer (or DevHeaders offline)
  Postgres event store + bus + locks
  Projectors / Projected → ChangeHub → subs
```

**Auth flow:** Auth.js → Zitadel authorize → **your** `/login?authRequest=V2_…` →
callback. Needs `ZITADEL_SERVICE_USER_TOKEN` from `make up`. After `make up`,
restart `make run`.

---

## Typed client generation

Rust `Service` inventory is source of truth. Two pool-free exports:

| Entrypoint | Application | Roles | Used by |
|------------|-------------|-------|---------|
| `e2e_service::distributed_client_surface` | `e2e-ui` | `admin`, `user` | App shell + user routes |
| `e2e_service::distributed_admin_client_surface` | `e2e-ui-admin` | `admin` | Nested `/admin` only |

```bash
make gen-client      # both surfaces from ui/distributed.config.js
make check-client    # dctl --check without rewrite
```

Root layout:

```ts
const distributed = createDistributedSvelteKitServer({
  routes: DISTRIBUTED_ROUTE_OPERATIONS,
  getSession: ({ locals }) => locals.auth(),
  getRole: (session) => engineRoleFromGroups(session?.user?.groups)
});
export const load = distributed.load;
```

Root shell:

```ts
import { provideDistributed } from '$distributed';

const client = provideDistributed({
  session,
  hydration: data.distributed,
  authority: data.distributedAuthority
});
```

Route:

```ts
import { Todos, useCommands } from '$distributed';

const todos = Todos.use();
const commands = useCommands();
await commands.todo.create({ title });
```

**Agent rule:** after inventory / command contract / `+page.graphql` changes, run
`make check-client` and commit generated diffs. Generated trees are outputs.

---

## Tests & CI

```bash
make test          # domain + behavioral + UI unit (no Docker)
make test-live     # OIDC isolation (needs make up + API)
make test-browser  # Playwright (needs make up + make run)
```

| Project | Specs | Session |
|---------|--------|---------|
| `chromium-anon` | `e2e/*.anon.spec.ts` | none |
| `setup-alice` → `chromium-user` | `e2e/*.user.spec.ts` | alice |
| `setup-admin` → `chromium-admin` | `e2e/*.admin.spec.ts` | admin |

CI: [`.github/workflows/integration-e2e-ui.yaml`](../../.github/workflows/integration-e2e-ui.yaml)
— offline `make test`, then browser with `make up` + API/UI + Playwright.

### Identity modes (suite)

| Profile | When | Auth |
|---------|------|------|
| **DevHeaders** | `OIDC_*` unset | `x-user-id` / `x-role` (`make test`) |
| **OidcBearer** | after `source e2e-ui.env` | real Bearer (`make test-live`) |

Always-on units: `cargo test -p e2e-service --lib`, `cargo test -p todo-domain --lib`,
`cd ui && npm test`. Do not run DevHeaders behavioral against an OIDC-only process.

### WebSocket auth

Browsers cannot set `Authorization` on the upgrade. Production path: unauthenticated
upgrade, then `connection_init` with `{ "authorization": "Bearer …" }`. Chat page
uses the session access token. Do not put long-lived tokens in query strings.

---

## Env (`e2e-ui.env` from `make up`)

| Variable | Purpose |
|----------|---------|
| `DATABASE_URL` | Postgres (or `sqlite:…` offline) |
| `OIDC_ISSUER` / `OIDC_AUDIENCE` | JWKS + aud |
| `OIDC_CLIENT_ID` / `SECRET` | Auth.js |
| `ZITADEL_SERVICE_USER_TOKEN` | Login V2 Session API (server only) |
| `AUTH_SECRET` | cookie encryption |
| `E2E_MACHINE_*` | suite JWT-bearer keys |

Offline without Docker: `cargo run -p e2e-runner` (DevHeaders + SQLite); UI
`make ui-install && cd ui && npm run dev` (sign-in needs `make up` for real OIDC).

---

## Crate map

| Package | Role |
|---------|------|
| `todo-domain` / `chat-domain` / `blob-domain` | Aggregates |
| `e2e-readmodels` | `todos`, `chat_messages`, `blob_games`, `auth_users` |
| `e2e-service` | Handlers + GraphQL surface |
| `e2e-runner` → bin `e2e-ui` | Process |
| `e2e-suite` | Behavioral + gated OIDC |

## Template usage

Copy this folder: keep domains pure, swap `DATABASE_URL` / OIDC for your IdP,
extend routes. In-repo fixture uses `@hops-ops/distributed` via
`file:../../../js`; outside the monorepo, pin a released npm version.

Normative design lives in the Distributed GitKB; this README is the checked-in
fixture map.

### Security notes

- Set **`GRAPHIQL=0`** outside local dev.
- Unset `OIDC_ISSUER` / `OIDC_AUDIENCE` → **DevHeaders** (local only).
- UI `/admin` is convenience; **GraphQL field roles + handler guards** are the boundary.
