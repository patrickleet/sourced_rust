//! Framework-agnostic HTTP request/response types for domain service dispatch.
//!
//! These types bridge HTTP frameworks (axum, actix-web, etc.) and the
//! `DomainService` command handler. The library stays framework-agnostic —
//! the actual HTTP server is wired by the consumer.

use serde::{Deserialize, Serialize};

use super::error::HandlerError;

/// An inbound command request (typically deserialized from an HTTP POST body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRequest {
    /// Optional client-supplied ID (auto-generated if omitted).
    #[serde(default)]
    pub id: Option<String>,
    /// Command name — maps to a registered handler.
    pub command: String,
    /// JSON payload forwarded to the handler.
    pub payload: serde_json::Value,
}

/// The response returned after dispatching a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResponse {
    /// HTTP-style status code.
    pub status: u16,
    /// Response body (success detail or error message).
    pub body: serde_json::Value,
}

impl CommandResponse {
    /// Build a success (200) response.
    pub fn ok() -> Self {
        Self {
            status: 200,
            body: serde_json::json!({ "ok": true }),
        }
    }

    /// Build an error response from a `HandlerError`.
    pub fn from_error(err: HandlerError) -> Self {
        let (status, message) = match &err {
            HandlerError::UnknownCommand(_) => (404, err.to_string()),
            HandlerError::DecodeFailed(_) => (400, err.to_string()),
            HandlerError::Rejected(_) => (422, err.to_string()),
            HandlerError::NotFound(_) => (404, err.to_string()),
            HandlerError::Repository(_) => (500, err.to_string()),
            HandlerError::Publish(_) => (502, err.to_string()),
            HandlerError::Other(_) => (500, err.to_string()),
        };
        Self {
            status,
            body: serde_json::json!({ "error": message }),
        }
    }
}

impl From<HandlerError> for CommandResponse {
    fn from(err: HandlerError) -> Self {
        Self::from_error(err)
    }
}
