//! Projects `workshop_order.placed` → `workshop_orders` read model.

use distributed::microsvc::{Context, HandlerError};
use distributed::ReadModelWritePlanBuilder;
use serde_json::{json, Value};
use workshop_orders_domain::WorkshopOrderPlaced;
use workshop_readmodels::map_order_placed;

use crate::deps::OrderDeps;
use crate::handlers::util::{decode_payload, read_model_error};

pub const EVENT: &str = "workshop_order.placed";

pub fn guard<R, L, S>(_ctx: &Context<OrderDeps<R, L, S>>) -> bool
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    true
}

pub async fn handle<R, L, S>(
    ctx: &Context<'_, OrderDeps<R, L, S>>,
) -> Result<Value, HandlerError>
where
    R: crate::bounds::EventStore,
    L: crate::bounds::Locks,
    S: crate::bounds::ReadStore,
{
    let event: WorkshopOrderPlaced = decode_payload(ctx.message())?;
    let row = map_order_placed(&event);
    let store = ctx.read_model_store();
    let mut plan = ReadModelWritePlanBuilder::new();
    plan.upsert(&row).map_err(read_model_error)?;
    plan.commit(store).await.map_err(read_model_error)?;
    Ok(json!({ "event": EVENT, "order_id": event.order_id }))
}
