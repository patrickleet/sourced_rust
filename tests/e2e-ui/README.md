# Fieldnote — e2e-ui template

A **copyable starting point** for a Distributed service + SvelteKit UI with:

| Pattern | Where |
|---------|--------|
| Multi-crate domain (todos + chat + blob) | `crates/*-domain`, `readmodels`, `service` |
| Projectors-only read models | command handlers never dual-write |
| GraphQL RLS | `owner_id = claim(x-user-id)` on todos / blob |
| Live subscriptions | `query ChatMessages @load @live` + ChangeHub |
| **Zitadel ingestor** | `POST /zitadel.ingress.v1` + scrape reconcile → `auth_users` ([local runbook](docs/zitadel-ingestor.md); design: `specs/e2e-ui/zitadel-ingestor`) |
| **GraphQL joins** | `chat_messages.author` / `blob_games.owner` → `auth_users` |
| **WebSocket auth** | Bearer access token in `connection_init` (OIDC best practice) |
| **Real OIDC** | Zitadel in Docker + Auth.js (PKCE + session cookie) |
| **Login V2** | **Custom** `/login` + `/signup` in this UI (Session API + CreateCallback); not Zitadel’s stock login image |
| **Postgres** | event store + bus + locks |
| **SSR GraphQL** | A generated static route registry drives root-layout loading and hydration (no Loading flash) |
| **Published JS client** | Local [`js/`](../../js/) package supplies typed transport, normalized causal replica, generated commands, and SvelteKit adapter |
| Auth routes | sign-in, protected todos/chat, session inspector |

## Quick start (full stack)

```bash
cd tests/e2e-ui
make up          # Postgres :5433 + Zitadel :18080 + bootstrap → e2e-ui.env
set -a && source e2e-ui.env && set +a
make run         # API :8791 + UI :5180
```

| URL | What |
|-----|------|
| http://127.0.0.1:5180 | Fieldnote UI |
| http://127.0.0.1:5180/todos | SSR todos (auth required) |
| http://127.0.0.1:5180/blob | Blob game — aggregate moves → projected `blob_games` map/score |
| http://127.0.0.1:5180/admin | Admin all-owners todos (admin role only) |
| http://127.0.0.1:5180/chat | SSR load + live WS continuation |
| http://127.0.0.1:5180/session | Session / token inspector |
| http://127.0.0.1:8791/graphql | GraphiQL |
| `ws://127.0.0.1:8791/graphql/ws` | Subscriptions |
| http://127.0.0.1:5180/login | Custom Login V2 (password form on this app) |
| http://127.0.0.1:5180/signup | Custom registration |
| http://localhost:18080/ui/console | Zitadel console |

**Auth flow:** Auth.js → Zitadel `/oauth/v2/authorize` → **your** `/login?authRequest=V2_…` (Session API + CreateCallback) → Auth.js `/auth/callback/oidc`. Requires `ZITADEL_SERVICE_USER_TOKEN` from `make up`. After `make up`, always **restart** `make run`.

**Demo logins** (Zitadel): `alice` / `bob` / `admin` — password `Password1!`

| Login | Engine role | Notes |
|-------|-------------|--------|
| `alice` / `bob` | `user` | Personal `/todos` (owner filter) |
| `admin` | `admin` | `/admin` — all owners' notes + **`todos_force_archive`** through a separate elevated client surface |

## Offline (no Docker)

```bash
# DevHeaders + SQLite memory/file
cargo run -p e2e-runner
# UI without OIDC still builds; sign-in needs make up for real OIDC
make ui-install
cd ui && npm run dev
```

```bash
make test          # domain + behavioral + structural UI (no Docker)
make test-live     # OIDC isolation (E2E_STACK=1, needs make up + API up)
make test-browser  # Playwright against live UI (needs make up + make run)
```

### Browser e2e (Playwright)

Real Chromium flows against the Fieldnote UI — login (OIDC + custom Login V2), todos, chat, blob, admin, unauth redirects.

```bash
make up && make run          # leave running
# other terminal:
cd tests/e2e-ui
make test-browser            # npm install + chromium + playwright test
```

