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
    if input.unit_cents <= 0 {
        return Err(HandlerError::Rejected("price must be positive".to_string()));
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use sourced_rust::microsvc::Session;
    use sourced_rust::{AggregateBuilder, HashMapRepository, Queueable};

    #[test]
    fn handle_rejects_non_positive_unit_cents() {
        let store = HashMapRepository::new();
        let service = crate::catalog_service::service(store.clone().queued().aggregate());

        let err = service
            .dispatch(
                COMMAND,
                json!({
                    "id": "prod-widget",
                    "name": "Widget",
                    "unit_cents": 0,
                }),
                Session::new(),
            )
            .unwrap_err();

        assert!(matches!(
            err,
            HandlerError::Rejected(ref message) if message == "price must be positive"
        ));
    }
}
