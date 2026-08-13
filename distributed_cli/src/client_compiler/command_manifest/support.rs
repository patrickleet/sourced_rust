use std::collections::BTreeSet;

use base64::Engine as _;
use serde_json::Value as JsonValue;

use crate::client_compiler::manifest::{ManifestCommand, ManifestField};
use crate::client_compiler::ClientCompileError;

pub(super) fn constant_matches(value: &JsonValue, expected: &ManifestField) -> bool {
    if expected.scalar == "JSON" {
        return true;
    }
    match (expected.scalar.as_str(), value) {
        ("Boolean", JsonValue::Bool(_)) => true,
        ("BigInt", JsonValue::Number(number)) => number.is_i64() || number.is_u64(),
        ("Int", JsonValue::Number(number)) => {
            number
                .as_i64()
                .is_some_and(|value| i32::try_from(value).is_ok())
                || number
                    .as_u64()
                    .is_some_and(|value| i32::try_from(value).is_ok())
        }
        ("Float", JsonValue::Number(_)) => true,
        ("String" | "ID", JsonValue::String(_)) => true,
        ("Timestamptz", JsonValue::String(value)) => is_rfc3339(value),
        ("Bytea", JsonValue::String(value)) => base64::engine::general_purpose::STANDARD
            .decode(value)
            .is_ok(),
        _ => false,
    }
}

fn is_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        std::str::from_utf8(bytes.get(range)?)
            .ok()?
            .parse::<u32>()
            .ok()
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0..4),
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if day == 0 || day > max_day || hour > 23 || minute > 59 || second > 60 {
        return false;
    }
    let mut cursor = 19;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == start {
            return false;
        }
    }
    match bytes.get(cursor) {
        Some(b'Z' | b'z') => cursor + 1 == bytes.len(),
        Some(b'+' | b'-') if cursor + 6 == bytes.len() && bytes.get(cursor + 3) == Some(&b':') => {
            matches!(
                (
                    number(cursor + 1..cursor + 3),
                    number(cursor + 4..cursor + 6)
                ),
                (Some(0..=23), Some(0..=59))
            )
        }
        _ => false,
    }
}

pub(super) fn graphql_name(value: &str, label: &str) -> Result<(), ClientCompileError> {
    if !crate::client_compiler::is_graphql_name(value) || value.starts_with("__") {
        Err(invalid(
            "client.manifest.graphql_name",
            format!("{label} `{value}` must be a valid GraphQL name"),
        ))
    } else {
        Ok(())
    }
}

pub(super) fn unique_nonempty(values: &[String], label: &str) -> Result<(), ClientCompileError> {
    let mut seen = BTreeSet::new();
    for value in values {
        nonempty(value, label)?;
        if !seen.insert(value) {
            return Err(invalid(
                "client.manifest.duplicate_entry",
                format!("{label} entries must be unique"),
            ));
        }
    }
    Ok(())
}

pub(super) fn nonempty(value: &str, label: &str) -> Result<(), ClientCompileError> {
    if value.trim().is_empty() {
        Err(invalid(
            "client.manifest.empty",
            format!("{label} must not be empty"),
        ))
    } else {
        Ok(())
    }
}

pub(super) fn command_error(
    command: &ManifestCommand,
    code: &'static str,
    message: impl std::fmt::Display,
) -> ClientCompileError {
    invalid(
        code,
        format!("manifest command `{}` {message}", command.name),
    )
}

pub(super) fn invalid(code: &'static str, message: impl Into<String>) -> ClientCompileError {
    ClientCompileError::manifest(code, message)
}
