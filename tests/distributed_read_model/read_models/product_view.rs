use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

/// Catalog product row, projected by the catalog service. The `attributes`
/// column is JSONB alongside the scalar columns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "products")]
pub struct ProductView {
    #[readmodel(id, column = "product_id")]
    pub product_id: String,
    pub name: String,
    pub unit_cents: i64,
    #[readmodel(jsonb)]
    pub attributes: BTreeMap<String, String>,
}
