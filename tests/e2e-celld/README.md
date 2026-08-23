# e2e-celld

New example, sibling of `tests/e2e-ui`. It is **not** `make run` in e2e-ui.

| Crate | Role |
|---|---|
| `todo-domain` / `chat-domain` / `blob-domain` | **same** domain crates as e2e-ui (path deps) |
| `e2e-celld-todo` | Todo mounts + wait-path to `TodoCell` |
| `e2e-celld-chat` | Chat mounts + wait-path to `ChatCell`; `@live` stays here |
| `e2e-celld-blob` | Blob Atomic commands (in-process) |
| `e2e-celld-identity` | Zitadel ingress/scrape + AuthUsers projector |
| `e2e-celld-graphql` | GraphQL process (`graphql_router_with_host`) |
| `tests/celld/worker` | `TodoCell` + `ChatCell` |

GraphQL mutations `todo.create` / `todo.complete` wait-dispatch to
`POST {CELLD_URL}/todo/{id}/{command}`. `chat.post` wait-dispatches to
`POST {CELLD_URL}/chat/{message_id}/chat.post`. The cell commits events and
outbox rows in one private SQLite. The GraphQL process publishes those rows
through `MessagePublisher` (NATS here; Kafka/Rabbit swap the bus). After
publish Ok it fire-and-forgets `POST …/outbox.complete` — the mutation does
not wait on that DO update. A 5s `outbox.drain` loop (and a cell alarm
POSTing `/internal/outbox/drain`) re-offers still-Pending rows. Eventual
projectors here fill SQL so `@live` still fires. Blob and identity stay
in-process. GraphQL and projectors are not cell class methods.

```sh
cd tests/e2e-ui
make up                 # Zitadel + Postgres for the Svelte login (optional)
make up-celld-nats      # Azurite + celld + NATS

cd ../e2e-celld
make run                # GraphQL :8791 + UI :5180
```

Open `http://localhost:5180`. The navbar shows a **celld** badge. Sign in
(`alice` / `Password1!` when Zitadel is up). Todos create/complete and
lobby posts go to cells; open Chat in two tabs to see `@live` still fire.

Override a busy celld port: `CELLD_HTTP_PORT=18880 make run`.
