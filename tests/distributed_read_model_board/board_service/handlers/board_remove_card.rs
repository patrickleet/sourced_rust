use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxMessage, SyncOutboxCommitExt};

use crate::board_service::{Board, BoardRepo, RemoveCard};

pub const COMMAND: &str = "board.remove_card";

pub fn guard(ctx: &Context<BoardRepo>) -> bool {
    ctx.has_fields(&["id", "card_id"])
}

pub fn handle(ctx: &Context<BoardRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<RemoveCard>()?;

    let mut board: Board = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    board.remove_card(input.card_id.clone())?;

    let outbox = OutboxMessage::domain_event("board.card_removed", &board)?;
    ctx.repo().outbox_sync(outbox).commit_sync(&mut board)?;

    Ok(json!({ "id": input.id, "card_id": input.card_id }))
}
