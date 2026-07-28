use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::ProjectionProgramError;

const PROGRAM_DIGEST_DOMAIN: &[u8] = b"distributed.projection-program/v1\0";
const KEY_ENCODING_DOMAIN: &[u8] = b"distributed.projection-key/v1\0";
const PARTITION_ENCODING_DOMAIN: &[u8] = b"distributed.projection-partition/v1\0";

pub(crate) fn canonical_json_bytes(
    value: &impl Serialize,
) -> Result<Vec<u8>, ProjectionProgramError> {
    let value = serde_json::to_value(value)
        .map_err(|error| ProjectionProgramError::CanonicalJson(error.to_string()))?;
    serde_json::to_vec(&sort_value(value))
        .map_err(|error| ProjectionProgramError::CanonicalJson(error.to_string()))
}

pub(crate) fn digest_program(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(PROGRAM_DIGEST_DOMAIN);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    digest.finalize().into()
}

pub(crate) fn bounded_key_bytes(
    value: &impl Serialize,
    max: usize,
) -> Result<Vec<u8>, ProjectionProgramError> {
    bounded_value_bytes(KEY_ENCODING_DOMAIN, "projection key", value, max)
}

pub(crate) fn bounded_partition_bytes(
    value: &impl Serialize,
    max: usize,
) -> Result<Vec<u8>, ProjectionProgramError> {
    bounded_value_bytes(
        PARTITION_ENCODING_DOMAIN,
        "projection partition",
        value,
        max,
    )
}

fn bounded_value_bytes(
    domain: &[u8],
    kind: &'static str,
    value: &impl Serialize,
    max: usize,
) -> Result<Vec<u8>, ProjectionProgramError> {
    let canonical = canonical_json_bytes(value)?;
    let mut encoded = Vec::with_capacity(domain.len() + 8 + canonical.len());
    encoded.extend_from_slice(domain);
    encoded.extend_from_slice(&(canonical.len() as u64).to_be_bytes());
    encoded.extend_from_slice(&canonical);
    if encoded.len() > max {
        return Err(ProjectionProgramError::ValueTooLarge {
            kind,
            len: encoded.len(),
            max,
        });
    }
    Ok(encoded)
}

fn sort_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted = map
                .into_iter()
                .map(|(key, value)| (key, sort_value(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => Value::Array(values.into_iter().map(sort_value).collect()),
        other => other,
    }
}
