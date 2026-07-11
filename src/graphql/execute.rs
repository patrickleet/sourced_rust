//! Dialect executors: run a SqlPlan and decode the single JSON column.

use async_graphql::Value;
use serde_json::Value as JsonValue;

use super::compile::{BindValue, SqlDialect, SqlPlan};
use super::engine::{EngineInner, GraphqlPool};

pub async fn execute_sql(inner: &EngineInner, plan: &SqlPlan) -> Result<Value, String> {
    match &inner.pool {
        #[cfg(feature = "sqlite")]
        GraphqlPool::Sqlite(pool) => execute_sqlite(pool, plan).await,
        #[cfg(feature = "postgres")]
        GraphqlPool::Postgres(pool) => {
            execute_postgres(pool, plan, inner.statement_timeout).await
        }
        #[allow(unreachable_patterns)]
        _ => Err("no database pool available for GraphQL execution".into()),
    }
}

#[cfg(feature = "sqlite")]
async fn execute_sqlite(
    pool: &sqlx::SqlitePool,
    plan: &SqlPlan,
) -> Result<Value, String> {
    use sqlx::Row;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| format!("sqlite begin: {e}"))?;

    // SQL is compiler-produced from schema metadata + bound parameters only
    // (never raw client strings). AssertSqlSafe documents the audit.
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
    let row = qb
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| format!("sqlite execute: {e}"))?;
    tx.commit()
        .await
        .map_err(|e| format!("sqlite commit: {e}"))?;

    let raw: Option<String> = row.try_get(0).ok();
    let text = raw.unwrap_or_else(|| "null".into());
    let mut json: JsonValue =
        serde_json::from_str(&text).map_err(|e| format!("json decode: {e}"))?;
    // SQLite `json_group_array(json_object(...))` can leave nested objects as
    // JSON text; recursively parse string-encoded JSON so GraphQL sees objects.
    deep_parse_json_strings(&mut json);
    rewrite_hex_bytes(&mut json, &plan.bytes_hex_paths);
    Value::from_json(json).map_err(|e| format!("graphql value: {e}"))
}

fn deep_parse_json_strings(value: &mut JsonValue) {
    match value {
        JsonValue::String(s) => {
            let t = s.trim();
            if (t.starts_with('{') && t.ends_with('}'))
                || (t.starts_with('[') && t.ends_with(']'))
            {
                if let Ok(mut parsed) = serde_json::from_str::<JsonValue>(s) {
                    deep_parse_json_strings(&mut parsed);
                    *value = parsed;
                }
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                deep_parse_json_strings(item);
            }
        }
        JsonValue::Object(map) => {
            for v in map.values_mut() {
                deep_parse_json_strings(v);
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
    let json: JsonValue =
        serde_json::from_str(&text).map_err(|e| format!("json decode: {e}"))?;
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
    if hex.len() % 2 != 0 {
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
