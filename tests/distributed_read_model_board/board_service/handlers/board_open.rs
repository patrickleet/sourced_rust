use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::board_service::{Board, BoardRepo, OpenBoard};

pub const COMMAND: &str = "board.open";

pub fn guard(ctx: &Context<BoardRepo>) -> bool {
    ctx.has_fields(&["id", "name"])
}

pub fn handle(ctx: &Context<BoardRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<OpenBoard>()?;
    if ctx.repo().peek(&input.id)?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "board {} already exists",
            input.id
        )));
    }

    let mut board = Board::default();
    board.open(input.id.clone(), input.name.clone())?;

    let outbox = OutboxMessage::domain_event("board.opened", &board)?;
    ctx.repo().outbox(outbox).commit(&mut board)?;

    Ok(json!({ "id": input.id }))
}
