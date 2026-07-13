//! Command: `workshop_order.place` → WorkshopOrder + outbox `workshop_order.placed`.

use distributed::microsvc::{Context, HandlerError};
use distributed::{OutboxMessage, ReadModelWritePlanBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use workshop_orders_domain::{WorkshopOrder, WorkshopOrderPlaced};
use workshop_readmodels::map_order_placed;

use crate::deps::OrderDeps;
use crate::handlers::util::{read_model_error, rejected};

pub const COMMAND: &str = "workshop_order.place";

#[derive(Debug, Deserialize)]
pub struct Input {
    pub order_id: String,
    pub product_id: String,
    pub customer_id: String,
    pub quantity: u32,
}

pub fn guard<R, L, S>(ctx: &Context<OrderDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: Send + Sync + 'static,
{
    ctx.has_fields(&["order_id", "product_id", "customer_id", "quantity"])
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, OrderDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    let input = ctx.input::<Input>()?;
    if ctx.repo().get(&input.order_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "order {} already exists",
            input.order_id
        )));
    }

    let mut order = WorkshopOrder::default();
    order
        .place(
            input.order_id.clone(),
            input.product_id.clone(),
            input.customer_id.clone(),
            input.quantity,
        )
        .map_err(rejected)?;

    let dto = WorkshopOrderPlaced {
        order_id: order.order_id.clone(),
        product_id: order.product_id.clone(),
        customer_id: order.customer_id.clone(),
        quantity: order.quantity,
    };
    let bytes = serde_json::to_vec(&dto).map_err(|e| HandlerError::Other(Box::new(e)))?;
    let outbox = OutboxMessage::create(
        format!(
            "{}:{}:{}",
            order.order_id,
            "workshop_order.placed",
            order.entity.version()
        ),
        "workshop_order.placed",
        bytes,
    )
    .map_err(|e| HandlerError::Other(Box::new(e)))?;

    ctx.repo().outbox(outbox).commit(&mut order).await?;
    let row = map_order_placed(&dto);
    let store = ctx.read_model_store();
    let mut plan = ReadModelWritePlanBuilder::new();
    plan.upsert(&row).map_err(read_model_error)?;
    plan.commit(store).await.map_err(read_model_error)?;

    Ok(json!({
        "order_id": input.order_id,
        "product_id": input.product_id,
        "customer_id": input.customer_id,
        "quantity": input.quantity,
    }))
}
