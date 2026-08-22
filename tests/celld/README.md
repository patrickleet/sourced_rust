# celld live Todo cell

First live celld host for portable command hosts: one `TodoCell` Durable
Object per todo id, SQLite private to the cell, Docker Compose for the
daemon.

This is **not** workers-rs packaging of `distributed::cell_host::AggregateCell`.
The Worker is a thin JS class with the same shard rule (`idFromName(todo_id)`).
The Rust library host stays the unit-tested adapter; this directory proves
the celld process.

## Prerequisites

- Docker
- `celld` CLI + `esbuild` on `PATH` (`curl -fsSL https://celld.dev/install.sh | sh`)
- A **qualified** bucket: S3, R2, Tigris, GCS, or Azure. Not MinIO community.

```sh
export CELLD_BUCKET=s3://your-bucket
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
# R2:
export CELLD_ENDPOINT=https://ACCOUNT.r2.cloudflarestorage.com
export CELLD_REGION=auto

celld diagnose --bucket "$CELLD_BUCKET" --endpoint "$CELLD_ENDPOINT" --region "$CELLD_REGION"
```

`celld diagnose` must report `ok bucket conditional write`.

## Run

```sh
docker compose -f tests/celld/docker-compose.yml up -d --wait
celld deploy tests/celld/worker --bucket "$CELLD_BUCKET" \
  --endpoint "$CELLD_ENDPOINT" --region "$CELLD_REGION"
CELLD_URL=http://127.0.0.1:18080 cargo test --test celld
```

Without `CELLD_URL`, `cargo test --test celld` only checks the worker
fixture and skips the live HTTP round-trip.

Tear down: `docker compose -f tests/celld/docker-compose.yml down`.