| Project | Specs | Session |
|---------|--------|---------|
| `chromium-anon` | `e2e/*.anon.spec.ts` | none |
| `setup-alice` → `chromium-user` | `e2e/*.user.spec.ts` | alice |
| `setup-admin` → `chromium-admin` | `e2e/*.admin.spec.ts` | admin |

Demo passwords: `alice` / `bob` / `admin` · `Password1!` (from bootstrap).

### CI

`on-pr-quality` and `on-push-main` call [`.github/workflows/integration-e2e-ui.yaml`](../../.github/workflows/integration-e2e-ui.yaml):

1. **offline** — `make test` (domain + behavioral suite + UI unit/build)
2. **browser** — `make up` → build API → run API+UI → Playwright Chromium

Artifacts on browser failure: `e2e-ui-playwright-report` (HTML report + traces).

### Suite identity modes (E6)

| Profile | When | How suite/API auth works |
|---------|------|---------------------------|
| **DevHeaders** | `OIDC_ISSUER` / `OIDC_AUDIENCE` **unset** | Suite sends `x-user-id` / `x-role`; `make test` behavioral expects this |
| **OidcBearer** | After `make up` + `source e2e-ui.env` | API rejects ambient headers; need real Bearer tokens (`make test-live` / machine keys) |

Always-on units (no stack): `cargo test -p e2e-service --lib` (strip spoof), `cargo test -p todo-domain --lib` (owner gates), `cd ui && npm test` (includes systems-harden unit pack).

Do **not** run DevHeaders behavioral against an OIDC-only process and treat 401 as product failure — wrong profile.

### UI unit tests

```bash
cd ui && npm test
# includes systems-harden-unit.mjs (C-U* red-team + pending merge)
```

## WebSocket authentication (best practice)

Browsers **cannot** set `Authorization` on the WebSocket upgrade handshake.

| Mode | How identity is established |
|------|-----------------------------|
| **OidcBearer (production path)** | Upgrade is unauthenticated; client sends `connection_init` with `{ "authorization": "Bearer <access_token>" }` (or `accessToken` / `headers.Authorization`). Server validates JWT (same as HTTP). |
| **DevHeaders (local)** | Upgrade headers, `?x-user-id=`, or GraphiQL `wsConnectionParams`. |

The SvelteKit chat page uses the session access token in `connection_init`. Do **not** put long-lived tokens in URL query strings for production.

## Architecture sketch

```text
Browser (SSR + client)
  Auth.js → Zitadel /oauth/v2/authorize
         → UI /login?authRequest=V2_…  (Session API + CreateCallback)
         → Auth.js /auth/callback/oidc → access_token
  GraphQL HTTP  Authorization: Bearer …
  GraphQL WS    connection_init.authorization

Zitadel edge (:18080)
  /oauth/*, /management/*, /v2/*, /ui/console → zitadel
  Login V2 baseUri → Fieldnote UI origin (custom pages)

e2e-runner (Distributed)
  OidcBearer identity
  PostgresRepository + PostgresBus + PostgresLockManager
  (or SQLite when DATABASE_URL=sqlite:…)
  Projectors → ChangeHub → subscription pushes
```

## Env (`e2e-ui.env` from `make up`)

| Variable | Purpose |
|----------|---------|
| `DATABASE_URL` | `postgres://e2e:e2e@127.0.0.1:5433/e2e_ui` |
| `OIDC_ISSUER` | Zitadel base URL |
| `OIDC_AUDIENCE` | Project id (JWT aud) |
| `OIDC_CLIENT_ID` / `SECRET` | Auth.js web app |
| `ZITADEL_SERVICE_USER_TOKEN` | IAM_LOGIN_CLIENT PAT for Session API + CreateCallback (server only) |
| `AUTH_SECRET` | Auth.js cookie encryption |
| `E2E_MACHINE_*` | Suite JWT-bearer keys |

## Crate map

