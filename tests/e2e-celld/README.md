# e2e-celld

New example, sibling of `tests/e2e-ui`. It is **not** `make run` in e2e-ui.

| Crate | Role |
|---|---|
| `todo-domain` / `chat-domain` / `blob-domain` | **same** domain crates as e2e-ui (path deps) |
| `e2e-celld-todo` | Todo mounts + `HttpCommandHost` to celld |
| `e2e-celld-chat` | Chat lobby (in-process) |
| `e2e-celld-blob` | Blob Atomic commands (in-process) |
| `e2e-celld-identity` | Zitadel ingress/scrape + AuthUsers projector |
| `e2e-celld-graphql` | GraphQL process (`graphql_router_with_host`) |
| `tests/celld/worker` | Todo cell (already existed) |

GraphQL mutations `todo.create` / `todo.complete` wait-dispatch to
`POST {CELLD_URL}/todo/{id}/{command}`. SQL lists fill by dual-writing the
local Todo service after the cell wait-path succeeds. Chat, Blob, and
identity stay in-process. GraphQL and projectors are not cell class methods.

```sh
cd tests/e2e-ui
make up                 # Zitadel + Postgres for the Svelte login (optional)
make up-celld-nats      # Azurite + celld + NATS

cd ../e2e-celld
make run                # GraphQL :8791 + UI :5180
```

Open `http://localhost:5180`. The navbar shows a **celld** badge. Sign in
(`alice` / `Password1!` when Zitadel is up) and use Todos — create/complete
go to celld.

Override a busy celld port: `CELLD_HTTP_PORT=18880 make run`.
