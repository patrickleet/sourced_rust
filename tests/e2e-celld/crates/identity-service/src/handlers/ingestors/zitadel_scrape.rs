//! Command: `zitadel.scrape.v1` — on-demand Management API reconciliation scrape.
//!
//! Authenticity: same shared secret as Action ingress (`x-zitadel-ingestor-secret`).
//! Requires `ZITADEL_SERVICE_USER_TOKEN` + `ZITADEL_API_URL` / `OIDC_ISSUER` in env.

use distributed::microsvc::{Context, HandlerError};
use serde_json::{json, Value};

use super::zitadel::scrape::{scrape_users_to_outbox, ZitadelScrapeConfig};
use super::zitadel::verify_authenticity;
use crate::deps::AuthDeps;

pub const COMMAND: &str = "zitadel.scrape.v1";

pub fn guard<R, L, S>(_ctx: &Context<AuthDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    // Empty body is fine; authenticity checked in handle.
    true
}

pub async fn handle<R, L, S>(ctx: &Context<'_, AuthDeps<R, L, S>>) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    // Not an Action event envelope — require shared secret.
    verify_authenticity(ctx.session(), false)?;

    let cfg = ZitadelScrapeConfig::from_env().ok_or_else(|| {
        HandlerError::Rejected(format!(
            "scrape not configured: set {} and {} (or OIDC_ISSUER)",
            super::zitadel::scrape::TOKEN_ENV,
            super::zitadel::scrape::API_URL_ENV
        ))
    })?;

    let leaf = ctx.repo().repo();
    let report = scrape_users_to_outbox(leaf, ctx.read_model_store(), &cfg).await;

    Ok(json!({
        "ok": report.errors.is_empty(),
        "listed": report.listed,
        "published": report.published,
        "skipped": report.skipped,
        "errors": report.errors,
    }))
}