| Package | Role |
|---------|------|
| `todo-domain` / `chat-domain` / `blob-domain` | Aggregates |
| `e2e-readmodels` | `todos`, `chat_messages`, `blob_games`, `auth_users`, and relationships |
| `e2e-service` | Handlers + GraphQL |
| `e2e-runner` → bin `e2e-ui` | Process |
| `e2e-suite` | Behavioral + gated OIDC |

## Template usage

Copy this folder as a starting service: keep domain pure, swap `DATABASE_URL` /
OIDC env for your IdP, and extend UI routes. Remove Fieldnote branding as needed
— patterns stay. The in-repository fixture intentionally depends on
`@hops-ops/distributed` through `file:../../../js` so it tests the local package;
after copying it outside this repository, replace that dependency with a
released npm version.

## Design docs (GitKB)

Normative architecture, API decisions, and implementation plans live in the
**Distributed GitKB**, not in this code fixture. This README documents how to
run and extend the checked-in example; update the Distributed KB when the
design itself changes.

### One typed client generation path

The Rust `Service` inventory is the source of truth for reads, commands,
permissions, result contracts, and optimistic effects. The fixture exposes two
pool-free client surfaces:

| Entrypoint | Application | Roles | Used by |
|------------|-------------|-------|---------|
| `e2e_service::distributed_client_surface` | `fieldnote` | `admin`, `user` | The normal app shell and user routes |
| `e2e_service::distributed_admin_client_surface` | `fieldnote-admin` | `admin` | The nested `/admin` shell only |

`dctl client-manifest` evaluates that inventory without starting Postgres.
`dctl client` combines the manifest with co-located route documents and emits
the typed operations, command tree, optimistic metadata, and static route
registry used by both SSR and the browser.

```bash
# from tests/e2e-ui
make gen-client      # generate both surfaces from ui/distributed.config.js
make check-client    # dctl --check both surfaces without rewriting
```

Route reads live beside their pages as `+page.graphql`:

```graphql
query Todos @load {
  todos(order_by: [{ status: asc }, { todo_id: asc }]) {
    todo_id
    owner_id
    title
    status
  }
}
```

`@load` adds the operation to the generated static route registry for SSR.
`@live` continues the same operation over WebSocket after hydration. Commands
come from the typed Rust inventory, so route documents contain reads rather
than hand-authored command mutations.

The compiler-owned `$distributed` target exports `Todos`,
`useCommands`, `provideDistributed`, and `DISTRIBUTED_ROUTE_OPERATIONS`.
The root server layout loads declared routes once:

```ts
const distributed = createDistributedSvelteKitServer({
  routes: DISTRIBUTED_ROUTE_OPERATIONS,
  getSession: ({ locals }) => locals.auth(),
  getRole: (session) => engineRoleFromGroups(session?.user?.groups)
});

export const load = distributed.load;
```

The root component tree provides one client with the same session source for
GraphQL HTTP, WebSocket connection init, commands, and authorization-scope
invalidation:

```ts
import { provideDistributed } from '$distributed';

const client = provideDistributed({
  session,
  hydration: data.distributed,
  authority: data.distributedAuthority
});
```

Routes resolve that nearest client only when the component initializes:

```ts
import { Todos, useCommands } from '$distributed';

const todos = Todos.use();
const commands = useCommands();
await commands.todo.create({ title }); // todo_id defaults to uuid_v7()
```

The nested `/admin` layout creates a second client from the
`fieldnote-admin` artifacts after its role gate. User pages cannot import or
discover elevated operations through the normal `fieldnote` client.

**Agent rule:** after changing the typed `Service` inventory, a command
contract, or a co-located `+page.graphql`, run `make check-client` and commit
the generated diffs. Generated artifacts are outputs, not authoring surfaces.

### Security template notes

- Set **`GRAPHIQL=0`** when not developing — GraphiQL is on by default for the fixture only.
- If `OIDC_ISSUER` / `OIDC_AUDIENCE` are unset, the API falls back to **DevHeaders** (local only). Never expose that mode on a public edge.
- UI `/admin` is a convenience gate; **GraphQL field roles + handler guards** are the security boundary.
