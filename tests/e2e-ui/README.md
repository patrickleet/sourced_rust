# Fieldnote — e2e-ui template

A **copyable starting point** for a Distributed service + SvelteKit UI with:

| Pattern | Where |
|---------|--------|
| Multi-crate domain (todos + chat) | `crates/*-domain`, `readmodels`, `service` |
| Projectors-only read models | command handlers never dual-write |
| GraphQL RLS | `owner_id = claim(x-user-id)` on todos |
| Live subscriptions | `subscription { chat_messages }` + ChangeHub |
| **WebSocket auth** | Bearer access token in `connection_init` (OIDC best practice) |
| **Real OIDC** | Zitadel in Docker + Auth.js |
| **Postgres** | event store + bus + locks |
| **SSR GraphQL** | `+page.server.ts` loads with session token (no Loading flash) |
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
| http://127.0.0.1:5180/chat | SSR seed + live WS sub |
| http://127.0.0.1:5180/session | Session / token inspector |
| http://127.0.0.1:8791/graphql | GraphiQL |
| `ws://127.0.0.1:8791/graphql/ws` | Subscriptions |

**Demo logins** (Zitadel): `alice` / `bob` / `admin` — password `Password1!`

## Offline (no Docker)

```bash
# DevHeaders + SQLite memory/file
cargo run -p e2e-runner
# UI without OIDC still builds; sign-in needs make up for real OIDC
cd ui && npm install && npm run dev
```

```bash
make test        # domain + behavioral + structural UI (no Docker)
make test-live   # OIDC isolation (E2E_STACK=1, needs make up + API up)
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
  Auth.js → Zitadel OIDC  → access_token
  GraphQL HTTP  Authorization: Bearer …
  GraphQL WS    connection_init.authorization

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
| `AUTH_SECRET` | Auth.js cookie encryption |
| `E2E_MACHINE_*` | Suite JWT-bearer keys |

## Crate map

| Package | Role |
|---------|------|
| `todo-domain` / `chat-domain` | Aggregates |
| `e2e-readmodels` | `todos`, `chat_messages` |
| `e2e-service` | Handlers + GraphQL |
| `e2e-runner` → bin `e2e-ui` | Process |
| `e2e-suite` | Behavioral + gated OIDC |

## Template usage

Copy this folder as a starting service: keep domain pure, swap `DATABASE_URL` / OIDC env for your IdP, and extend UI routes. Remove Fieldnote branding as needed — patterns stay.
