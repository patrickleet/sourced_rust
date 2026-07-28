use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::constants::PROJECTION_BINDING_DIGEST_DOMAIN;

pub(crate) fn canonical_projection_topology_bytes(
    value: &impl Serialize,
) -> Result<Vec<u8>, serde_json::Error> {
    let value = serde_json::to_value(value)?;
    serde_json::to_vec(&sort_value(value))
}

pub(crate) fn digest_projection_binding(canonical: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(PROJECTION_BINDING_DIGEST_DOMAIN);
    digest.update((canonical.len() as u64).to_be_bytes());
    digest.update(canonical);
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
