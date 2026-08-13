use distributed::microsvc::{Context, HandlerError};
use serde_json::{json, Value};

use crate::board_service::handlers::board_state_message;
use crate::board_service::{AddCard, Board, BoardRepo};

pub const COMMAND: &str = "board.add_card";

pub fn guard(ctx: &Context<BoardRepo>) -> bool {
    ctx.has_fields(&["id", "card_id", "column", "title"])
}

pub async fn handle(ctx: &Context<'_, BoardRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<AddCard>()?;

    let mut board: Board = ctx
        .repo()
        .get(&input.id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    board.add_card(
        input.card_id.clone(),
        input.column.clone(),
        input.title.clone(),
        input.labels.clone(),
        input.assignee.clone(),
    )?;

    let outbox = board_state_message(&board, "board.card_added")?;
    ctx.repo().outbox(outbox).commit(&mut board).await?;

    Ok(json!({ "id": input.id, "card_id": input.card_id }))
}
