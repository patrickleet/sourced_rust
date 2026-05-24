use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::catalog_service::{AddProduct, CatalogRepo, Product};

pub const COMMAND: &str = "product.add";

pub fn guard(ctx: &Context<CatalogRepo>) -> bool {
    ctx.has_fields(&["id", "name", "unit_cents"])
}

pub fn handle(ctx: &Context<CatalogRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<AddProduct>()?;
    if ctx.repo().peek(&input.id)?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "product {} already exists",
            input.id
        )));
    }

    let mut product = Product::default();
    product.add(input.id.clone(), input.name.clone(), input.unit_cents)?;

    let mut outbox = OutboxMessage::domain_event("product.added", &product)?;
    ctx.repo().outbox(&mut outbox).commit(&mut product)?;

    Ok(json!({ "id": input.id }))
}
