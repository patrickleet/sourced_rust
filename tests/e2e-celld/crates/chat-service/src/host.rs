//! Wait-path host: celld for `chat.post`. Cell outbox drains through [`MessagePublisher`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use distributed::bus::MessagePublisher;
use distributed::command_dispatch::{CommandHost, HttpCommandHost, LocalCommandHost};
use distributed::graphql::protocol::ProtocolResponseAccumulator;
use distributed::graphql::VerifiedPrincipal;
use distributed::microsvc::{
    CausalCommandPublicStatus, CausalDispatchError, CausalDispatchResult, Service, Session,
};
use serde_json::{json, Value};

/// Routes `chat.post` to `{CELLD_URL}/chat/{message_id}/chat.post`.
///
/// The cell commits events + outbox in one SQLite. This host publishes those
/// rows through the process [`MessagePublisher`] (NATS/Kafka/Rabbit — not a
/// second SQL outbox). After `publish` Ok, `outbox.complete` is spawned and
/// the mutation returns without waiting on the DO. A 5s drain loop re-reads
/// still-Pending rows via `outbox.drain`.
pub struct CelldChatCommandHost<P> {
    celld_url: String,
    http: HttpCommandHost,
    publisher: P,
    local: LocalCommandHost,
    pending: Arc<Mutex<HashSet<String>>>,
    completed: Arc<Mutex<HashMap<String, CausalCommandPublicStatus>>>,
}

impl<P> CelldChatCommandHost<P>
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    pub fn new(
        celld_url: impl Into<String>,
        service: Arc<Service>,
        publisher: P,
    ) -> Self {
        let celld_url = celld_url.into().trim_end_matches('/').to_string();
        let http = HttpCommandHost::new(&celld_url);
        let pending = Arc::new(Mutex::new(HashSet::new()));
        http.spawn_outbox_drain_loop("chat", publisher.clone(), Arc::clone(&pending));
        Self {
            http,
            celld_url,
            publisher,
            local: LocalCommandHost::new(service),
            pending,
            completed: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl<P> CommandHost for CelldChatCommandHost<P>
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
        if command != "chat.post" {
            return self
                .local
                .invoke(command, command_id, input, session, principal, protocol)
                .await;
        }
        let message_id = input
            .get("message_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CausalDispatchError::BadRequest("message_id required for celld wait-path".into())
            })?;
        let started = Instant::now();
        let http = self
            .http
            .retarget(format!("{}/chat/{message_id}", self.celld_url));
        let (status, body) = http
            .post_wait_path(command, command_id, input.clone(), &session)
            .await?;
        let cell_ms = started.elapsed().as_millis();
        let outbox = CausalDispatchResult::outbox_from_wait_path(&body);
        if !outbox.is_empty() {
            if let Ok(mut guard) = self.pending.lock() {
                guard.insert(message_id.to_string());
            }
        }
        http.drain_cell_outbox(&self.publisher, &outbox).await;
        eprintln!(
            "e2e-celld: chat.post cell={cell_ms}ms outbox={}",
            outbox.len()
        );
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
        let remote = CausalDispatchResult::from_wait_path_wire(body).map_err(|error| {
            CausalDispatchError::Internal(format!("wait-path decode: {error}"))
        })?;
        let payload = graphql_chat_payload(&input, remote.payload(), &session);
        let mut remote = remote.with_payload(payload);
        if let Some(protocol) = protocol {
            remote = self
                .local
                .service()
                .seal_wait_path_dispatch(command, &protocol, remote)?;
            if let Ok(mut guard) = self.completed.lock() {
                guard.insert(command_id.to_string(), remote.public_status());
            }
        }
        Ok(remote)
    }

    async fn status(
        &self,
        command_id: &str,
        session: &Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalCommandPublicStatus, CausalDispatchError> {
        if let Some(status) = self
            .completed
            .lock()
            .ok()
            .and_then(|guard| guard.get(command_id).cloned())
        {
            return Ok(status);
        }
        self.local
            .status(command_id, session, principal, protocol)
            .await
    }
}

fn graphql_chat_payload(input: &Value, remote: &Value, session: &Session) -> Value {
    json!({
        "message_id": remote.get("message_id").or_else(|| input.get("message_id")).cloned().unwrap_or(json!("")),
        "room_id": remote.get("room_id").or_else(|| input.get("room_id")).cloned().unwrap_or(json!("lobby")),
        "author_id": remote
            .get("author_id")
            .cloned()
            .or_else(|| session.user_id().map(|id| json!(id)))
            .unwrap_or(json!("")),
        "body": remote.get("body").or_else(|| input.get("body")).cloned().unwrap_or(json!("")),
        "created_at": remote.get("created_at").or_else(|| input.get("created_at")).cloned().unwrap_or(json!("")),
    })
}
