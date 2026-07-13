# e2e-ui layout

## Crates

1. `todo-domain` / `chat-domain` — aggregates, domain errors, outbox facts. No HTTP/SQL.
2. `readmodels` — `TodoView` + `ChatMessageView` + `distributed_manifest()`.
3. `service` — command handlers, event projectors, GraphQL engine (+ change stream).
4. `runner` — `DATABASE_URL`, bus, outbox worker, bind address.

## Rules

- **Owner from session** for todos; chat **author from session**.
- **Projectors only** write read models — commands commit aggregate + outbox.
- **GraphQL RLS** on todos: `owner_id = claim(x-user-id)` for role `user`.
- **Subscriptions**: `repo.read_model_changes()` → `GraphqlEngineBuilder::change_stream`;
  clients connect to WebSocket `/graphql/ws`.

## UI

SvelteKit SPA: todos (query + optimistic poll) and chat (live subscription).
