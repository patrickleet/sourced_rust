use std::collections::BTreeMap;

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{InMemoryReadModelStore, ReadModelUnitOfWorkExt};

use crate::catalog_service::ProductSnapshot;
use crate::read_models::ProductView;

pub const CONSUMER: &str = "product-catalog-projection";
pub const EVENTS: &[&str] = &["product.added", "product.repriced"];

pub fn guard(ctx: &Context<InMemoryReadModelStore>) -> bool {
    ctx.has_fields(&["id", "event_type", "payload"])
}

pub fn handle(ctx: &Context<InMemoryReadModelStore>) -> Result<Value, HandlerError> {
    let event = super::event(ctx)?;
    let snapshot: ProductSnapshot = event
        .decode()
        .map_err(|err| HandlerError::DecodeFailed(format!("product snapshot: {err}")))?;

    let mut attributes = BTreeMap::new();
    attributes.insert("category".to_string(), "general".to_string());
    let view = ProductView {
        product_id: snapshot.id.clone(),
        name: snapshot.name.clone(),
        unit_cents: snapshot.unit_cents,
        attributes,
    };

    let mut session = ctx.repo().session();
    session
        .save(&view)
        .map_err(super::read_model_error)?
        .mark_processed(CONSUMER, &event.id);
    session.commit().map_err(super::read_model_error)?;

    Ok(json!({ "event_id": event.id }))
}
