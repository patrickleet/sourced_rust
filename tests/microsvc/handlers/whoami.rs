//! Handler: whoami
//!
//! Returns the authenticated user's ID from the session.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};

use super::Repo;

pub const COMMAND: &str = "whoami";

pub fn guard(_ctx: &Context<Repo>) -> bool {
    true
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let user_id = ctx.user_id()?;
    Ok(json!({ "user_id": user_id }))
}
