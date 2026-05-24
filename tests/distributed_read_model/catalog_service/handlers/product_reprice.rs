use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::catalog_service::{CatalogRepo, Product, RepriceProduct};

pub const COMMAND: &str = "product.reprice";

pub fn guard(ctx: &Context<CatalogRepo>) -> bool {
    ctx.has_fields(&["id", "unit_cents"])
}

pub fn handle(ctx: &Context<CatalogRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<RepriceProduct>()?;
    if input.unit_cents <= 0 {
        return Err(HandlerError::Rejected("price must be positive".to_string()));
    }

    let mut product: Product = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    product.reprice(input.unit_cents)?;

    let mut outbox = OutboxMessage::domain_event("product.repriced", &product)?;
    ctx.repo().outbox(&mut outbox).commit(&mut product)?;

    Ok(json!({ "id": input.id, "unit_cents": product.unit_cents }))
}
