use async_graphql::Value;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::graphql::filter::{LitValue, Operand};
use crate::microsvc::Session;
use crate::table::ColumnType;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum BindValue {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Text(String),
    Bytes(Vec<u8>),
    Json(JsonValue),
}

pub(super) fn operand_to_bind(
    op: &Operand,
    session: &Session,
    column_type: &ColumnType,
) -> Result<BindValue, String> {
    match op {
        Operand::Lit(lit) => lit_to_bind(lit),
        Operand::Claim(c) => {
            let raw = session
                .get(&c.header)
                .or_else(|| session.get(&c.header.to_ascii_lowercase()))
                .ok_or_else(|| format!("missing claim `{}`", c.header))?;
            parse_claim(raw, column_type)
        }
    }
}

fn lit_to_bind(lit: &LitValue) -> Result<BindValue, String> {
    Ok(match lit {
        LitValue::String(s) => BindValue::Text(s.clone()),
        LitValue::I64(i) => BindValue::I64(*i),
        LitValue::F64(f) => BindValue::F64(*f),
        LitValue::Bool(b) => BindValue::Bool(*b),
        LitValue::Json(j) => BindValue::Json(j.clone()),
        LitValue::Null => BindValue::Null,
    })
}

fn parse_claim(raw: &str, column_type: &ColumnType) -> Result<BindValue, String> {
    match column_type {
        ColumnType::Integer | ColumnType::UnsignedInteger => raw
            .parse::<i64>()
            .map(BindValue::I64)
            .map_err(|_| format!("claim value `{raw}` is not an integer")),
        ColumnType::Float => raw
            .parse::<f64>()
            .map(BindValue::F64)
            .map_err(|_| format!("claim value `{raw}` is not a float")),
        ColumnType::Boolean => match raw {
            "true" | "TRUE" | "1" => Ok(BindValue::Bool(true)),
            "false" | "FALSE" | "0" => Ok(BindValue::Bool(false)),
            _ => Err(format!("claim value `{raw}` is not a boolean")),
        },
        ColumnType::Json => Err("claims cannot compare to Json columns".into()),
        _ => Ok(BindValue::Text(raw.to_string())),
    }
}

pub(super) fn value_to_bind(v: &Value, column_type: &ColumnType) -> Result<BindValue, String> {
    match v {
        Value::Null => Ok(BindValue::Null),
        Value::Boolean(b) => Ok(BindValue::Bool(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(BindValue::I64(i))
            } else if let Some(f) = n.as_f64() {
                Ok(BindValue::F64(f))
            } else {
                Err("number out of range".into())
            }
        }
        Value::String(s) => match column_type {
            ColumnType::Bytes => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(s.as_bytes())
                    .map(BindValue::Bytes)
                    .map_err(|e| format!("invalid base64: {e}"))
            }
            ColumnType::Integer | ColumnType::UnsignedInteger => s
                .parse::<i64>()
                .map(BindValue::I64)
                .map_err(|_| format!("expected integer, got `{s}`")),
            _ => Ok(BindValue::Text(s.clone())),
        },
        Value::List(_) | Value::Object(_) | Value::Enum(_) => {
            let json = value_to_json(v)?;
            Ok(BindValue::Json(json))
        }
        _ => Err("unsupported GraphQL value for bind".into()),
    }
}

fn value_to_json(v: &Value) -> Result<JsonValue, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

#[cfg(test)]
mod parse_claim_tests {
    use super::*;

    #[test]
    fn integer_claim_ok_and_fail() {
        assert!(matches!(
            parse_claim("42", &ColumnType::Integer).unwrap(),
            BindValue::I64(42)
        ));
        assert!(parse_claim("nope", &ColumnType::Integer).is_err());
    }

    #[test]
    fn bool_claim_variants() {
        assert!(matches!(
            parse_claim("true", &ColumnType::Boolean).unwrap(),
            BindValue::Bool(true)
        ));
        assert!(matches!(
            parse_claim("0", &ColumnType::Boolean).unwrap(),
            BindValue::Bool(false)
        ));
        assert!(parse_claim("maybe", &ColumnType::Boolean).is_err());
    }

    #[test]
    fn json_claim_rejected() {
        assert!(parse_claim("{}", &ColumnType::Json).is_err());
    }
}
