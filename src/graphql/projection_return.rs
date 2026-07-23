//! Helpers for **same-tx projection** command returns (**library / optional topology**).
//!
//! # Product rule
//!
//! | Topology | Return | Client `result.kind` |
//! |----------|--------|----------------------|
//! | Async projector (default Seeker / e2e-ui todos) | ack or domain-fact fields | `ack` / `fact` + reconcile |
//! | Handler commits read model in the same request | view-shaped JSON | `projection` |
//!
//! If you write the read model in the command handler, **return it**.
//! If you do not, do **not** invent a full list row — return ack/fact only.
//!
//! # When to use these helpers
//!
//! - **Use** [`stage_projection_and_payload`] / [`projection_return_value`] only when the
//!   command handler **intentionally** commits the read model in the same request
//!   (e.g. game / same-tx demos) and the client policy is `result.kind = projection`.
//! - **Do not use** for default async projector commands (e2e-ui todos, chat posts).
//!   Those return fact/ack and reconcile via subscription or delayed refetch.
//!
//! e2e-ui todos create returns fact-shaped fields from the domain fact — treat as
//! **`fact`**, not projection-from-store (projectors are separate event handlers).

use serde::Serialize;

use crate::read_model::{ReadModelWritePlanBuilder, RelationalReadModel};
use crate::table::TableStoreError;

/// JSON value returned from a GraphQL command mutation when the payload is
/// view-shaped (same fields the read model / GraphQL object exposes).
pub type ProjectionPayload = serde_json::Value;

/// Serialize a row as the GraphQL mutation payload after (or as part of)
/// committing the same row to the read model in this request.
///
/// Prefer [`stage_projection_and_payload`] so the upsert and the returned JSON
/// cannot drift.
pub fn projection_return_value<V: Serialize>(row: &V) -> Result<ProjectionPayload, String> {
    serde_json::to_value(row).map_err(|e| e.to_string())
}

/// Stage a full-row upsert on `plan` and return the same value as GraphQL JSON.
///
/// Call only when the handler **intentionally** commits the projection in the
/// same request. Do **not** use for default async-projector commands (todos).
///
/// ```ignore
/// let mut plan = ReadModelWritePlanBuilder::new();
/// let payload = stage_projection_and_payload(&mut plan, &view)?;
/// // commit plan with the aggregate/outbox in the same unit of work
/// Ok(payload)
/// ```
pub fn stage_projection_and_payload<M>(
    plan: &mut ReadModelWritePlanBuilder,
    row: &M,
) -> Result<ProjectionPayload, TableStoreError>
where
    M: RelationalReadModel + Serialize,
{
    plan.upsert(row)?;
    serde_json::to_value(row)
        .map_err(|e| TableStoreError::Metadata(format!("projection payload serialize failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct ViewRow {
        todo_id: String,
        title: String,
        status: String,
    }

    #[test]
    fn projection_return_value_is_view_shaped_json() {
        let row = ViewRow {
            todo_id: "t1".into(),
            title: "hi".into(),
            status: "open".into(),
        };
        let v = projection_return_value(&row).expect("json");
        assert_eq!(v["todo_id"], "t1");
        assert_eq!(v["title"], "hi");
        assert_eq!(v["status"], "open");
        // Only fields present on the row — no invented admin/secret keys
        assert!(v.get("secret").is_none());
    }
}
