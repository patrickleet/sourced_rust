//! e2e-ui runner — one-screen host invocation.
//!
//! Env:
//! - `DATABASE_URL` — `sqlite:…` (default) or `postgres://…`
//! - `BIND` (default `127.0.0.1:8791`)
//! - `OIDC_ISSUER` / `OIDC_AUDIENCE` / `OIDC_JWKS_URI` → OidcBearer; else DevHeaders
//! - `ZITADEL_SERVICE_USER_TOKEN` + `OIDC_ISSUER`/`ZITADEL_API_URL` → periodic user scrape
//! - `ZITADEL_SCRAPE_INTERVAL_SECS` (default 60; `0` = no background loop)

use std::env;

use e2e_service::{identity_from_env, run, HostOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:./e2e-ui.db?mode=rwc".into());
    let bind = env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8791".into());
    run(
        &database_url,
        HostOptions {
            bind,
            identity: identity_from_env(),
        },
    )
    .await
}
