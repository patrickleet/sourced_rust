# Zitadel Action → e2e-ui ingress

Teaching fixture for **provider ingress → outbox → projector → read model → GraphQL joins**.

Simplified cousin of the gitkb domain-service Zitadel ingestor: here we **do not** drive an
`AuthAccount` aggregate. The projector maps `zitadel.*.v1` messages straight into
`auth_users`, which chat and blob games join for display names.

## Pipeline

```text
                    ┌─ Zitadel Action / curl ── POST /zitadel.ingress.v1
                    │
Provider messages ──┼─ Management API scrape ── background loop + POST /zitadel.scrape.v1
                    │     (missed events / backfill)
                    ▼
              outbox  zitadel.user.*.v1
                    ▼
              project_auth_user  →  auth_users
                    ▼
              GraphQL joins: chat.author / blob.owner
```

| Layer | Location |
|-------|----------|
| Authenticity | `handlers/ingestors/zitadel/auth.rs` |
| Map Action → subject | `handlers/ingestors/zitadel/map.rs` |
| Outbox publish | `handlers/ingestors/zitadel/publish.rs` |
| Management scrape | `handlers/ingestors/zitadel/scrape.rs` |
| On-demand scrape cmd | `handlers/ingestors/zitadel_scrape.rs` (`zitadel.scrape.v1`) |
| Projector | `handlers/events/project_auth_user.rs` |
| Read model + joins | `e2e-readmodels` `AuthUserView`, `ChatMessageView.author`, `BlobGameView.owner` |

## Scrape (reconcile)

Actions can drop events. The scraper lists users from Zitadel Management API and
publishes the **same** provider subjects the Action path uses (`zitadel.user.*.v1`),
so projectors stay single-path.

| Env | Purpose |
|-----|---------|
| `ZITADEL_SERVICE_USER_TOKEN` | PAT (same as Login V2; written by `make up`) |
| `ZITADEL_API_URL` or `OIDC_ISSUER` | API base (e.g. `http://localhost:18080`) |
| `ZITADEL_SCRAPE_INTERVAL_SECS` | Background period (default **60**; `0` = no loop) |
| `ZITADEL_SCRAPE_ON_START` | Run once at process start (default **on** when configured) |

Delivery ids are `zitadel-scrape:{userId}:{fingerprint}` so unchanged profiles
do not spam the outbox on every tick.

```bash
# On-demand (secret header)
curl -sS -X POST "http://127.0.0.1:8791/zitadel.scrape.v1" \
  -H "content-type: application/json" \
  -H "x-zitadel-ingestor-secret: $ZITADEL_INGESTOR_SECRET" \
  -d '{}'
# → { listed, published, skipped, errors }
```

## Authenticity

| Item | Value |
|------|--------|
| Env | `ZITADEL_INGESTOR_SECRET` (required for fixture path) |
| Header | `x-zitadel-ingestor-secret: <secret>` |
| Alt | `Authorization: Bearer <secret>` |
| Local Actions | `ZITADEL_INGESTOR_ALLOW_ACTION_EVENTS=1` accepts native Actions v2 bodies without secret |
| OIDC | Path `/zitadel.ingress.v1` skips the bearer gate (secret is the authn) |

Missing or invalid secret → **401**.

## curl fixtures

```bash
SECRET=dev-secret-change-me
BASE=http://127.0.0.1:8791
export ZITADEL_INGESTOR_SECRET="$SECRET"

# Reject without secret
curl -sS -o /dev/null -w "%{http_code}\n" -X POST "$BASE/zitadel.ingress.v1" \
  -H 'content-type: application/json' \
  -d '{"event_type":"user.human.created","provider_subject":"u1","delivery_id":"d0"}'
# → 401

# Import a human (provider_subject = OIDC sub = E2E_HUMAN_ALICE_UID from e2e-ui.env)
set -a && source e2e-ui.env && set +a
SECRET="${ZITADEL_INGESTOR_SECRET:-e2e-zitadel-ingestor-secret}"
curl -sS -X POST "$BASE/zitadel.ingress.v1" \
  -H "content-type: application/json" \
  -H "x-zitadel-ingestor-secret: $SECRET" \
  -d "{
    \"delivery_id\": \"local-create-alice\",
    \"event_type\": \"user.human.created\",
    \"provider_subject\": \"$E2E_HUMAN_ALICE_UID\",
    \"email\": \"alice@e2e.local\",
    \"display_name\": \"Alice\",
    \"approval_status\": \"approved\"
  }"
# → published zitadel.user.human.created.v1
# → after outbox drain: auth_users row (chat/blob joins resolve display_name)
```

### GraphQL join (after import)

```graphql
query {
  chat_messages(where: { room_id: { _eq: "lobby" } }) {
    body
    author_id
    author { display_name email status }
  }
  blob_games {
    game_id
    owner_id
    owner { display_name email }
  }
  auth_users {
    user_id
    display_name
    chat_messages { body }
    blob_games { score }
  }
}
```

`user_id` on `auth_users` is the Zitadel subject. Chat `author_id` and blob `owner_id`
are the session principal (OIDC `sub`) — they match after you ingest that subject.

## Local run

```bash
cd tests/e2e-ui
export ZITADEL_INGESTOR_SECRET=dev-secret-change-me
# optional in e2e-ui.env for make run
make run
```

Outbox dispatcher + bus consumers are already spawned by `e2e-runner`.

## Tests

```bash
cargo test -p e2e-service --lib
cargo test -p e2e-readmodels --lib
```
