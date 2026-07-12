//! Soft-skip / fail-open contracts (until strict_where / harden-16).
//!
//! **Two layers:**
//! 1. **Schema** — `*_bool_exp` / `*_order_by` only expose granted columns, so
//!    unknown or denied keys usually fail GraphQL validation (no SQL).
//! 2. **Compile soft-skip** — if a key reaches `compile_client_where` /
//!    `compile_order_by` (e.g. future IR path), denied/unknown keys are
//!    ignored (`continue`) while other predicates still apply.
//!
//! Do not flip either layer to fail-closed without an explicit product decision.

use async_graphql::Request;
use distributed::graphql::{select, GraphqlEngine, ModelPermissions};

use super::common::{assert_no_sql_leak, seed_orders, session, OrderView};

/// Unknown where key: schema rejects (preferred) or soft-skips; never SQL leak.
#[tokio::test]
async fn unknown_where_key_rejected_or_soft_skipped_without_sql_leak() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().role("user", select().all_columns()))
        .build()
        .unwrap();
    let s = session("user", "x");
    let resp = engine
        .execute(
            &s,
            Request::new(
                r#"{ orders(where: { not_a_real_column: { _eq: "x" }, status: { _eq: "open" } }) { order_id status } }"#,
            ),
        )
        .await;
    assert_no_sql_leak(&resp);
    if resp.errors.is_empty() {
        // Compile soft-skip path: remaining status filter applies.
        let data = serde_json::to_value(&resp.data).unwrap();
        let orders = data["orders"].as_array().unwrap();
        assert!(
            orders.iter().all(|o| o["status"] == "open"),
            "soft-skip must keep status filter: {data}"
        );
    } else {
        // Schema gate path (current default for free-form keys).
        let msgs = resp
            .errors
            .iter()
            .map(|e| e.message.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            msgs.contains("unknown field") || msgs.contains("not_a_real_column"),
            "expected unknown field, got {msgs}"
        );
    }
}

/// Denied column in where: schema omits field → validation error, or soft-skip.
#[tokio::test]
async fn denied_where_column_schema_or_soft_skip_without_sql_leak() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["restricted"])
        .model::<OrderView>(
            ModelPermissions::new().role(
                "restricted",
                select().columns(["order_id", "status"]),
            ),
        )
        .build()
        .unwrap();
    let s = session("restricted", "x");
    let resp = engine
        .execute(
            &s,
            Request::new(
                r#"{ orders(where: { total_cents: { _eq: 100 }, status: { _eq: "open" } }) { order_id status } }"#,
            ),
        )
        .await;
    assert_no_sql_leak(&resp);
    if resp.errors.is_empty() {
        let data = serde_json::to_value(&resp.data).unwrap();
        for o in data["orders"].as_array().unwrap() {
            assert_eq!(o["status"], "open");
        }
    } else {
        let msgs = resp
            .errors
            .iter()
            .map(|e| e.message.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            msgs.contains("total_cents") || msgs.contains("unknown field"),
            "expected unknown field for denied column, got {msgs}"
        );
    }
}

/// Junk order_by column: schema rejects or soft-skips; no SQL leak.
#[tokio::test]
async fn junk_order_by_column_schema_or_soft_skip_without_sql_leak() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().role("user", select().all_columns()))
        .build()
        .unwrap();
    let s = session("user", "x");
    let resp = engine
        .execute(
            &s,
            Request::new(
                r#"{ orders(order_by: [{ totally_fake: asc }, { status: desc }]) { order_id status } }"#,
            ),
        )
        .await;
    assert_no_sql_leak(&resp);
    if resp.errors.is_empty() {
        let data = serde_json::to_value(&resp.data).unwrap();
        assert_eq!(data["orders"].as_array().unwrap().len(), 3);
    } else {
        let msgs = resp
            .errors
            .iter()
            .map(|e| e.message.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            msgs.contains("totally_fake") || msgs.contains("unknown field"),
            "expected unknown field for junk order_by, got {msgs}"
        );
    }
}
