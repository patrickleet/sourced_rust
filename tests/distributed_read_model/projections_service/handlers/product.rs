use std::collections::BTreeMap;

use sourced_rust::bus::Event;
use sourced_rust::{InMemoryReadModelStore, ReadModelCommitOutcome, ReadModelUnitOfWorkExt};

use crate::catalog_service::ProductSnapshot;
use crate::read_models::ProductView;

pub const CONSUMER: &str = "product-catalog-projection";
pub const EVENTS: &[&str] = &["product.added", "product.repriced"];

pub fn handle(store: &InMemoryReadModelStore, event: &Event) -> ReadModelCommitOutcome {
    let snapshot: ProductSnapshot = event.decode().expect("product snapshot should decode");

    let mut attributes = BTreeMap::new();
    attributes.insert("category".to_string(), "general".to_string());
    let view = ProductView {
        product_id: snapshot.id.clone(),
        name: snapshot.name.clone(),
        unit_cents: snapshot.unit_cents,
        attributes,
    };

    let mut session = store.session();
    session
        .save(&view)
        .expect("product projection should stage upsert")
        .mark_processed(CONSUMER, &event.id);
    session.commit().expect("product projection should commit")
}
