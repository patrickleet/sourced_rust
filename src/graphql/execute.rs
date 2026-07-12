//! Dialect executors: run a SqlPlan and decode the single JSON column.
#![allow(clippy::items_after_test_module)]

use std::future::Future;
use std::time::Duration;

use async_graphql::Value;
use serde_json::Value as JsonValue;

use super::compile::{BindValue, SqlDialect, SqlPlan};
use super::engine::{EngineInner, GraphqlPool};

pub async fn execute_sql(inner: &EngineInner, plan: &SqlPlan) -> Result<Value, String> {
    match &inner.pool {
        #[cfg(feature = "sqlite")]
        GraphqlPool::Sqlite(pool) => execute_sqlite(pool, plan, inner.statement_timeout).await,
        #[cfg(feature = "postgres")]
        GraphqlPool::Postgres(pool) => execute_postgres(pool, plan, inner.statement_timeout).await,
        #[allow(unreachable_patterns)]
        _ => Err("no database pool available for GraphQL execution".into()),
    }
}

/// Wall-clock budget around a single statement future.
///
/// On elapse returns `Err("statement timeout")` — the stable string mapped to
/// client `TIMEOUT` by [`super::schema::client_error_for_execute_err`].
pub(crate) async fn apply_statement_timeout<T, F>(timeout: Duration, run: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    match tokio::time::timeout(timeout, run).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("statement timeout".into()),
    }
}

#[cfg(feature = "sqlite")]
async fn execute_sqlite(
    pool: &sqlx::SqlitePool,
    plan: &SqlPlan,
    timeout: std::time::Duration,
) -> Result<Value, String> {
    use sqlx::Row;

    // SQL is compiler-produced from schema metadata + bound parameters only.
    let run = async {
        let mut qb = sqlx::query(sqlx::AssertSqlSafe(plan.sql.clone()));
        for bind in &plan.binds {
            qb = match bind {
                BindValue::Null => qb.bind(None::<String>),
                BindValue::Bool(b) => qb.bind(*b),
                BindValue::I64(i) => qb.bind(*i),
                BindValue::F64(f) => qb.bind(*f),
                BindValue::Text(s) => qb.bind(s.clone()),
                BindValue::Bytes(b) => qb.bind(b.clone()),
                BindValue::Json(j) => qb.bind(j.to_string()),
            };
        }
        // Read-only SELECT: no write transaction required.
        let row = qb
            .fetch_one(pool)
            .await
            .map_err(|e| format!("sqlite execute: {e}"))?;
        let raw: Option<String> = row
            .try_get::<Option<String>, _>(0)
            .map_err(|e| format!("sqlite json column: {e}"))?;
        Ok::<_, String>(raw.unwrap_or_else(|| "null".into()))
    };

    let text = apply_statement_timeout(timeout, run).await?;
    let mut json: JsonValue =
        serde_json::from_str(&text).map_err(|e| format!("json decode: {e}"))?;
    deep_parse_json_strings(&mut json);
    rewrite_hex_bytes(&mut json, &plan.bytes_hex_paths);
    Value::from_json(json).map_err(|e| format!("graphql value: {e}"))
}

#[cfg(test)]
mod statement_timeout_tests {
    use super::apply_statement_timeout;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn elapses_to_statement_timeout_error() {
        let run = async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok::<String, String>("never".into())
        };
        let handle =
            tokio::spawn(
                async move { apply_statement_timeout(Duration::from_millis(1), run).await },
            );
        tokio::time::advance(Duration::from_millis(5)).await;
        let err = handle.await.expect("join").expect_err("budget must elapse");
        assert_eq!(err, "statement timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn completes_when_under_budget() {
        let run = async { Ok::<String, String>("ok".into()) };
        let handle =
            tokio::spawn(async move { apply_statement_timeout(Duration::from_secs(5), run).await });
        // Allow the ready future to be polled under paused time.
        tokio::time::advance(Duration::from_millis(1)).await;
        let v: String = handle.await.expect("join").expect("under budget");
        assert_eq!(v, "ok");
    }

