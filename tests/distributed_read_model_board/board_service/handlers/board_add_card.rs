use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::board_service::{AddCard, Board, BoardRepo};

pub const COMMAND: &str = "board.add_card";

pub fn guard(ctx: &Context<BoardRepo>) -> bool {
    ctx.has_fields(&["id", "card_id", "column", "title"])
}

pub fn handle(ctx: &Context<BoardRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<AddCard>()?;

    let mut board: Board = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    board.add_card(
        input.card_id.clone(),
        input.column.clone(),
        input.title.clone(),
        input.labels.clone(),
        input.assignee.clone(),
    )?;

    let outbox = OutboxMessage::domain_event("board.card_added", &board)?;
    ctx.repo().outbox(outbox).commit(&mut board)?;

    Ok(json!({ "id": input.id, "card_id": input.card_id }))
}
