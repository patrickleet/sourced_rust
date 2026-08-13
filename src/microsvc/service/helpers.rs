use std::collections::HashMap;

use serde_json::Value;

use super::routes::HandlerSpec;
use crate::bus::{Message, MessageKind};
use crate::microsvc::error::HandlerError;
use crate::microsvc::session::Session;

pub(super) fn names_by_kind(specs: &[HandlerSpec], kind: MessageKind) -> Vec<&str> {
    let mut names = Vec::new();

    for spec in specs.iter().filter(|spec| spec.kind == kind) {
        for name in spec.names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }

    names
}

/// Whether a content type declares a JSON payload (`application/json` or any
/// `+json` structured suffix), ignoring parameters like `;charset=utf-8`.
pub(super) fn is_json_content_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    essence == "application/json" || essence.ends_with("+json")
}

#[cfg(feature = "otel")]
pub(super) fn microsvc_dispatch_span(message: &Message) -> tracing::Span {
    crate::telemetry::microsvc_dispatch_span(message)
}

#[cfg(feature = "otel")]
pub(super) fn microsvc_handler_span(message: &Message) -> tracing::Span {
    crate::telemetry::microsvc_handler_span(message)
}

pub(super) fn message_to_json_input(message: &Message) -> Result<Value, HandlerError> {
    serde_json::from_slice::<Value>(&message.payload).map_err(|e| {
        HandlerError::DecodeFailed(format!(
            "invalid JSON payload for message '{}': {}",
            message.name, e
        ))
    })
}

pub(super) fn message_to_session(message: &Message) -> Session {
    let vars: HashMap<String, String> = message
        .metadata
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.clone()))
        .collect();
    Session::from_map(vars)
}
