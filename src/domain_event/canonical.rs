use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use super::DomainEventCaptureError;

/// Canonical domain-event body codec emitted by Distributed version one.
///
/// It is compact JSON with object keys recursively sorted by their UTF-8
/// strings. Array order and scalar values are preserved. This definition is
/// independent of aggregate replay's positional bitcode codec.
pub const DOMAIN_EVENT_BODY_CODEC: &str = "distributed-json";
/// Version of [`DOMAIN_EVENT_BODY_CODEC`].
pub const DOMAIN_EVENT_BODY_CODEC_VERSION: u16 = 1;
/// Maximum canonical domain-event body size, in bytes.
pub const MAX_DOMAIN_EVENT_BODY_BYTES: usize = 1024 * 1024;
/// Maximum accepted serialized occurrence size, including envelope and base64.
pub const MAX_DOMAIN_EVENT_OCCURRENCE_WIRE_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn canonical_json_bytes(
    value: &impl Serialize,
) -> Result<Vec<u8>, DomainEventCaptureError> {
    let value = serde_json::to_value(value)
        .map_err(|error| DomainEventCaptureError::BodyEncoding(error.to_string()))?;
    serde_json::to_vec(&canonical_json_value(value))
        .map_err(|error| DomainEventCaptureError::BodyEncoding(error.to_string()))
}

fn canonical_json_value(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonical_json_value).collect())
        }
        Value::Object(values) => {
            let sorted = values
                .into_iter()
                .map(|(key, value)| (key, canonical_json_value(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}
