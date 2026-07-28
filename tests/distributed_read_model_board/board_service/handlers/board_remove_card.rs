use distributed::microsvc::{Context, HandlerError};
use serde_json::{json, Value};

use crate::board_service::handlers::board_state_message;
use crate::board_service::{Board, BoardRepo, RemoveCard};

pub const COMMAND: &str = "board.remove_card";

pub fn guard(ctx: &Context<BoardRepo>) -> bool {
    ctx.has_fields(&["id", "card_id"])
}

pub async fn handle(ctx: &Context<'_, BoardRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<RemoveCard>()?;

    let mut board: Board = ctx
        .repo()
        .get(&input.id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    board.remove_card(input.card_id.clone())?;

    let outbox = board_state_message(&board, "board.card_removed")?;
    ctx.repo().outbox(outbox).commit(&mut board).await?;

    Ok(json!({ "id": input.id, "card_id": input.card_id }))
}
