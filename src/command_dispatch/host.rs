//! Causal wait-path host used by GraphQL. Local in-process or HTTP loopback.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::graphql::identity::VerifiedPrincipal;
use crate::graphql::protocol::ProtocolResponseAccumulator;
use crate::microsvc::cell_host::{CELL_PRINCIPAL_PARTITION_HEADER, CELL_SERVICE_ID_HEADER};
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
                    .dispatch_causal_with_receipt(command, command_id, input, session, principal)
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
                    .causal_command_status_with_protocol(command_id, session, principal, protocol)
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
#[derive(Clone)]
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

    /// Same connection pool, different wait-path base (`{celld}/{shard}`).
    /// `Client::new()` loads TLS roots; do not construct one per command.
    pub fn retarget(&self, base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            client: self.client.clone(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// POST `{base}/{path}` with a JSON body (cell `outbox.complete`, alarms).
    pub async fn post_json(
        &self,
        path: &str,
        body: Value,
    ) -> Result<(u16, Value), CausalDispatchError> {
        let response = self
            .client
            .post(format!("{}/{path}", self.base))
            .json(&body)
            .send()
            .await
            .map_err(|err| CausalDispatchError::Internal(format!("cell HTTP failed: {err}")))?;
        let status = response.status().as_u16();
        let body: Value = response
            .json()
            .await
            .map_err(|err| CausalDispatchError::Internal(format!("cell HTTP body: {err}")))?;
        Ok((status, body))
    }

    /// POST `{base}/{command}` and return status + JSON, including 4xx with
    /// cell `outbox` for drain retries.
    pub async fn post_wait_path(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: &Session,
    ) -> Result<(u16, Value), CausalDispatchError> {
        self.post_wait_path_inner(command, command_id, input, session, None)
            .await
    }

    /// POST a cell wait-path command with identity derived by the verified
    /// GraphQL host. These headers are part of the trusted internal boundary,
    /// not values copied from public request headers or command input.
    pub async fn post_cell_wait_path(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: &Session,
        service_id: &str,
        principal_partition: &str,
    ) -> Result<(u16, Value), CausalDispatchError> {
        self.post_wait_path_inner(
            command,
            command_id,
            input,
            session,
            Some((service_id, principal_partition)),
        )
        .await
    }

    async fn post_wait_path_inner(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: &Session,
        cell_identity: Option<(&str, &str)>,
    ) -> Result<(u16, Value), CausalDispatchError> {
        let mut request =
            self.client
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
        if let Some((service_id, principal_partition)) = cell_identity {
            request = request
                .header(CELL_SERVICE_ID_HEADER, service_id)
                .header(CELL_PRINCIPAL_PARTITION_HEADER, principal_partition);
        }
        let response = request.send().await.map_err(|err| {
            CausalDispatchError::Internal(format!("wait-path HTTP failed: {err}"))
        })?;
        let status = response.status().as_u16();
        let body: Value = response
            .json()
            .await
            .map_err(|err| CausalDispatchError::Internal(format!("wait-path HTTP body: {err}")))?;
        Ok((status, body))
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
        let (status, body) = self
            .post_wait_path(command, command_id, input, &session)
            .await?;
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

/// Local dispatcher is a causal [`CommandHost`]. GraphQL must use this
/// trait object, not [`LocalCommandDispatcher::service`].
#[async_trait]
impl CommandHost for super::LocalCommandDispatcher {
    async fn invoke(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalDispatchResult, CausalDispatchError> {
        LocalCommandHost::new(Arc::clone(self.service()))
            .invoke(command, command_id, input, session, principal, protocol)
            .await
    }

    async fn status(
        &self,
        command_id: &str,
        session: &Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalCommandPublicStatus, CausalDispatchError> {
        LocalCommandHost::new(Arc::clone(self.service()))
            .status(command_id, session, principal, protocol)
            .await
    }
}
