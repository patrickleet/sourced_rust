//! Canonical encoding and digests for mutation programs.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::MutationProgramError;

const PROGRAM_DIGEST_DOMAIN: &[u8] = b"distributed.mutation-program/v1\0";

pub(crate) fn canonical_json_bytes(
    value: &impl Serialize,
) -> Result<Vec<u8>, MutationProgramError> {
    let value = serde_json::to_value(value)
        .map_err(|error| MutationProgramError::CanonicalJson(error.to_string()))?;
    serde_json::to_vec(&sort_value(value))
        .map_err(|error| MutationProgramError::CanonicalJson(error.to_string()))
}

pub(crate) fn digest_program(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(PROGRAM_DIGEST_DOMAIN);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    digest.finalize().into()
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
