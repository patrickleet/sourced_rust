//! GraphQL [`CommandHost`] for celld wait-path + shared cell outbox drain.
//!
//! Aggregate crates supply a [`CelldRoute`] (kind, shard, payload map). Outbox
//! publish, complete, extra drain, and wait-path protocol seal are the same
//! for every cell.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use super::outbox::{drain_cell_outbox, spawn_cell_outbox_drain_loop};
use crate::bus::MessagePublisher;
use crate::command_dispatch::{CommandHost, HttpCommandHost, LocalCommandHost};
use crate::graphql::identity::VerifiedPrincipal;
use crate::graphql::protocol::ProtocolResponseAccumulator;
use crate::microsvc::{
    CausalCommandPublicStatus, CausalDispatchError, CausalDispatchResult, Service, Session,
};

const COMPLETED_STATUS_CACHE_LIMIT: usize = 4_096;

/// One aggregate's cell wait-path: command names, URL kind, shard id, payload.
#[derive(Clone, Copy)]
pub struct CelldRoute {
    pub commands: &'static [&'static str],
    /// Path segment `{CELLD_URL}/{kind}/{shard}/{command}`.
    pub kind: &'static str,
    pub shard: fn(&Value) -> Option<String>,
    pub payload: fn(command: &str, input: &Value, remote: &Value, session: &Session) -> Value,
}

impl CelldRoute {
    pub const fn new(
        commands: &'static [&'static str],
        kind: &'static str,
        shard: fn(&Value) -> Option<String>,
        payload: fn(command: &str, input: &Value, remote: &Value, session: &Session) -> Value,
    ) -> Self {
        Self {
            commands,
            kind,
            shard,
            payload,
        }
    }
}

/// Routes selected commands to celld; everything else stays on [`LocalCommandHost`].
pub struct CelldCommandHost<P> {
    celld_url: String,
    http: HttpCommandHost,
    publisher: P,
    local: LocalCommandHost,
    routes: Vec<CelldRoute>,
    pending: Arc<Mutex<HashSet<(String, String)>>>,
    completed: Arc<Mutex<HashMap<(String, String), CausalCommandPublicStatus>>>,
}

impl<P> CelldCommandHost<P>
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    pub fn new(celld_url: impl Into<String>, service: Arc<Service>, publisher: P) -> Self {
        let celld_url = celld_url.into().trim_end_matches('/').to_string();
        let http = HttpCommandHost::new(&celld_url);
        let pending = Arc::new(Mutex::new(HashSet::new()));
        spawn_cell_outbox_drain_loop(http.clone(), publisher.clone(), Arc::clone(&pending));
        Self {
            http,
            celld_url,
            publisher,
            local: LocalCommandHost::new(service),
            routes: Vec::new(),
            pending,
            completed: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn route(mut self, route: CelldRoute) -> Self {
        self.routes.push(route);
        self
    }

    fn route_for(&self, command: &str) -> Option<&CelldRoute> {
        self.routes
            .iter()
            .find(|route| route.commands.contains(&command))
    }

    fn service_id(&self) -> Result<&str, CausalDispatchError> {
        self.local.service().name().ok_or_else(|| {
            CausalDispatchError::Internal(
                "celld command host requires a named executable service".into(),
            )
        })
    }

    fn remember_completed(&self, key: (String, String), status: CausalCommandPublicStatus) {
        let Ok(mut completed) = self.completed.lock() else {
            return;
        };
        if completed.len() >= COMPLETED_STATUS_CACHE_LIMIT && !completed.contains_key(&key) {
            if let Some(evicted) = completed.keys().next().cloned() {
                completed.remove(&evicted);
            }
        }
        completed.insert(key, status);
    }
}

fn remote_dispatch_error(status: u16, body: &Value) -> CausalDispatchError {
    let message = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("wait-path rejected")
        .to_string();
    match body.get("code").and_then(Value::as_str) {
        Some("BAD_REQUEST") => CausalDispatchError::BadRequest(message),
        Some("FORBIDDEN") => CausalDispatchError::Forbidden,
        Some("COMMAND_ID_REUSE") => CausalDispatchError::CommandIdReuse,
        Some("COMMAND_IN_PROGRESS") => CausalDispatchError::InProgress,
        Some("COMMAND_EXPIRED") => CausalDispatchError::Expired,
        Some("INTERNAL") => {
            CausalDispatchError::Internal(format!("cell wait-path failed with HTTP {status}"))
        }
        Some("UNAUTHORIZED") => CausalDispatchError::Rejected {
            code: "UNAUTHORIZED",
            status,
            message,
        },
        Some("NOT_FOUND") => CausalDispatchError::Rejected {
            code: "NOT_FOUND",
            status,
            message,
        },
        _ => CausalDispatchError::Rejected {
            code: "REJECTED",
            status,
            message,
        },
    }
}

#[async_trait]
impl<P> CommandHost for CelldCommandHost<P>
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    async fn invoke(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalDispatchResult, CausalDispatchError> {
        let Some(route) = self.route_for(command).copied() else {
            return self
                .local
                .invoke(command, command_id, input, session, principal, protocol)
                .await;
        };
        let shard = (route.shard)(&input)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CausalDispatchError::BadRequest(format!(
                    "{} id required for celld wait-path",
                    route.kind
                ))
            })?;
        let service_id = self.service_id()?.to_string();
        let principal_partition = principal.partition_for_service(&service_id);
        let http = self
            .http
            .retarget(format!("{}/{}/{}", self.celld_url, route.kind, shard));
        let (status, body) = http
            .post_cell_wait_path(
                command,
                command_id,
                input.clone(),
                &session,
                &service_id,
                &principal_partition,
            )
            .await?;
        let outbox = CausalDispatchResult::outbox_from_wait_path(&body);
        if !outbox.is_empty() {
            if let Ok(mut guard) = self.pending.lock() {
                guard.insert((route.kind.to_string(), shard));
            }
        }
        drain_cell_outbox(&http, &self.publisher, &outbox).await;
        if status >= 400 {
            return Err(remote_dispatch_error(status, &body));
        }
        let remote = CausalDispatchResult::from_wait_path_wire(body)
            .map_err(|error| CausalDispatchError::Internal(format!("wait-path decode: {error}")))?;
        let payload = (route.payload)(command, &input, remote.payload(), &session);
        let mut remote = remote.with_payload(payload);
        if let Some(protocol) = protocol {
            remote = self
                .local
                .service()
                .seal_wait_path_dispatch(command, &protocol, remote)?;
        }
        self.remember_completed(
            (principal_partition, command_id.to_string()),
            remote.public_status(),
        );
        Ok(remote)
    }

    async fn status(
        &self,
        command_id: &str,
        session: &Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalCommandPublicStatus, CausalDispatchError> {
        let service_id = self.service_id()?;
        let principal_partition = principal.partition_for_service(service_id);
        let key = (principal_partition, command_id.to_string());
        if let Some(status) = self
            .completed
            .lock()
            .ok()
            .and_then(|guard| guard.get(&key).cloned())
        {
            return Ok(status);
        }
        self.local
            .status(command_id, session, principal, protocol)
            .await
    }
}
