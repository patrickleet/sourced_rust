//! Command: `zitadel.ingress.v1` — verify + map + publish provider message only.

use distributed::microsvc::{Context, HandlerError};
use serde_json::{json, Value};

use super::auth::verify_authenticity;
use super::map::{looks_like_action_event, map_action_delivery, normalize_ingress_body};
use super::publish::publish_mapped_delivery;
use crate::deps::AuthDeps;

/// Public HTTP command name (POST `/{COMMAND}`).
pub const COMMAND: &str = "zitadel.ingress.v1";

pub fn guard<R, L, S>(ctx: &Context<AuthDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    !ctx.raw_input().is_null()
}

pub async fn handle<R, L, S>(ctx: &Context<'_, AuthDeps<R, L, S>>) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    let raw = ctx.raw_input().clone();
    let is_action_event = looks_like_action_event(&raw);
    verify_authenticity(ctx.session(), is_action_event)?;

    let input = normalize_ingress_body(&raw);
    let Some(mapped) = map_action_delivery(&input) else {
        return Ok(json!({
            "ok": true,
            "published": null,
            "skipped": "unmapped_event_type",
            "event_type": input.event_type,
            "action_event": is_action_event,
        }));
    };

    // Provider envelope only — projector maps to auth_users.
    let leaf = ctx.repo().repo();
    publish_mapped_delivery(leaf, &mapped)
        .await
        .map_err(|e| HandlerError::Other(Box::new(std::io::Error::other(e))))?;

    Ok(json!({
        "ok": true,
        "published": mapped.message_name,
        "event_id": mapped.delivery_id,
        "provider_subject": mapped.payload.provider_subject,
        "user_kind": mapped.payload.user_kind,
        "approval_status": mapped.payload.approval_status,
        "action_event": is_action_event,
    }))
}
