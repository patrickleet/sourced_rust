//! Versioned command dispatch envelope shared by local and remote adapters.
//!
//! Wire fields beyond the existing [`crate::microsvc::CommandRequest`] are
//! additive and match the task-20 approved remote profile.

use crate::microsvc::{CommandRequest, CommandResponse};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Envelope schema version for local/remote semantic equality.
pub const COMMAND_DISPATCH_ENVELOPE_VERSION: u32 = 1;

/// Versioned dispatch envelope. Local and remote adapters share this shape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDispatchEnvelope {
    pub version: u32,
    pub command: String,
    pub input: serde_json::Value,
    /// Verified identity claims reconstructed at the writer boundary.
    #[serde(default)]
    pub session_variables: BTreeMap<String, String>,
    /// Stable command contract fingerprint when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_fingerprint: Option<String>,
    /// Client-supplied idempotency key for ledger dedup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// Causation identifier linking related work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    /// Deadline as unix millis; remote adapters must fail closed when exceeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_unix_ms: Option<u64>,
}

impl CommandDispatchEnvelope {
    pub fn from_request(request: &CommandRequest) -> Self {
        let mut session_variables = BTreeMap::new();
        for (key, value) in &request.session_variables {
            session_variables.insert(key.clone(), value.clone());
        }
        Self {
            version: COMMAND_DISPATCH_ENVELOPE_VERSION,
            command: request.command.clone(),
            input: request.input.clone(),
            session_variables,
            command_fingerprint: None,
            idempotency_key: None,
            causation_id: None,
            deadline_unix_ms: None,
        }
    }

    pub fn into_request(self) -> Result<CommandRequest, String> {
        if self.version != COMMAND_DISPATCH_ENVELOPE_VERSION {
            return Err(format!(
                "unsupported command dispatch envelope version {}",
                self.version
            ));
        }
        Ok(CommandRequest {
            command: self.command,
            input: self.input,
            session_variables: self.session_variables.into_iter().collect(),
        })
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// Durable receipt returned alongside a successful or rejected dispatch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDispatchReceipt {
    pub command: String,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_id: Option<String>,
}

impl CommandDispatchReceipt {
    pub fn from_response(command: &str, response: &CommandResponse) -> Self {
        Self {
            command: command.to_string(),
            status: response.status,
            causation_id: None,
            idempotency_key: None,
            ledger_id: None,
        }
    }
}
