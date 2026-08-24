# Optional celld + NATS profile (not the default playground)

This directory is an **optional** split-process profile of the same e2e-ui
Svelte app. It is **not** `make run` and **not** a replacement for the
one-process playground (`DCS-DEC-001`, `ESM-REQ-009`).

Default remains:

```sh
cd tests/e2e-ui
make up    # Postgres + Zitadel
make run   # one backend + UI
```

Optional profile:

```sh
cd tests/e2e-ui
make up-celld-nats     # Azurite + celld + NATS (not make run)
make test-celld        # live --test celld + GraphQL wait-path smoke (CI)
make test-celld-nats   # GraphQL wait-path smoke + SQL list only
make down-celld-nats   # NATS only
make down-celld        # Azurite + celld

cd ../e2e-celld
make run               # new GraphQL service crates + the Svelte UI
```

`tests/e2e-ui/crates/service/src/host.rs` stays a single backend process.
The playground UI against celld is the sibling example `tests/e2e-celld/`
(new service crates; same domain crates). Do not add that topology here.

## What this profile is

| Path | Where |
|---|---|
| GraphQL wait-path mutations | `CelldCommandHost` → celld `POST /todo/{id}/todo.create` (`{ commandId, input }` + internal identity headers) |
| Fire-and-forget / events | NATS JetStream `publish` / `subscribe` |
| Todo / Chat lists | SQL read models (projectors subscribe on NATS, **not** in cells) |
| BlobGames by-id | `ReadStore::CellByKey` GET of the sealed row |
| `@live` / SQL joins on Blob | rejected by the cell-by-key compiler (`DCS-5`) |

GraphQL, `@live`, and Eventual projectors are **not** cell class methods
(`DCS-AC-008.1`, `PCH-REQ-005`).

## Bring-up (local only)

Azurite + celld already live under `tests/celld/docker-compose.yml`. NATS
is extra and named so it cannot be confused with `tests/e2e-ui/docker`.

```sh
cd tests/e2e-ui
make up-celld-nats
make test-celld-nats
```

Override ports if busy: `CELLD_HTTP_PORT=18880 NATS_PORT=14223 make up-celld-nats`.
If `14222` is already taken by a leftover `docker run` NATS, `make down-celld-nats` removes that container too.

Manual equivalent (same as the Make recipes):

```sh
# 1) celld + Azurite (no MinIO)
docker compose -f tests/celld/docker-compose.yml up -d --build azurite
# deploy worker, then:
docker compose -f tests/celld/docker-compose.yml up -d celld

# 2) NATS for this optional profile only
docker compose -f tests/e2e-ui/celld-nats-profile/docker-compose.yml up -d

export CELLD_URL=http://127.0.0.1:${CELLD_HTTP_PORT:-18080}
export NATS_URL=nats://127.0.0.1:${NATS_PORT:-14222}

cargo test --test e2e_ui_celld_nats_profile --features graphql,http,sqlite
```

Without `CELLD_URL` **and** `NATS_URL`, the test still checks that the
default host is one-process and this profile is documented; live smoke
is skipped (`PCH-AC-006.1`).

Tear down NATS only: `docker compose -f tests/e2e-ui/celld-nats-profile/docker-compose.yml down`.
Do not use that as `make down` for the playground.

## Identity

Reuse e2e-ui OIDC / DevHeaders. No new secret files. Azurite uses the
public emulator account already documented in `tests/celld`.
