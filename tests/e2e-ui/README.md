# Fieldnote — e2e-ui template

A **copyable starting point** for a Distributed service + SvelteKit UI with:

| Pattern | Where |
|---------|--------|
| Multi-crate domain (todos + chat + blob) | `crates/*-domain`, `readmodels`, `service` |
| Projectors-only read models | command handlers never dual-write |
| GraphQL RLS | `owner_id = claim(x-user-id)` on todos / blob |
| Live subscriptions | `subscription { chat_messages }` + ChangeHub |
| **Zitadel ingestor** | `POST /zitadel.ingress.v1` + scrape reconcile → `auth_users` ([local runbook](docs/zitadel-ingestor.md); design: `specs/e2e-ui/zitadel-ingestor`) |
| **GraphQL joins** | `chat_messages.author` / `blob_games.owner` → `auth_users` |
| **WebSocket auth** | Bearer access token in `connection_init` (OIDC best practice) |
| **Real OIDC** | Zitadel in Docker + Auth.js (PKCE + session cookie) |
| **Login V2** | **Custom** `/login` + `/signup` in this UI (Session API + CreateCallback); not Zitadel’s stock login image |
| **Postgres** | event store + bus + locks |
| **SSR GraphQL** | `+page.server.ts` loads with session token (no Loading flash) |
| **Published JS client** | Local [`js/`](../../js/) package supplies typed transport, cache, command runtime, and SvelteKit adapter |
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
| http://127.0.0.1:5180/chat | SSR seed + live WS sub |
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
| `admin` | `admin` | `/admin` — all owners' notes + **`todos_force_archive`** (admin-only mutation; absent from user SDL) |

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

Normative design lives in the Distributed GitKB attached to this repository:

| Spec | Content |
|------|---------|
| `specs/e2e-ui/layout` | Crate map, projector/RLS rules, UI surface |
| `specs/e2e-ui/blob-game` | Blob Game aggregate, facts, projection, GraphQL, and UI contract |
| `specs/e2e-ui/zitadel-ingestor` | Provider ingress, reconciliation, directory projection, and joins |
| `specs/e2e-ui/sveltekit-dx` | SvelteKit GraphQL DX (unified client, defineResource) |
| `specs/e2e-ui/gql-codegen-dx` | Co-located `.gql` + graphql-codegen |
| `specs/e2e-ui/rust-role-sdl-codegen` | Role SDL from Rust engine → UI schema |

### UI GraphQL schema + codegen

Role SDL is **generated** from the same GraphQL engine the API runs
(`build_graphql_engine` + `sdl_for_role`):

| File | Role | Notes |
|------|------|--------|
| `ui/schema/user.graphql` | `user` | No `todos_force_archive` |
| `ui/schema/admin.graphql` | `admin` | Superset — includes admin-only mutations |

Codegen uses **admin** schema so co-located admin ops typecheck; **runtime ACL is the session engine role** — a user token cannot call admin-only fields even if types exist in the bundle.

```bash
# from tests/e2e-ui
make export-sdl          # user + admin SDL from GraphQL engine
make gen-gql             # export-sdl + TypedDocumentNode from co-located *.gql
make check-gql           # gen-gql + git diff --exit-code (CI / agents)

# or from ui/
npm run gen:schema
npm run gen:gql
npm run gen
```

Edit co-located `routes/**/*.gql`, run `make gen-gql`, commit schema + `*.generated.ts`.

**Agent rule:** after changing `build_graphql_engine` / `graphql_commands()`, command handlers, or `*.gql`, run `make check-gql` **and** `make check-commands`, then commit any schema/generated diffs. Do not hand-edit `schema/*.graphql`, `*.generated.ts`, or `commands.generated.ts` as source of truth.

### Command client (typed functions over GraphQL wire)

Day-to-day writes are **commands**, not hand-authored mutation documents.
The same Rust registry (`e2e_service::graphql_commands()`) exports a catalog;
`make gen-commands` invokes the `distributed-gen-commands` CLI supplied by
`@hops-ops/distributed` and emits:

| Artifact | Purpose |
|----------|---------|
| `ui/src/lib/api/commands.manifest.json` | Machine catalog from Rust |
| `ui/src/lib/api/commands.operations.gql` | Copy-paste mutations for GraphiQL |
| `ui/src/lib/api/commands.generated.ts` | Same documents + `bindCommands` → `gql.commands.*` |
| `ui/src/lib/api/commands.policies.generated.ts` | Typed result/reconciliation defaults from the service manifest |

Co-located route `*.gql` files hold **queries/subscriptions** only. Command
mutations live under `$lib/api/commands.operations.gql`.

```bash
make export-commands   # → commands.manifest.json
make gen-commands      # → .operations.gql + .generated.ts
make check-commands    # fail on drift
```

Example (commands pre-bound on the client):

```ts
const gql = useGraphql(() => data);
await gql.commands.todosCreate({ todo_id, title });
await gql.commands.chatMessagesPost({ message_id, body, room_id });
gql.subscribe(chat.subscription!, { onNext });
```

See distributed GitKB: `specs/query-layer/references/command-client-dx` and
epic `tasks/graphql-qs-command-client-1`.

### Security template notes

- Set **`GRAPHIQL=0`** when not developing — GraphiQL is on by default for the fixture only.
- If `OIDC_ISSUER` / `OIDC_AUDIENCE` are unset, the API falls back to **DevHeaders** (local only). Never expose that mode on a public edge.
- UI `/admin` is a convenience gate; **GraphQL field roles + handler guards** are the security boundary.
