//! Remote command dispatch adapter (task 20 approved profile).
//!
//! Transport, trust, credential, and replay rules are fixed by
//! [`APPROVED_REMOTE_DISPATCH_PROFILE`]. This module implements only that
//! approved contract — it does not invent alternate security modes.

use super::envelope::{CommandDispatchEnvelope, COMMAND_DISPATCH_ENVELOPE_VERSION};
use super::{CommandDispatchError, CommandDispatcher};
use crate::microsvc::{CommandRequest, CommandResponse};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Stable identifier for the single production remote profile approved by
/// task 20. Implementation and tests must cite this constant.
pub const APPROVED_REMOTE_DISPATCH_PROFILE: &str =
    "distributed.command_dispatch.remote.v1.https_mtls_service_identity";

/// Trust mode fixed by the approved remote profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTrustMode {
    /// Mutual TLS with workload/service identity (approved production mode).
    MutualTlsServiceIdentity,
}

/// Configuration for the approved remote adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteDispatchConfig {
    /// Logical destination id from the deployment plan topology.
    pub destination: String,
    /// Absolute HTTPS endpoint for the writer process.
    pub endpoint: String,
    /// Trust mode — only the approved production mode is accepted.
    pub trust: RemoteTrustMode,
    /// Maximum request body bytes.
    pub max_body_bytes: usize,
    /// Request timeout.
    pub timeout_ms: u64,
}

impl RemoteDispatchConfig {
    pub fn validate(&self) -> Result<(), CommandDispatchError> {
        if self.destination.trim().is_empty() {
            return Err(CommandDispatchError::Unroutable(
                "remote destination must not be empty".into(),
            ));
        }
        if !self.endpoint.starts_with("https://") {
            return Err(CommandDispatchError::Rejected(
                "approved remote profile requires https endpoints".into(),
            ));
        }
        if !matches!(self.trust, RemoteTrustMode::MutualTlsServiceIdentity) {
            return Err(CommandDispatchError::Rejected(
                "unsupported remote trust mode".into(),
            ));
        }
        if self.max_body_bytes == 0 || self.timeout_ms == 0 {
            return Err(CommandDispatchError::Rejected(
                "remote max_body_bytes and timeout_ms must be positive".into(),
            ));
        }
        Ok(())
    }
}

/// HTTP client abstraction so tests can loopback without a network.
#[async_trait]
pub trait RemoteCommandTransport: Send + Sync {
    async fn post_json(
        &self,
        endpoint: &str,
        body: &[u8],
        headers: BTreeMap<String, String>,
    ) -> Result<(u16, Vec<u8>), CommandDispatchError>;
}

/// Remote dispatcher that encodes the approved envelope and posts it.
pub struct RemoteCommandDispatcher {
    config: RemoteDispatchConfig,
    transport: Arc<dyn RemoteCommandTransport>,
}

impl RemoteCommandDispatcher {
    pub fn new(
        config: RemoteDispatchConfig,
        transport: Arc<dyn RemoteCommandTransport>,
    ) -> Result<Self, CommandDispatchError> {
        config.validate()?;
        Ok(Self { config, transport })
    }

    pub fn config(&self) -> &RemoteDispatchConfig {
        &self.config
    }

    pub fn profile(&self) -> &'static str {
        APPROVED_REMOTE_DISPATCH_PROFILE
    }
}

