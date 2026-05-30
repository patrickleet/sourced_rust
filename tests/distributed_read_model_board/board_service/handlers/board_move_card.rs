use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::OutboxMessage;

use crate::board_service::{Board, BoardRepo, MoveCard};

pub const COMMAND: &str = "board.move_card";

pub fn guard(ctx: &Context<BoardRepo>) -> bool {
    ctx.has_fields(&["id", "card_id", "column"])
}

pub async fn handle(ctx: &Context<'_, BoardRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<MoveCard>()?;

    let mut board: Board = ctx
        .repo()
        .get(&input.id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    board.move_card(input.card_id.clone(), input.column.clone())?;

    let outbox = OutboxMessage::domain_event("board.card_moved", &board)?;
    ctx.repo().outbox(outbox).commit(&mut board).await?;

    Ok(json!({ "id": input.id, "card_id": input.card_id, "column": input.column }))
}
