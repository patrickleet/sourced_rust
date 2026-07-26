use std::collections::HashMap;

use serde_json::Value;

/// An inbound command request.
///
/// Generic command envelope used by in-process dispatch and adapters that
/// already decoded a gateway payload. Example shape:
/// ```json
/// {
///   "command": "order.create",
///   "input": { "product_id": "SKU-1" },
///   "session_variables": { "x-user-id": "user-42" }
/// }
/// ```
///
/// `session_variables` keys are deployment convention (see [`Session`]). A
/// query-layer action (Hasura, custom BFF, …) can map its native claims into
/// these variables before calling `dispatch_request`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandRequest {
    /// Command name (URL path, action name, or explicit field).
    pub command: String,
    /// JSON input payload.
    pub input: Value,
    /// Opaque session variables (identity claims, roles, tenant, etc.).
    pub session_variables: HashMap<String, String>,
}

/// Response from dispatching a command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandResponse {
    /// HTTP-style status code.
    pub status: u16,
    /// Response body (handler result or error).
    pub body: Value,
}