#[async_trait]
impl CommandDispatcher for RemoteCommandDispatcher {
    async fn dispatch(
        &self,
        request: &CommandRequest,
    ) -> Result<CommandResponse, CommandDispatchError> {
        let mut envelope = CommandDispatchEnvelope::from_request(request);
        envelope.version = COMMAND_DISPATCH_ENVELOPE_VERSION;
        // Never accept caller-supplied roles as trusted identity across the wire.
        // The approved profile requires the writer to reconstruct identity from
        // mTLS service identity + framework adapters. Session variables that
        // look like forwarded roles are stripped here.
        envelope.session_variables.retain(|key, _| {
            let lower = key.to_ascii_lowercase();
            !(lower == "x-roles" || lower == "roles" || lower.ends_with("-roles"))
        });

        let body = envelope
            .canonical_bytes()
            .map_err(|error| CommandDispatchError::Rejected(error.to_string()))?;
        if body.len() > self.config.max_body_bytes {
            return Err(CommandDispatchError::Rejected(format!(
                "remote command body exceeds max_body_bytes {}",
                self.config.max_body_bytes
            )));
        }

        if let Some(deadline) = envelope.deadline_unix_ms {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_millis() as u64;
            if now > deadline {
                return Err(CommandDispatchError::DeadlineExceeded);
            }
        }

        let mut headers = BTreeMap::new();
        headers.insert(
            "content-type".into(),
            "application/json".into(),
        );
        headers.insert(
            "x-distributed-dispatch-profile".into(),
            APPROVED_REMOTE_DISPATCH_PROFILE.into(),
        );
        headers.insert(
            "x-distributed-destination".into(),
            self.config.destination.clone(),
        );

        let (status, response_body) = self
            .transport
            .post_json(&self.config.endpoint, &body, headers)
            .await?;
        if !(200..300).contains(&status) {
            return Err(CommandDispatchError::Transport(format!(
                "remote writer returned status {status}"
            )));
        }
        let response: CommandResponse = serde_json::from_slice(&response_body).map_err(|error| {
            CommandDispatchError::Transport(format!(
                "remote writer returned invalid response: {error}"
            ))
        })?;
        Ok(response)
    }

    fn kind(&self) -> &'static str {
        "remote"
    }
}

/// In-memory loopback transport for parity tests.
#[allow(dead_code)]
pub struct LoopbackRemoteTransport {
    handler: Arc<dyn Fn(CommandRequest) -> CommandResponse + Send + Sync>,
}

#[allow(dead_code)]
impl LoopbackRemoteTransport {
    pub fn new(
        handler: impl Fn(CommandRequest) -> CommandResponse + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Arc::new(handler),
        }
    }
}

#[async_trait]
impl RemoteCommandTransport for LoopbackRemoteTransport {
    async fn post_json(
        &self,
        _endpoint: &str,
        body: &[u8],
        _headers: BTreeMap<String, String>,
    ) -> Result<(u16, Vec<u8>), CommandDispatchError> {
        let envelope: CommandDispatchEnvelope = serde_json::from_slice(body).map_err(|error| {
            CommandDispatchError::Rejected(format!("invalid remote envelope: {error}"))
        })?;
        let request = envelope
            .into_request()
            .map_err(CommandDispatchError::Rejected)?;
        let response = (self.handler)(request);
        let bytes = serde_json::to_vec(&response).map_err(|error| {
            CommandDispatchError::Transport(format!("encode loopback response: {error}"))
        })?;
        Ok((200, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[tokio::test]
    async fn remote_loopback_preserves_command_bytes_and_strips_forwarded_roles() {
        let transport = Arc::new(LoopbackRemoteTransport::new(|request| {
            assert!(!request.session_variables.contains_key("x-roles"));
            CommandResponse {
                status: 200,
                body: json!({ "ok": true, "command": request.command }),
            }
        }));
        let dispatcher = RemoteCommandDispatcher::new(
            RemoteDispatchConfig {
                destination: "todo-writer".into(),
                endpoint: "https://writer.example/commands".into(),
                trust: RemoteTrustMode::MutualTlsServiceIdentity,
                max_body_bytes: 64 * 1024,
                timeout_ms: 5_000,
            },
            transport,
        )
        .unwrap();
        assert_eq!(dispatcher.profile(), APPROVED_REMOTE_DISPATCH_PROFILE);

        let mut session = HashMap::new();
        session.insert("x-user-id".into(), "user-1".into());
        session.insert("x-roles".into(), "admin".into());
        let response = dispatcher
            .dispatch(&CommandRequest {
                command: "todo.create".into(),
                input: json!({ "title": "hi" }),
                session_variables: session,
            })
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body["command"], "todo.create");
    }

    #[test]
    fn remote_config_requires_https_and_approved_trust() {
        let err = RemoteDispatchConfig {
            destination: "w".into(),
            endpoint: "http://insecure".into(),
            trust: RemoteTrustMode::MutualTlsServiceIdentity,
            max_body_bytes: 1,
            timeout_ms: 1,
        }
        .validate()
        .unwrap_err();
        assert!(err.to_string().contains("https"));
    }
}
