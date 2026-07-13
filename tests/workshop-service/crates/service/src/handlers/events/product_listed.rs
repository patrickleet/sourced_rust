//! Projects `product.listed` → `products` read model.

use distributed::microsvc::{Context, HandlerError};
use distributed::ReadModelWritePlanBuilder;
use serde_json::{json, Value};
use workshop_catalog_domain::ProductListed;
use workshop_readmodels::map_product_listed;

use crate::deps::ProductDeps;
use crate::handlers::util::{decode_payload, read_model_error};

pub const EVENT: &str = "product.listed";

pub fn guard<R, L, S>(_ctx: &Context<ProductDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    true
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, ProductDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    let event: ProductListed = decode_payload(ctx.message())?;
    let row = map_product_listed(&event);
    let store = ctx.read_model_store();
    let mut plan = ReadModelWritePlanBuilder::new();
    plan.upsert(&row).map_err(read_model_error)?;
    plan.commit(store).await.map_err(read_model_error)?;
    Ok(json!({ "event": EVENT, "product_id": event.product_id }))
}
