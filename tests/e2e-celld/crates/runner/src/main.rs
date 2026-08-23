//! Celld example runner (not `tests/e2e-ui` / `make run`).
//!
//! Env:
//! - `CELLD_URL` — required
//! - `DATABASE_URL` — `sqlite:…` (default) or `postgres://…`
//! - `BIND` (default `127.0.0.1:8791`)
//! - `OIDC_*` → OidcBearer; else DevHeaders

use std::env;

use e2e_celld_graphql::{identity_from_env, run, HostOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:./e2e-celld.db?mode=rwc".into());
    let bind = env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8791".into());
    let celld_url = env::var("CELLD_URL").map_err(|_| {
        "CELLD_URL is required. Start infra: make -C tests/e2e-ui up-celld-nats"
    })?;
    eprintln!("e2e-celld CELLD_URL={celld_url}");
    run(
        &database_url,
        HostOptions {
            bind,
            identity: identity_from_env(),
            celld_url,
        },
    )
    .await
}
