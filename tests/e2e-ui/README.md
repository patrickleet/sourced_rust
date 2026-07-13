# e2e-ui fixture

Multi-crate Distributed service + SvelteKit UI for end-to-end demos.

| Area | What |
|------|------|
| **Todos** | Per-user todos; GraphQL filter `owner_id = claim(x-user-id)` |
| **Chat** | Shared lobby; **GraphQL subscriptions** push after projectors |

## Run the full app

```bash
cd tests/e2e-ui
make
```

| | URL |
|--|-----|
| **UI** | http://127.0.0.1:5180 |
| **Todos** | http://127.0.0.1:5180/ |
| **Chat** | http://127.0.0.1:5180/chat |
| **API** | http://127.0.0.1:8791 |
| **GraphiQL** | http://127.0.0.1:8791/graphql |
| **Subscriptions WS** | `ws://127.0.0.1:8791/graphql/ws` |

Ctrl-C stops API + UI. `make test` runs domain/suite/UI contracts.

## Crate map

| Path | Package | Role |
|------|---------|------|
| `crates/todo-domain` | `todo-domain` | Todo aggregate |
| `crates/chat-domain` | `chat-domain` | ChatMessage aggregate |
| `crates/readmodels` | `e2e-readmodels` | `todos` + `chat_messages` |
| `crates/service` | `e2e-service` | Commands + projectors + GraphQL |
| `crates/runner` | `e2e-runner` → bin `e2e-ui` | Process |
| `crates/suite` | `e2e-suite` | Behavioral T1–T6 |
| `ui/` | SvelteKit | Todos + chat subscription page |

## Chat + subscriptions

1. Command `chat.post` commits aggregate + outbox (`chat_message.posted`).
2. Projector upserts `chat_messages` (not dual-write from the command).
3. Repo broadcasts `ReadModelChange` → GraphQL `ChangeHub`.
4. Active `subscription { chat_messages(...) { ... } }` re-queries and pushes.

Browser client: `ui/src/lib/graphql-ws.ts` (graphql-transport-ws) → `/graphql/ws`
with `?x-user-id=&x-role=` (browsers cannot set custom WS headers).

## Commands

| Command | Effect |
|---------|--------|
| `todo.create` / `rename` / `complete` / `reopen` / `archive` | Personal todos |
| `chat.post` | Post to a room (default `lobby`); author = session user |
