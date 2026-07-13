//! Shared read models for the workshop fixture (catalog + orders).
//!
//! One crate covers both BCs (same packaging advice as gitkb-readmodels).

use distributed::ReadModel;
use serde::{Deserialize, Serialize};
use workshop_catalog_domain::ProductListed;
use workshop_orders_domain::WorkshopOrderPlaced;

/// Catalog listing projection. PK: `product_id`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[table("products")]
pub struct ProductView {
    #[id("product_id")]
    pub product_id: String,
    pub name: String,
    pub price_cents: i64,
    pub owner_id: String,
    pub listed: bool,
}

/// Order list projection. PK: `order_id`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[table("workshop_orders")]
pub struct OrderView {
    #[id("order_id")]
    pub order_id: String,
    pub product_id: String,
    pub customer_id: String,
    pub quantity: i64,
    pub status: String,
}

pub fn map_product_listed(e: &ProductListed) -> ProductView {
    ProductView {
        product_id: e.product_id.clone(),
        name: e.name.clone(),
        price_cents: e.price_cents,
        owner_id: e.owner_id.clone(),
        listed: true,
    }
}

pub fn map_order_placed(e: &WorkshopOrderPlaced) -> OrderView {
    OrderView {
        order_id: e.order_id.clone(),
        product_id: e.product_id.clone(),
        customer_id: e.customer_id.clone(),
        quantity: e.quantity as i64,
        status: "placed".into(),
    }
}

/// Inventory table names for static checks.
pub const INVENTORY_TABLES: &[&str] = &["products", "workshop_orders"];

/// Register schemas on an in-memory store (tests).
pub fn register_all_schemas(
    store: &distributed::InMemoryReadModelStore,
) -> Result<(), distributed::TableStoreError> {
    match store.register_schema::<ProductView>() {
        Ok(()) => {}
        Err(distributed::TableStoreError::Metadata(msg)) if msg.contains("already contains") => {}
        Err(e) => return Err(e),
    }
    match store.register_schema::<OrderView>() {
        Ok(()) => {}
        Err(distributed::TableStoreError::Metadata(msg)) if msg.contains("already contains") => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

/// Build a [`DistributedProjectManifest`] for schema bootstrap / GraphQL.
/// Uses ReadModel-derived schemas so `_sourced_version` and indexes match upserts.
pub fn distributed_manifest() -> distributed::DistributedProjectManifest {
    use distributed::RelationalReadModel;

    distributed::DistributedProjectManifest::new("workshop-service")
        .table_schema(ProductView::schema().clone())
        .table_schema(OrderView::schema().clone())
}
