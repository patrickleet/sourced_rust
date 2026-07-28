use distributed::microsvc::{Context, HandlerError};
use serde_json::{json, Value};

use crate::board_service::handlers::board_state_message;
use crate::board_service::{Board, BoardRepo, OpenBoard};

pub const COMMAND: &str = "board.open";

pub fn guard(ctx: &Context<BoardRepo>) -> bool {
    ctx.has_fields(&["id", "name"])
}

pub async fn handle(ctx: &Context<'_, BoardRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<OpenBoard>()?;
    if ctx.repo().peek(&input.id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "board {} already exists",
            input.id
        )));
    }

    let mut board = Board::default();
    board.open(input.id.clone(), input.name.clone())?;

    let outbox = board_state_message(&board, "board.opened")?;
    ctx.repo().outbox(outbox).commit(&mut board).await?;

    Ok(json!({ "id": input.id }))
}
