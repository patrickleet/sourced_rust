use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::account_service::account::{Account, OpenAccount};
use crate::account_service::AccountRepo;

pub const COMMAND: &str = "account.open";

pub fn guard(ctx: &Context<AccountRepo>) -> bool {
    ctx.has_fields(&["id", "owner"])
}

pub fn handle(ctx: &Context<AccountRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<OpenAccount>()?;

    if ctx.repo().peek(&input.id)?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "account {} already exists",
            input.id
        )));
    }

    let mut account = Account::default();
    account.open(input.id.clone(), input.owner.clone())?;

    let mut outbox = OutboxMessage::domain_event("AccountOpened", &account)?;
    ctx.repo().outbox(&mut outbox).commit(&mut account)?;

    Ok(json!({ "id": input.id, "owner": input.owner }))
}
