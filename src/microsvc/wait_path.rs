//! Shared wait-path envelope for HTTP and gRPC command ingress.
//!
//! `{ commandId, input }` selects causal invoke. Identity comes from the
//! trusted transport (headers/metadata), never from the JSON body.

use serde::Deserialize;
use serde_json::{json, Value};

use super::session::Session;
use super::service::{CausalDispatchError, CausalDispatchResult, Service};
use crate::graphql::identity::VerifiedPrincipal;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WaitPathBody {
    command_id: String,
    #[serde(default)]
    input: Value,
}

/// Parse a wait-path body. `session_variables` / `roles` in JSON are ignored.
pub(crate) fn parse_wait_path_body(value: &Value) -> Option<(String, Value)> {
    let parsed: WaitPathBody = serde_json::from_value(value.clone()).ok()?;
    if parsed.command_id.trim().is_empty() {
        return None;
    }
    let input = if parsed.input.is_null() {
        json!({})
    } else {
        parsed.input
    };
    Some((parsed.command_id, input))
}

pub(crate) fn wait_path_response(result: &CausalDispatchResult) -> Value {
    json!({
        "payload": result.payload(),
        "receipt": {
            "commandId": result.command_id(),
            "causationId": result.causation_id(),
            "state": result.state(),
        }
    })
}

pub(crate) async fn dispatch_wait_path(
    service: &Service,
    command: &str,
    command_id: &str,
    input: Value,
    session: Session,
) -> Result<CausalDispatchResult, CausalDispatchError> {
    let subject = session
        .user_id()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CausalDispatchError::Rejected {
                code: "UNAUTHORIZED",
                status: 401,
                message: "durable commands require a verified transport identity".into(),
            }
        })?;
    let principal = VerifiedPrincipal::from_trusted_transport(subject);
    service
        .dispatch_causal_with_receipt(command, command_id, input, session, principal)
        .await
}
