use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::account_service::account::{Account, DepositMoney};
use crate::account_service::AccountRepo;

pub const COMMAND: &str = "account.deposit";

pub fn guard(ctx: &Context<AccountRepo>) -> bool {
    ctx.has_fields(&["id", "amount_cents"])
}

pub fn handle(ctx: &Context<AccountRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<DepositMoney>()?;
    if input.amount_cents <= 0 {
        return Err(HandlerError::Rejected(
            "deposit amount must be positive".to_string(),
        ));
    }

    let mut account: Account = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    account.deposit(input.amount_cents)?;

    let mut outbox = OutboxMessage::domain_event("MoneyDeposited", &account)?;
    ctx.repo().outbox(&mut outbox).commit(&mut account)?;

    Ok(json!({
        "id": input.id,
        "balance_cents": account.balance_cents,
    }))
}
