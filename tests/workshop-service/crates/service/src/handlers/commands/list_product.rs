//! Command: `product.list` → Product + outbox `product.listed`.

use distributed::microsvc::{Context, HandlerError};
use distributed::{OutboxMessage, ReadModelWritePlanBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use workshop_catalog_domain::{Product, ProductListed};
use workshop_readmodels::map_product_listed;

use crate::deps::ProductDeps;
use crate::handlers::util::{read_model_error, rejected};

pub const COMMAND: &str = "product.list";

#[derive(Debug, Deserialize)]
pub struct Input {
    pub product_id: String,
    pub name: String,
    pub price_cents: i64,
    pub owner_id: String,
}

pub fn guard<R, L, S>(ctx: &Context<ProductDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    ctx.has_fields(&["product_id", "name", "price_cents", "owner_id"])
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, ProductDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    let input = ctx.input::<Input>()?;
    if ctx.repo().get(&input.product_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "product {} already listed",
            input.product_id
        )));
    }

    let mut product = Product::default();
    product
        .list(
            input.product_id.clone(),
            input.name.clone(),
            input.price_cents,
            input.owner_id.clone(),
        )
        .map_err(rejected)?;

    let dto = ProductListed {
        product_id: product.product_id.clone(),
        name: product.name.clone(),
        price_cents: product.price_cents,
        owner_id: product.owner_id.clone(),
    };
    let bytes = serde_json::to_vec(&dto).map_err(|e| HandlerError::Other(Box::new(e)))?;
    let outbox = OutboxMessage::create(
        format!(
            "{}:{}:{}",
            product.product_id,
            "product.listed",
            product.entity.version()
        ),
        "product.listed",
        bytes,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;

    // Commit aggregate + outbox, then project RM (same store as GraphQL).
    // Event handlers still run for bus-driven multi-service topologies.
    ctx.repo().outbox(outbox).commit(&mut product).await?;
    let row = map_product_listed(&dto);
    let store = ctx.read_model_store();
    let mut plan = ReadModelWritePlanBuilder::new();
    plan.upsert(&row).map_err(read_model_error)?;
    plan.commit(store).await.map_err(read_model_error)?;

    Ok(json!({
        "product_id": input.product_id,
        "name": input.name,
        "price_cents": input.price_cents,
    }))
}
