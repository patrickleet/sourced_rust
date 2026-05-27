use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::board_service::{Board, BoardRepo, MoveCard};

pub const COMMAND: &str = "board.move_card";
pub const SPEC: sourced_rust::microsvc::HandlerSpec =
    sourced_rust::microsvc::HandlerSpec::command(COMMAND);

pub fn guard(ctx: &Context<BoardRepo>) -> bool {
    ctx.has_fields(&["id", "card_id", "column"])
}

pub fn handle(ctx: &Context<BoardRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<MoveCard>()?;

    let mut board: Board = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    board.move_card(input.card_id.clone(), input.column.clone())?;

    let outbox = OutboxMessage::domain_event("board.card_moved", &board)?;
    ctx.repo().outbox(outbox).commit(&mut board)?;

    Ok(json!({ "id": input.id, "card_id": input.card_id, "column": input.column }))
}
