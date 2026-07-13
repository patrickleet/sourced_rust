# workshop-service fixture

Realistic multi-crate **Cargo workspace** under `distributed/tests/workshop-service`.  
Domain is a **maker workshop** marketplace (catalog + orders) — not a copy of gitkb auth/billing/platform.

## Crate map

| Path | Package | Role |
|------|---------|------|
| `crates/catalog-domain` | `workshop-catalog-domain` | Product aggregate (list / reprice / unlist) |
| `crates/orders-domain` | `workshop-orders-domain` | WorkshopOrder aggregate (place / fulfill / cancel) |
| `crates/readmodels` | `workshop-readmodels` | Shared `ProductView` + `OrderView` + manifest |
| `crates/service` | `workshop-service` | Composable handlers + GraphQL engine builder |
| `crates/runner-monolith` | `workshop-runner-monolith` | One process: all handlers + GraphQL |
| `crates/runner-split` | `workshop-runner-split` | Catalog / orders / gateway binaries |
| `crates/suite` | `workshop-suite` | Shared behavioral tests (HTTP only) |
| `ui/` | `workshop-ui` | SvelteKit UI (the-website patterns) |

## Single service (monolith)

```bash
cd tests/workshop-service
cargo run -p workshop-runner-monolith
# BIND=127.0.0.1:8791  DATABASE_URL=sqlite::memory:
```

Handlers for **both** BCs register on one `Service` via `build_full_service`.

## Microservices (handler relocation)

Same domain + readmodel crates. Only **where handlers run** changes:

```text
build_catalog_service  →  workshop-catalog  (product.*)
build_orders_service   →  workshop-orders   (workshop_order.*)
workshop-split-all     →  gateway: GraphQL + proxies commands by name prefix
```

```bash
# In-process multi topology (suite-friendly):
cargo run -p workshop-runner-split --bin workshop-split-all
# GATEWAY_BIND=127.0.0.1:8794
```

Or two OS processes + shared `DATABASE_URL` file SQLite:

```bash
export DATABASE_URL='sqlite:./workshop-split.db?mode=rwc'
cargo run -p workshop-runner-split --bin workshop-catalog &  # :8792
cargo run -p workshop-runner-split --bin workshop-orders &   # :8793
```

## Shared suite (same assertions)

```bash
# Monolith (default: boots in-process SQLite+InMemoryBus)
cargo test -p workshop-suite --test behavioral

# Multi-service (same case IDs)
cargo test -p workshop-suite --test multi_service

# Against external process
WORKSHOP_BASE_URL=http://127.0.0.1:8794 cargo test -p workshop-suite --test behavioral
```

Case IDs: `W1_list_product`, `W2_place_order`, `W3_graphql_products`, `W4_graphql_order_isolation`.

## Matrix cells

| Cell | How |
|------|-----|
| SQLite memory + InMemoryBus | suite default (monolith) |
| SQLite file + SqliteBus | runners (`DATABASE_URL=sqlite:./….db?mode=rwc`) |
| Multi-service | `--test multi_service` or `workshop-split-all` |
| OIDC | set `OIDC_ISSUER` + `OIDC_AUDIENCE` (+ `OIDC_JWKS_URI`); else DevHeaders |

## UI

```bash
cd ui && npm install && npm run build && npm test
# optional: WORKSHOP_BASE_URL=http://127.0.0.1:8791 npm test
```

Patterns from hops `sites/the-website`: GraphQL server helper, protected route, session cookies / identity headers.

## Spec

See [[specs/workshop-domain]] (GitKB) and `docs/layout.md` for the layout + microservice split advice.
