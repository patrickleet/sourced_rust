//! Causal wait-path host used by GraphQL. Local in-process or HTTP loopback.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::graphql::identity::VerifiedPrincipal;
use crate::graphql::protocol::ProtocolResponseAccumulator;
use crate::microsvc::{
    CausalCommandPublicStatus, CausalDispatchError, CausalDispatchResult, Service, Session,
    ROLE_KEY, USER_ID_KEY,
};

/// Wait-path command host. GraphQL mutations call this instead of `Service`.
#[async_trait]
pub trait CommandHost: Send + Sync {
    async fn invoke(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalDispatchResult, CausalDispatchError>;

    async fn status(
        &self,
        command_id: &str,
        session: &Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalCommandPublicStatus, CausalDispatchError>;
}

pub type SharedCommandHost = Arc<dyn CommandHost>;

/// In-process host wrapping a writer [`Service`].
pub struct LocalCommandHost {
    service: Arc<Service>,
}

impl LocalCommandHost {
    pub fn new(service: Arc<Service>) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }
}

#[async_trait]
impl CommandHost for LocalCommandHost {
    async fn invoke(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalDispatchResult, CausalDispatchError> {
        match protocol {
            Some(protocol) => {
                self.service
                    .dispatch_causal_with_receipt_and_protocol(
                        command, command_id, input, session, principal, protocol,
                    )
                    .await
            }
            None => {
                self.service
                    .dispatch_causal_with_receipt(
                        command, command_id, input, session, principal,
                    )
                    .await
            }
        }
    }

    async fn status(
        &self,
        command_id: &str,
        session: &Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalCommandPublicStatus, CausalDispatchError> {
        match protocol {
            Some(protocol) => {
                self.service
                    .causal_command_status_with_protocol(
                        command_id, session, principal, protocol,
                    )
                    .await
            }
            None => {
                self.service
                    .causal_command_status(command_id, session, principal)
                    .await
            }
        }
    }
}

/// HTTP wait-path client (`POST {base}/{command}` with `{ commandId, input }`).
pub struct HttpCommandHost {
    base: String,
    client: reqwest::Client,
}

impl HttpCommandHost {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl CommandHost for HttpCommandHost {
    async fn invoke(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: Session,
        _principal: VerifiedPrincipal,
        _protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalDispatchResult, CausalDispatchError> {
        let mut request = self
            .client
            .post(format!("{}/{command}", self.base))
            .json(&serde_json::json!({
                "commandId": command_id,
                "input": input,
            }));
        if let Some(user) = session.user_id() {
            request = request.header(USER_ID_KEY, user);
        }
        if let Some(roles) = session.get(ROLE_KEY) {
            request = request.header(ROLE_KEY, roles);
        }
        let response = request.send().await.map_err(|err| {
            CausalDispatchError::Internal(format!("wait-path HTTP failed: {err}"))
        })?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.map_err(|err| {
            CausalDispatchError::Internal(format!("wait-path HTTP body: {err}"))
        })?;
        if status >= 400 {
            let message = body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("wait-path rejected")
                .to_string();
            return Err(CausalDispatchError::Rejected {
                code: "REJECTED",
                status,
                message,
            });
        }
        CausalDispatchResult::from_wait_path_wire(body)
    }

    async fn status(
        &self,
        command_id: &str,
        _session: &Session,
        _principal: VerifiedPrincipal,
        _protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalCommandPublicStatus, CausalDispatchError> {
        Ok(CausalCommandPublicStatus::unknown(command_id))
    }
}
