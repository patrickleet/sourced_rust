//! Wait-path host: celld for create/complete, local dual-write for SQL lists.

use std::sync::Arc;

use async_trait::async_trait;
use distributed::command_dispatch::{CommandHost, HttpCommandHost, LocalCommandHost};
use distributed::graphql::protocol::ProtocolResponseAccumulator;
use distributed::graphql::VerifiedPrincipal;
use distributed::microsvc::{
    CausalCommandPublicStatus, CausalDispatchError, CausalDispatchResult, Service, Session,
};
use serde_json::{json, Value};

const CELLD_TODO_COMMANDS: &[&str] = &["todo.create", "todo.complete"];

/// Routes `todo.create` / `todo.complete` to `{CELLD_URL}/todo/{id}/{command}`.
/// Other commands stay on the local [`Service`] so Chat/Blob and extra Todo
/// transitions keep working. After a cell wait-path succeeds, the local host
/// runs too so Eventual SQL lists fill (projectors are not cell methods).
pub struct CelldTodoCommandHost {
    celld_url: String,
    local: LocalCommandHost,
}

impl CelldTodoCommandHost {
    pub fn new(celld_url: impl Into<String>, service: Arc<Service>) -> Self {
        Self {
            celld_url: celld_url.into().trim_end_matches('/').to_string(),
            local: LocalCommandHost::new(service),
        }
    }
}

#[async_trait]
impl CommandHost for CelldTodoCommandHost {
    async fn invoke(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalDispatchResult, CausalDispatchError> {
        if !CELLD_TODO_COMMANDS.contains(&command) {
            return self
                .local
                .invoke(command, command_id, input, session, principal, protocol)
                .await;
        }
        let todo_id = input
            .get("todo_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CausalDispatchError::BadRequest("todo_id required for celld wait-path".into())
            })?;
        let remote = HttpCommandHost::new(format!("{}/todo/{todo_id}", self.celld_url));
        let remote = remote
            .invoke(
                command,
                command_id,
                input.clone(),
                session.clone(),
                principal.clone(),
                None,
            )
            .await?;
        match self
            .local
            .invoke(
                command,
                command_id,
                input.clone(),
                session.clone(),
                principal,
                protocol,
            )
            .await
        {
            Ok(local) => Ok(local),
            Err(error) => {
                eprintln!("e2e-celld: local dual-write after cell wait-path failed: {error:?}");
                let payload = graphql_todo_payload(command, &input, remote.payload(), &session);
                Ok(remote.with_payload(payload))
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
        self.local
            .status(command_id, session, principal, protocol)
            .await
    }
}

fn graphql_todo_payload(command: &str, input: &Value, remote: &Value, session: &Session) -> Value {
    let id = remote
        .get("todo_id")
        .or_else(|| remote.get("id"))
        .or_else(|| input.get("todo_id"))
        .cloned()
        .unwrap_or(json!(""));
    let status = remote.get("status").cloned().unwrap_or_else(|| {
        if command == "todo.complete" {
            json!("completed")
        } else {
            json!("open")
        }
    });
    if command == "todo.complete" {
        json!({ "todo_id": id, "status": status })
    } else {
        json!({
            "todo_id": id,
            "owner_id": session.user_id().unwrap_or("celld-local"),
            "title": remote.get("title").or_else(|| input.get("title")).cloned().unwrap_or(json!("")),
            "status": status,
        })
    }
}
