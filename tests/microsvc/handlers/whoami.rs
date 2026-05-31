//! Handler: whoami
//!
//! Returns the authenticated user's ID from the session.

use distributed::microsvc::{Context, HandlerError};
use serde_json::{json, Value};

use super::Repo;

pub const COMMAND: &str = "whoami";

pub fn guard(_ctx: &Context<Repo>) -> bool {
    true
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let user_id = ctx.user_id()?;
    Ok(json!({ "user_id": user_id }))
}
