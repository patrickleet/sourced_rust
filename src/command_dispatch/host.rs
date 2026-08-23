//! Causal wait-path host used by GraphQL. Local in-process or HTTP loopback.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::bus::{Message, MessagePublisher};
use crate::graphql::identity::VerifiedPrincipal;
use crate::graphql::protocol::ProtocolResponseAccumulator;
use crate::microsvc::{
    CausalCommandPublicStatus, CausalDispatchError, CausalDispatchResult, Service, Session,
    ROLE_KEY, USER_ID_KEY,
};
use crate::OutboxMessage;

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

    /// POST `{base}/{command}` and return status + JSON, including 4xx with
    /// cell `outbox` for drain retries.
    pub async fn post_wait_path(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: &Session,
    ) -> Result<(u16, Value), CausalDispatchError> {
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
        let body: Value = response
            .json()
            .await
            .map_err(|err| CausalDispatchError::Internal(format!("wait-path HTTP body: {err}")))?;
        Ok((status, body))
    }

    /// SOA `outbox.complete`: mark cell SQLite rows Published after
    /// [`MessagePublisher::publish`] returns `Ok`. Fire-and-forget — do not
    /// await this before returning the mutation.
    pub fn complete_outbox_later(&self, ids: impl IntoIterator<Item = impl Into<String>>) {
        let ids: Vec<String> = ids.into_iter().map(Into::into).collect();
        if ids.is_empty() {
            return;
        }
        let client = self.client.clone();
        let url = format!("{}/outbox.complete", self.base);
        tokio::spawn(async move {
            let _ = client
                .post(&url)
                .json(&serde_json::json!({ "ids": ids }))
                .send()
                .await;
        });
    }

    /// Publish pending cell outbox through the process bus. On `Ok`, spawn
    /// `outbox.complete` and return — the mutation must not wait on the DO
    /// update. Publish `Err` retries in-process; the cell SQLite is still the
    /// durable row (no second SQL).
    pub async fn drain_cell_outbox<P>(&self, publisher: &P, rows: &[OutboxMessage])
    where
        P: MessagePublisher + Clone + Send + Sync + 'static,
    {
        let mut published = Vec::new();
        for row in rows {
            if publisher
                .publish(Message::from(row.clone()))
                .await
                .is_ok()
            {
                published.push(row.id.clone());
                continue;
            }
            let publisher = publisher.clone();
            let complete = self.clone();
            let row = row.clone();
            tokio::spawn(async move {
                for backoff_ms in [50_u64, 100, 200, 400, 800, 1600, 3200] {
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    if publisher.publish(Message::from(row.clone())).await.is_ok() {
                        complete.complete_outbox_later(std::iter::once(row.id.clone()));
                        return;
                    }
                }
                eprintln!(
                    "cell outbox: bus publish still failing for {}; cell SQLite still has the row",
                    row.id
                );
            });
        }
        self.complete_outbox_later(published);
    }

    /// Extra drainer (SOA `spawn_outbox_publish_loop` analogue): every 5s,
    /// `POST {base}/{kind}/{id}/outbox.drain` for cells this process has seen
    /// and re-publish still-Pending rows.
    pub fn spawn_outbox_drain_loop<P>(
        &self,
        kind: &'static str,
        publisher: P,
        pending: Arc<Mutex<HashSet<String>>>,
    ) where
        P: MessagePublisher + Clone + Send + Sync + 'static,
    {
        let http = self.clone();
        let celld_url = self.base.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let ids: Vec<String> = match pending.lock() {
                    Ok(guard) => guard.iter().cloned().collect(),
                    Err(_) => continue,
                };
                for id in ids {
                    let shard = http.retarget(format!("{celld_url}/{kind}/{id}"));
                    let Ok((_, body)) = shard
                        .post_wait_path(
                            "outbox.drain",
                            "drain",
                            serde_json::json!({}),
                            &Session::new(),
                        )
                        .await
                    else {
                        continue;
                    };
                    let rows = CausalDispatchResult::outbox_from_wait_path(&body);
                    if rows.is_empty() {
                        if let Ok(mut guard) = pending.lock() {
                            guard.remove(&id);
                        }
                        continue;
                    }
                    shard.drain_cell_outbox(&publisher, &rows).await;
                }
            }
        });
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
