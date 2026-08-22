# celld live Todo cell

First live celld host for portable command hosts: one `TodoCell` Durable
Object per todo id, private SQLite, Docker Compose for the daemon **and**
Azurite (no AWS or Cloudflare account).

The Worker is a workers-rs Durable Object class around
`distributed::cell_host::AggregateCell<Todo>`. Shard rule is still
`idFromName(todo_id)` (`PCH-DEC-004`). GraphQL and projectors are not
cell methods. Cell stream storage is still the in-memory
`CellStreamStore` stand-in (SQL-backed cell storage is follow-up).

Azurite is celld's documented local development store. It is **not** a
production fleet bucket.

## Prerequisites

- Docker
- `celld` CLI + `esbuild` on `PATH` (`curl -fsSL https://celld.dev/install.sh | sh`)
- `worker-build` (`cargo install worker-build`) and the `wasm32-unknown-unknown` target

## Run

```sh
docker compose -f tests/celld/docker-compose.yml up -d --build azurite
# wait until azurite-init exits 0

export AZURE_STORAGE_USE_EMULATOR=true
export AZURE_STORAGE_ACCOUNT_NAME=devstoreaccount1
export AZURE_STORAGE_ACCOUNT_KEY='Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw=='

celld diagnose --bucket az://celld --listen 127.0.0.1:18090 --internal-listen 127.0.0.1:18091
(cd tests/celld/worker && worker-build --release)
celld deploy tests/celld/worker --bucket az://celld
docker compose -f tests/celld/docker-compose.yml up -d celld
CELLD_URL=http://127.0.0.1:18080 cargo test --test celld
```

Nodes load a deployment at startup, so deploy before the celld container starts (or restart it after deploy). `celld diagnose` should report `ok bucket conditional write`. A host-side peer probe to `:8081` is expected to fail: that listener is not published.

If host port 18080 is already taken, set `CELLD_HTTP_PORT` (for example `18880`)
before `docker compose up` and use that port in `CELLD_URL`. If host port 8080 is taken, pass `--listen` / `--internal-listen` to `celld diagnose` as above.

Without `CELLD_URL`, `cargo test --test celld` only checks the worker
fixture and skips the live HTTP round-trip.

Tear down: `docker compose -f tests/celld/docker-compose.yml down -v`.

## Ports

| Host | Inside compose | What |
|---|---|---|
| 18080 (or `CELLD_HTTP_PORT`) | celld `:8080` | Worker HTTP |
| 10000 | Azurite blob | Host `celld deploy` / `diagnose` |
| — | celld `:8081` | Peer/internal — not published |

celld's Azure emulator client always uses `127.0.0.1:10000`. The celld
container forwards that address to the `azurite` service. Sharing Azurite's
network namespace is not used: Docker Desktop injects `extra_hosts`, which
conflicts with `network_mode: service:…`.
