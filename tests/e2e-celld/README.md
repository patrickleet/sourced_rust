# e2e-celld

New example, sibling of `tests/e2e-ui`. It is **not** `make run` in e2e-ui.

| Crate | Role |
|---|---|
| `todo-domain` / `chat-domain` / `blob-domain` | **same** domain crates as e2e-ui (path deps) |
| `e2e-celld-todo` | Todo mounts + `CelldRoute` (kind/shard/payload) for `TodoCell` |
| `e2e-celld-chat` | Chat mounts + `CelldRoute` for `ChatCell`; `@live` stays here |
| `e2e-celld-blob` | Blob Atomic commands (in-process) |
| `e2e-celld-identity` | Zitadel ingress/scrape + AuthUsers projector |
| `e2e-celld-graphql` | GraphQL process (`graphql_router_with_host`) |
| `tests/celld/worker` | `TodoCell` + `ChatCell` |

`distributed::cell_host::CelldCommandHost` wait-dispatches `todo.create` /
`todo.complete` / `chat.post` to `{CELLD_URL}/{kind}/{shard}/{command}`.
Aggregate crates only supply a `CelldRoute` (kind, shard id, payload map).
The cell commits events and outbox in one private SQLite. A single bounded host
scheduler claims leased rows, publishes them through `MessagePublisher` (NATS
here; Kafka/Rabbit swap the bus), and settles only rows owned by that worker.
Mutation completion only queues the durable cell address; it never waits for
the broker. Cell alarms POST the same address-only hint to
`/internal/outbox/drain`. Eventual projectors here fill SQL so
`@live` still fires. Blob and identity stay in-process. GraphQL is the user edge (`OidcBearer`
on the engine); Zitadel Actions and outbox drain are internal HTTP on the
same process. GraphQL and projectors are not cell class methods.

## Private HTTP boundary

All cell reads, commands, outbox claim/settle operations, cell alarms, and
Zitadel internal routes require `DISTRIBUTED_INTERNAL_SECRET` in the
`x-distributed-internal-secret` header. Startup fails when the secret is
missing or invalid. Local compose ports and the GraphQL listener bind to
loopback by default. The checked-in value in the worker config is test-only;
real deployments must install a unique secret binding and still use TLS and
network policy. Health endpoints remain public and disclose no secret.

The threat model assumes the GraphQL edge has already verified user identity.
The internal secret prevents a network caller from forging trusted user/role,
service, partition, alarm, or outbox-settlement headers. Strict envelopes,
body limits, URL encoding, disabled redirects, timeouts, leases, and ownership
tokens bound the damage from malformed, replayed, slow, or concurrent requests.

Workspace tests (no live celld):

```sh
make test               # cargo test --workspace (CI)
```

```sh
cd tests/e2e-ui
make up                 # Zitadel + Postgres (read models + login)
make up-celld-nats      # Azurite + celld + NATS

cd ../e2e-celld
make run                # GraphQL :8791 + UI :5180 (watches sources)
```

Eventual projectors and `@live` use **Postgres** from `e2e-ui.env`
(`DATABASE_URL`). There is no SQLite read-model path. Cells still keep
private SQLite per Durable Object. Override with `E2E_CELLD_DATABASE_URL`.

`make run` reloads on its own:

| Surface | How |
|---|---|
| Svelte UI | Vite HMR (`npm run dev`) |
| GraphQL host | `cargo-watch` on `src/`, e2e-celld crates, and domain crates |
| Cell worker | `cargo-watch` → `worker-build --dev` + `celld deploy` + compose restart |

`WATCH=0` / `WATCH_WORKER=0` turn those cargo-watch loops off. Compose
`CELLD_WATCH` is the node's SQLite working directory, not a source watcher
— celld loads a deployment at startup, so worker changes need a restart
(the watch target does that). First `cargo-watch` install: `cargo install cargo-watch`.

Open `http://localhost:5180`. The navbar shows a **celld** badge. Sign in
(`alice` / `Password1!` when Zitadel is up). Todos create/complete and
lobby posts go to cells; open Chat in two tabs to see `@live` still fire.

Override a busy celld port: `CELLD_HTTP_PORT=18880 make run`.
