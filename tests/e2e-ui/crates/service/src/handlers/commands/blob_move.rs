//! Command: `blob.move` — direction up|down|left|right.

use blob_domain::Direction;
use distributed::microsvc::{Context, HandlerError};
use serde::Deserialize;
use serde_json::Value;

use crate::deps::BlobDeps;
use crate::handlers::commands::blob_cmd::{
    commit_blob_event, fact_json, load_game, map_domain,
};
use crate::handlers::util::{require_user, session_has_user};

pub const COMMAND: &str = "blob.move";

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobMoveInput {
    pub game_id: String,
    pub direction: String,
}

pub fn guard<R, L, S>(ctx: &Context<BlobDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    ctx.has_fields(&["game_id", "direction"]) && session_has_user(ctx.session())
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, BlobDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    let owner = require_user(ctx.session())?;
    let input = ctx.input::<BlobMoveInput>()?;
    let dir = Direction::parse(&input.direction).ok_or_else(|| {
        HandlerError::Rejected(format!(
            "invalid direction `{}` (use up|down|left|right)",
            input.direction
        ))
    })?;

    let mut game = load_game(ctx, &input.game_id).await?;
    game.move_dir(&owner, dir).map_err(map_domain)?;

    let fact = commit_blob_event(ctx, &mut game, "blob.moved").await?;
    Ok(fact_json(&fact))
}