    #[tokio::test(start_paused = true)]
    async fn propagates_inner_error() {
        let run = async { Err::<String, String>("sqlite execute: boom".into()) };
        let handle =
            tokio::spawn(async move { apply_statement_timeout(Duration::from_secs(5), run).await });
        tokio::time::advance(Duration::from_millis(1)).await;
        let err = handle.await.expect("join").expect_err("inner err");
        assert!(err.contains("boom"), "{err}");
    }
}

/// Parse string-encoded JSON only for array elements (SQLite json_group_array
/// quirk). Object property strings stay GraphQL String scalars (never re-typed).
fn deep_parse_json_strings(value: &mut JsonValue) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                if let JsonValue::String(s) = item {
                    let trimmed = s.trim();
                    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
                        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
                    {
                        if let Ok(mut parsed) = serde_json::from_str::<JsonValue>(s) {
                            deep_parse_json_strings(&mut parsed);
                            *item = parsed;
                        }
                    }
                } else {
                    deep_parse_json_strings(item);
                }
            }
        }
        JsonValue::Object(map) => {
            for v in map.values_mut() {
                match v {
                    JsonValue::Object(_) | JsonValue::Array(_) => deep_parse_json_strings(v),
                    // Leave scalar strings alone (including JSON-looking text columns).
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

#[cfg(feature = "postgres")]
async fn execute_postgres(
    pool: &sqlx::PgPool,
    plan: &SqlPlan,
    timeout: std::time::Duration,
) -> Result<Value, String> {
    use sqlx::Row;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| format!("postgres begin: {e}"))?;
    let timeout_ms = timeout.as_millis() as i64;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = '{timeout_ms}ms'"
    )))
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("statement_timeout: {e}"))?;

    let mut qb = sqlx::query(sqlx::AssertSqlSafe(plan.sql.clone()));
    for bind in &plan.binds {
        qb = match bind {
            BindValue::Null => qb.bind(None::<String>),
            BindValue::Bool(b) => qb.bind(*b),
            BindValue::I64(i) => qb.bind(*i),
            BindValue::F64(f) => qb.bind(*f),
            BindValue::Text(s) => qb.bind(s.clone()),
            BindValue::Bytes(b) => qb.bind(b.clone()),
            // No sqlx `json` feature — bind as text; compiler adds ::jsonb casts.
            BindValue::Json(j) => qb.bind(j.to_string()),
        };
    }
    let row = qb
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| format!("postgres execute: {e}"))?;
    tx.commit()
        .await
        .map_err(|e| format!("postgres commit: {e}"))?;

    let text: String = row
        .try_get::<String, _>(0)
        .or_else(|_| {
            row.try_get::<Option<String>, _>(0)
                .map(|o| o.unwrap_or_else(|| "null".into()))
        })
        .map_err(|e| format!("postgres json column: {e}"))?;
    let json: JsonValue = serde_json::from_str(&text).map_err(|e| format!("json decode: {e}"))?;
    Value::from_json(json).map_err(|e| format!("graphql value: {e}"))
}

/// Rewrite hex-encoded Bytes paths to base64 (SQLite executor path).
pub fn rewrite_hex_bytes(json: &mut JsonValue, paths: &[String]) {
    for path in paths {
        let parts: Vec<&str> = path.split('.').collect();
        rewrite_path(json, &parts);
    }
}

fn rewrite_path(json: &mut JsonValue, parts: &[&str]) {
    if parts.is_empty() {
        return;
    }
    match json {
        JsonValue::Array(items) => {
            for item in items {
                rewrite_path(item, parts);
            }
        }
        JsonValue::Object(map) => {
            if parts.len() == 1 {
                if let Some(JsonValue::String(hex)) = map.get_mut(parts[0]) {
                    if let Some(b64) = hex_to_base64(hex) {
                        *hex = b64;
                    }
                }
            } else if let Some(child) = map.get_mut(parts[0]) {
                rewrite_path(child, &parts[1..]);
            }
        }
        _ => {}
    }
}

fn hex_to_base64(hex: &str) -> Option<String> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16).ok()?;
        bytes.push(byte);
    }
    use base64::Engine as _;
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[allow(dead_code)]
pub fn dialect_name(d: SqlDialect) -> &'static str {
    match d {
        SqlDialect::Postgres => "postgres",
        SqlDialect::Sqlite => "sqlite",
    }
}
