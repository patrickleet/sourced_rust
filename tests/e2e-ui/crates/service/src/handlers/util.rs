//! Shared handler helpers.

use distributed::bus::Message;
use distributed::microsvc::{HandlerError, Session};
use distributed::{BitcodePayloadCodec, PayloadCodec};
use serde::de::DeserializeOwned;

/// Decode event payload as JSON (tests) or bitcode (outbox → bus).
pub fn decode_payload<T: DeserializeOwned>(message: &Message) -> Result<T, HandlerError> {
    let ct = message.content_type.as_str();
    if ct.contains("json") || looks_like_json(message.payload()) {
        return serde_json::from_slice(message.payload())
            .map_err(|e| HandlerError::DecodeFailed(format!("json payload: {e}")));
    }
    BitcodePayloadCodec::decode(message.payload())
        .map_err(|e| HandlerError::DecodeFailed(format!("bitcode payload: {e}")))
}

fn looks_like_json(bytes: &[u8]) -> bool {
    matches!(
        bytes.iter().find(|b| !b.is_ascii_whitespace()),
        Some(b'{' | b'[')
    )
}

pub fn rejected(err: impl std::fmt::Display) -> HandlerError {
    HandlerError::Rejected(err.to_string())
}

pub fn read_model_error(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::Other(Box::new(std::io::Error::other(e.to_string())))
}

/// Authenticated user from session (`x-user-id` via DevHeaders or OIDC claim map).
pub fn require_user(session: &Session) -> Result<String, HandlerError> {
    session
        .user_id()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| HandlerError::Unauthorized("missing x-user-id".into()))
}

/// Require engine role `admin` (`x-role` / OIDC claim map).
pub fn require_admin(session: &Session) -> Result<(), HandlerError> {
    match session.role() {
        Some("admin") => Ok(()),
        Some(other) => Err(HandlerError::Rejected(format!(
            "admin role required, got `{other}`"
        ))),
        None => Err(HandlerError::Unauthorized("missing x-role".into())),
    }
}
