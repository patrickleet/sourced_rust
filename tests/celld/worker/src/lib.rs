//! Todo and Chat Durable Object classes backed by `AggregateCell`.
//!
//! HTTP is command-named wait-path (`POST /{command}` with
//! `{ commandId, input }`) plus GET of the sealed row. GraphQL and
//! projectors are not methods on this class (`PCH-REQ-005`). Chat `@live`
//! stays on the GraphQL host.

use std::time::Duration;

use chat_domain::{post, ChatMessage, ChatMessageState};
use distributed::cell_host::{
    AggregateCell, CellCommandIdentity, CellDispatchError, CellDispatchResult, DurableCellCommand,
    DurableCellEvents, DurableCellSnapshot, CELL_PRINCIPAL_PARTITION_HEADER,
    CELL_SERVICE_ID_HEADER,
};
use distributed::microsvc::{Session, ROLE_KEY, USER_ID_KEY};
use distributed::{EventRecord, OutboxMessage, OutboxMessageStatus};
use serde::Deserialize;
use serde_json::{json, Value};
use todo_domain::{
    archive, complete, create, force_archive, purge, rename, reopen, Todo, TodoState,
};
use worker::*;

const EVENTS_DDL: &str = "CREATE TABLE IF NOT EXISTS cell_events (
  stream TEXT NOT NULL,
  seq INTEGER NOT NULL,
  body TEXT NOT NULL,
  PRIMARY KEY (stream, seq)
)";

const SNAPSHOTS_DDL: &str = "CREATE TABLE IF NOT EXISTS cell_snapshots (
  stream TEXT PRIMARY KEY,
  body TEXT NOT NULL
)";

const SEALED_DDL: &str = "CREATE TABLE IF NOT EXISTS cell_sealed (
  id TEXT PRIMARY KEY,
  body TEXT NOT NULL
)";

const OUTBOX_DDL: &str = "CREATE TABLE IF NOT EXISTS cell_outbox (
  id TEXT PRIMARY KEY,
  body TEXT NOT NULL
)";

const COMMANDS_DDL: &str = "CREATE TABLE IF NOT EXISTS cell_commands (
  id TEXT PRIMARY KEY,
  body TEXT NOT NULL
)";

#[durable_object]
pub struct TodoCell {
    cell: AggregateCell<Todo>,
    sql: SqlStorage,
    storage: Storage,
    env: Env,
    shard: String,
}

impl DurableObject for TodoCell {
    fn new(state: State, env: Env) -> Self {
        console_error_panic_hook::set_once();
        let storage = state.storage();
        let sql = storage.sql();
        sql.exec(EVENTS_DDL, None).expect("create cell_events");
        sql.exec(SNAPSHOTS_DDL, None)
            .expect("create cell_snapshots");
        sql.exec(SEALED_DDL, None).expect("create cell_sealed");
        sql.exec(OUTBOX_DDL, None).expect("create cell_outbox");
        sql.exec(COMMANDS_DDL, None).expect("create cell_commands");
        let shard = state.id().name().unwrap_or_else(|| "todo".to_string());
        let cell = AggregateCell::<Todo>::new_with_snapshots(shard.clone(), 1)
            .expect("todo cell identity")
            .mount(create())
            .mount(rename())
            .mount(complete())
            .mount(reopen())
            .mount(archive())
            .mount(force_archive())
            .mount(purge());
        if let Ok(events) = load_events(&sql) {
            let _ = cell.restore_durable_events(events);
        }
        if let Ok(snapshots) = load_snapshots(&sql) {
            let _ = cell.restore_durable_snapshots(snapshots);
        }
        if let Ok(commands) = load_commands(&sql) {
            let _ = cell.restore_durable_commands(commands);
        }
        Self {
            cell,
            sql,
            storage,
            env,
            shard,
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if let Err(error) = restore_working_copy(&self.sql, &self.cell) {
            return json_status(json!({ "error": error }), 500);
        }
        let url = req.url()?;
        let parts: Vec<String> = url
            .path()
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
        let id = match parts.get(1) {
            Some(id) if parts.first().map(String::as_str) == Some("todo") => id.clone(),
            _ => return json_status(json!({ "error": "missing todo id" }), 400),
        };

        match (req.method(), parts.get(2).map(String::as_str)) {
            (Method::Get, None) => get_todo(&self.cell, &id).await,
            (Method::Post, Some("todo.create")) => {
                create_todo(
                    &self.sql,
                    &self.storage,
                    &self.env,
                    &self.cell,
                    &id,
                    &mut req,
                )
                .await
            }
            (Method::Post, Some(command))
                if matches!(
                    command,
                    "todo.rename"
                        | "todo.complete"
                        | "todo.reopen"
                        | "todo.archive"
                        | "todo.force_archive"
                        | "todo.purge"
                ) =>
            {
                transition_todo(
                    &self.sql,
                    &self.storage,
                    &self.env,
                    &self.cell,
                    &id,
                    command,
                    &mut req,
                )
                .await
            }
            (Method::Post, Some("outbox.complete")) => {
                complete_outbox(&self.sql, &self.storage, &self.env, &self.cell, &mut req).await
            }
            (Method::Post, Some("outbox.drain")) => drain_outbox(&self.cell),
            _ => json_status(json!({ "error": "not found" }), 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if let Err(error) = restore_working_copy(&self.sql, &self.cell) {
            return json_status(json!({ "error": error }), 500);
        }
        run_outbox_alarm(&self.storage, &self.env, &self.cell, "todo", &self.shard).await
    }
}

#[durable_object]
pub struct ChatCell {
    cell: AggregateCell<ChatMessage>,
    sql: SqlStorage,
    storage: Storage,
    env: Env,
    shard: String,
}

impl DurableObject for ChatCell {
    fn new(state: State, env: Env) -> Self {
        console_error_panic_hook::set_once();
        let storage = state.storage();
        let sql = storage.sql();
        sql.exec(EVENTS_DDL, None).expect("create cell_events");
        sql.exec(SEALED_DDL, None).expect("create cell_sealed");
        sql.exec(OUTBOX_DDL, None).expect("create cell_outbox");
        sql.exec(COMMANDS_DDL, None).expect("create cell_commands");
        let shard = state.id().name().unwrap_or_else(|| "chat".to_string());
        let cell = AggregateCell::<ChatMessage>::new(shard.clone())
            .expect("chat cell identity")
            .mount(post());
        if let Ok(events) = load_events(&sql) {
            let _ = cell.restore_durable_events(events);
        }
        if let Ok(commands) = load_commands(&sql) {
            let _ = cell.restore_durable_commands(commands);
        }
        Self {
            cell,
            sql,
            storage,
            env,
            shard,
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if let Err(error) = restore_chat_copy(&self.sql, &self.cell) {
            return json_status(json!({ "error": error }), 500);
        }
        let url = req.url()?;
        let parts: Vec<String> = url
            .path()
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
        let id = match parts.get(1) {
            Some(id) if parts.first().map(String::as_str) == Some("chat") => id.clone(),
            _ => return json_status(json!({ "error": "missing chat id" }), 400),
        };

        match (req.method(), parts.get(2).map(String::as_str)) {
            (Method::Get, None) => get_chat(&self.cell, &id).await,
            (Method::Post, Some("chat.post")) => {
                post_chat(
                    &self.sql,
                    &self.storage,
                    &self.env,
                    &self.cell,
                    &id,
                    &mut req,
                )
                .await
            }
            (Method::Post, Some("outbox.complete")) => {
                complete_outbox(&self.sql, &self.storage, &self.env, &self.cell, &mut req).await
            }
            (Method::Post, Some("outbox.drain")) => drain_outbox(&self.cell),
            _ => json_status(json!({ "error": "not found" }), 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if let Err(error) = restore_chat_copy(&self.sql, &self.cell) {
            return json_status(json!({ "error": error }), 500);
        }
        run_outbox_alarm(&self.storage, &self.env, &self.cell, "chat", &self.shard).await
    }
}

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    let url = req.url()?;
    let path = url.path();
    if path == "/" || path == "/health" {
        return Response::ok("distributed todo+chat cells\n");
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let (binding, id) = match (parts.first().copied(), parts.get(1).copied()) {
        (Some("todo"), Some(id)) => ("TODO", id),
        (Some("chat"), Some(id)) => ("CHAT", id),
        _ => {
            return Response::error(
                "cells: GET|POST /todo/:id[/todo.<command>|outbox.drain|outbox.complete]  GET|POST /chat/:id[/chat.post|outbox.drain|outbox.complete]\n",
                404,
            );
        }
    };
    let namespace = env.durable_object(binding)?;
    let stub = namespace.id_from_name(id)?.get_stub()?;
    stub.fetch_with_request(req).await
}

fn session_from_headers(user: Option<String>, roles: Option<String>) -> Session {
    let mut session = Session::new();
    if let Some(user) = user.filter(|value| !value.is_empty()) {
        session.set(USER_ID_KEY, user);
    }
    if let Some(roles) = roles.filter(|value| !value.is_empty()) {
        session.set(ROLE_KEY, roles);
    }
    session
}

fn request_session(req: &Request) -> Session {
    let user = req
        .headers()
        .get(USER_ID_KEY)
        .ok()
        .flatten()
        .filter(|value| !value.is_empty());
    let roles = req
        .headers()
        .get(ROLE_KEY)
        .ok()
        .flatten()
        .filter(|value| !value.is_empty());
    session_from_headers(user, roles)
}

async fn get_chat(cell: &AggregateCell<ChatMessage>, id: &str) -> Result<Response> {
    if let Ok(Some(row)) = cell.sealed_row() {
        return json_status(row, 200);
    }
    match cell.load().await {
        Ok(Some(message)) => json_status(http_chat(&ChatMessageState::from(&message)), 200),
        Ok(None) => json_status(json!({ "error": "not found", "id": id }), 404),
        Err(error) => json_status(json!({ "error": error.to_string() }), 500),
    }
}

async fn post_chat(
    sql: &SqlStorage,
    storage: &Storage,
    env: &Env,
    cell: &AggregateCell<ChatMessage>,
    id: &str,
    req: &mut Request,
) -> Result<Response> {
    let session = request_session(req);
    let body = req.json::<Value>().await.unwrap_or(json!({}));
    let (command_id, mut input) = match wait_path_parts(&body) {
        Ok(parts) => parts,
        Err(error) => return map_cell_error(error, cell),
    };
    let identity = match request_cell_identity(req, &command_id) {
        Ok(identity) => identity,
        Err(error) => return map_cell_error(error, cell),
    };
    if input.get("message_id").and_then(Value::as_str).is_none() {
        input
            .as_object_mut()
            .map(|object| object.insert("message_id".into(), json!(id)));
    }
    match cell
        .dispatch_idempotent("chat.post", &identity, input, session)
        .await
    {
        Ok(dispatch) => {
            seal_chat_from_load(cell).await;
            persist_chat_copy(sql, cell)?;
            arm_drain_alarm(storage, env, has_pending(cell)).await;
            wait_path_ok(
                dispatch.payload().clone(),
                &dispatch,
                201,
                outbox_wire(cell),
            )
        }
        Err(error) => {
            persist_chat_copy(sql, cell)?;
            arm_drain_alarm(storage, env, has_pending(cell)).await;
            map_cell_error(error, cell)
        }
    }
}

fn http_chat(state: &ChatMessageState) -> Value {
    json!({
        "message_id": state.message_id,
        "room_id": state.room_id,
        "author_id": state.author_id,
        "body": state.body,
        "created_at": state.created_at,
    })
}

fn restore_chat_copy(
    sql: &SqlStorage,
    cell: &AggregateCell<ChatMessage>,
) -> std::result::Result<(), String> {
    let events = load_events(sql).map_err(|error| error.to_string())?;
    cell.restore_durable_events(events)
        .map_err(|error| error.to_string())?;
    let commands = load_commands(sql).map_err(|error| error.to_string())?;
    cell.restore_durable_commands(commands)
        .map_err(|error| error.to_string())?;
    let outbox = load_outbox(sql).map_err(|error| error.to_string())?;
    cell.restore_durable_outbox(outbox)
        .map_err(|error| error.to_string())?;
    if let Some(row) = load_sealed(sql).map_err(|error| error.to_string())? {
        cell.replace_sealed_row(row)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn seal_chat_from_load(cell: &AggregateCell<ChatMessage>) {
    if let Ok(Some(message)) = cell.load().await {
        let _ = cell.replace_sealed_row(http_chat(&ChatMessageState::from(&message)));
    }
}

fn persist_chat_copy(sql: &SqlStorage, cell: &AggregateCell<ChatMessage>) -> Result<()> {
    let events = cell
        .durable_events()
        .map_err(|error| Error::RustError(error.to_string()))?;
    sql.exec("DELETE FROM cell_events", None)?;
    for stream in events {
        for event in stream.events {
            let body = serde_json::to_string(&event)
                .map_err(|error| Error::RustError(error.to_string()))?;
            sql.exec(
                "INSERT INTO cell_events (stream, seq, body) VALUES (?, ?, ?)",
                Some(vec![
                    stream.stream.clone().into(),
                    SqlStorageValue::Integer(event.sequence as i64),
                    body.into(),
                ]),
            )?;
        }
    }
    persist_commands(sql, cell)?;
    persist_outbox(sql, cell)?;
    sql.exec("DELETE FROM cell_sealed", None)?;
    if let Ok(Some(row)) = cell.sealed_row() {
        let body =
            serde_json::to_string(&row).map_err(|error| Error::RustError(error.to_string()))?;
        sql.exec(
            "INSERT INTO cell_sealed (id, body) VALUES (?, ?)",
            Some(vec!["row".into(), body.into()]),
        )?;
    }
    Ok(())
}

async fn get_todo(cell: &AggregateCell<Todo>, id: &str) -> Result<Response> {
    if let Ok(Some(row)) = cell.sealed_row() {
        return json_status(row, 200);
    }
    match cell.load().await {
        Ok(Some(todo)) => json_status(http_todo(&TodoState::from(&todo)), 200),
        Ok(None) => json_status(json!({ "error": "not found", "id": id }), 404),
        Err(error) => json_status(json!({ "error": error.to_string() }), 500),
    }
}

fn wait_path_parts(body: &Value) -> std::result::Result<(String, Value), CellDispatchError> {
    let command_id = body
        .get("commandId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CellDispatchError::BadRequest("commandId is required".into()))?;
    let input = body.get("input").cloned().unwrap_or_else(|| body.clone());
    Ok((command_id, input))
}

fn request_cell_identity(
    req: &Request,
    command_id: &str,
) -> std::result::Result<CellCommandIdentity, CellDispatchError> {
    let service_id = required_internal_header(req, CELL_SERVICE_ID_HEADER)?;
    let principal_partition = required_internal_header(req, CELL_PRINCIPAL_PARTITION_HEADER)?;
    CellCommandIdentity::new(service_id, principal_partition, command_id)
}

fn required_internal_header(
    req: &Request,
    name: &str,
) -> std::result::Result<String, CellDispatchError> {
    req.headers()
        .get(name)
        .map_err(|error| {
            CellDispatchError::Internal(format!("could not read internal cell header: {error}"))
        })?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or(CellDispatchError::Unauthorized)
}

fn wait_path_ok(
    payload: Value,
    dispatch: &CellDispatchResult,
    status: u16,
    outbox: Value,
) -> Result<Response> {
    json_status(
        json!({
            "payload": payload,
            "receipt": {
                "commandId": dispatch.command_id(),
                "causationId": dispatch.causation_id(),
                "state": dispatch.state(),
                "replayed": dispatch.replayed(),
            },
            "outbox": outbox,
        }),
        status,
    )
}

fn outbox_wire<A>(cell: &AggregateCell<A>) -> Value
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let rows = cell.durable_outbox().unwrap_or_default();
    Value::Array(
        rows.iter()
            .filter(|message| message.is_pending())
            .map(outbox_item)
            .collect(),
    )
}

fn has_pending<A>(cell: &AggregateCell<A>) -> bool
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    cell.durable_outbox()
        .ok()
        .map(|rows| rows.iter().any(OutboxMessage::is_pending))
        .unwrap_or(false)
}

async fn create_todo(
    sql: &SqlStorage,
    storage: &Storage,
    env: &Env,
    cell: &AggregateCell<Todo>,
    id: &str,
    req: &mut Request,
) -> Result<Response> {
    let body = req.json::<Value>().await.unwrap_or(json!({}));
    let (command_id, input) = match wait_path_parts(&body) {
        Ok(parts) => parts,
        Err(error) => return map_cell_error(error, cell),
    };
    let identity = match request_cell_identity(req, &command_id) {
        Ok(identity) => identity,
        Err(error) => return map_cell_error(error, cell),
    };
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match cell
        .dispatch_idempotent(
            "todo.create",
            &identity,
            json!({ "todo_id": id, "title": title }),
            request_session(req),
        )
        .await
    {
        Ok(dispatch) => {
            seal_from_load(cell).await;
            persist_working_copy(sql, cell)?;
            arm_drain_alarm(storage, env, has_pending(cell)).await;
            wait_path_ok(
                http_from_command(id, dispatch.payload(), &title),
                &dispatch,
                201,
                outbox_wire(cell),
            )
        }
        Err(error) => {
            persist_working_copy(sql, cell)?;
            arm_drain_alarm(storage, env, has_pending(cell)).await;
            map_cell_error(error, cell)
        }
    }
}

async fn transition_todo(
    sql: &SqlStorage,
    storage: &Storage,
    env: &Env,
    cell: &AggregateCell<Todo>,
    id: &str,
    command: &str,
    req: &mut Request,
) -> Result<Response> {
    let body = req.json::<Value>().await.unwrap_or(json!({}));
    let (command_id, mut input) = match wait_path_parts(&body) {
        Ok(parts) => parts,
        Err(error) => return map_cell_error(error, cell),
    };
    let identity = match request_cell_identity(req, &command_id) {
        Ok(identity) => identity,
        Err(error) => return map_cell_error(error, cell),
    };
    let Some(input_object) = input.as_object_mut() else {
        return map_cell_error(
            CellDispatchError::BadRequest("input must be an object".into()),
            cell,
        );
    };
    input_object
        .entry("todo_id".to_string())
        .or_insert_with(|| json!(id));
    match cell
        .dispatch_idempotent(command, &identity, input, request_session(req))
        .await
    {
        Ok(dispatch) => {
            seal_from_load(cell).await;
            persist_working_copy(sql, cell)?;
            arm_drain_alarm(storage, env, has_pending(cell)).await;
            let title = cell
                .load()
                .await
                .ok()
                .flatten()
                .map(|todo| TodoState::from(&todo).title)
                .unwrap_or_default();
            wait_path_ok(
                http_from_command(id, dispatch.payload(), &title),
                &dispatch,
                200,
                outbox_wire(cell),
            )
        }
        Err(error) => {
            persist_working_copy(sql, cell)?;
            arm_drain_alarm(storage, env, has_pending(cell)).await;
            map_cell_error(error, cell)
        }
    }
}

fn http_todo(state: &TodoState) -> Value {
    json!({
        "id": state.todo_id,
        "title": state.title,
        "status": state.status,
    })
}

fn http_from_command(id: &str, payload: &Value, fallback_title: &str) -> Value {
    let mut body = payload.as_object().cloned().unwrap_or_default();
    body.entry("id".to_string()).or_insert_with(|| json!(id));
    body.entry("todo_id".to_string())
        .or_insert_with(|| json!(id));
    body.entry("owner_id".to_string())
        .or_insert_with(|| json!(""));
    body.entry("title".to_string())
        .or_insert_with(|| json!(fallback_title));
    body.entry("status".to_string())
        .or_insert_with(|| json!("open"));
    Value::Object(body)
}

fn map_cell_error<A>(error: CellDispatchError, cell: &AggregateCell<A>) -> Result<Response>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let status = error.status_code();
    json_status(
        json!({
            "error": error.client_message(),
            "code": error.code(),
            "outbox": outbox_wire(cell),
        }),
        status,
    )
}

fn json_status(body: Value, status: u16) -> Result<Response> {
    Ok(Response::from_json(&body)?.with_status(status))
}

#[derive(Deserialize)]
struct EventRow {
    stream: String,
    #[allow(dead_code)]
    seq: i64,
    body: String,
}

fn restore_working_copy(
    sql: &SqlStorage,
    cell: &AggregateCell<Todo>,
) -> std::result::Result<(), String> {
    let events = load_events(sql).map_err(|error| error.to_string())?;
    cell.restore_durable_events(events)
        .map_err(|error| error.to_string())?;
    let snapshots = load_snapshots(sql).map_err(|error| error.to_string())?;
    cell.restore_durable_snapshots(snapshots)
        .map_err(|error| error.to_string())?;
    let commands = load_commands(sql).map_err(|error| error.to_string())?;
    cell.restore_durable_commands(commands)
        .map_err(|error| error.to_string())?;
    let outbox = load_outbox(sql).map_err(|error| error.to_string())?;
    cell.restore_durable_outbox(outbox)
        .map_err(|error| error.to_string())?;
    if let Some(row) = load_sealed(sql).map_err(|error| error.to_string())? {
        cell.replace_sealed_row(row)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn seal_from_load(cell: &AggregateCell<Todo>) {
    if let Ok(Some(todo)) = cell.load().await {
        let _ = cell.replace_sealed_row(http_todo(&TodoState::from(&todo)));
    }
}

fn persist_working_copy(sql: &SqlStorage, cell: &AggregateCell<Todo>) -> Result<()> {
    let events = cell
        .durable_events()
        .map_err(|error| Error::RustError(error.to_string()))?;
    sql.exec("DELETE FROM cell_events", None)?;
    for stream in events {
        for event in stream.events {
            let body = serde_json::to_string(&event)
                .map_err(|error| Error::RustError(error.to_string()))?;
            sql.exec(
                "INSERT INTO cell_events (stream, seq, body) VALUES (?, ?, ?)",
                Some(vec![
                    stream.stream.clone().into(),
                    SqlStorageValue::Integer(event.sequence as i64),
                    body.into(),
                ]),
            )?;
        }
    }
    let snapshots = cell
        .durable_snapshots()
        .map_err(|error| Error::RustError(error.to_string()))?;
    sql.exec("DELETE FROM cell_snapshots", None)?;
    for snapshot in snapshots {
        let body = serde_json::to_string(&snapshot)
            .map_err(|error| Error::RustError(error.to_string()))?;
        sql.exec(
            "INSERT INTO cell_snapshots (stream, body) VALUES (?, ?)",
            Some(vec![snapshot.stream.into(), body.into()]),
        )?;
    }
    persist_commands(sql, cell)?;
    persist_outbox(sql, cell)?;
    sql.exec("DELETE FROM cell_sealed", None)?;
    if let Ok(Some(row)) = cell.sealed_row() {
        let body =
            serde_json::to_string(&row).map_err(|error| Error::RustError(error.to_string()))?;
        sql.exec(
            "INSERT INTO cell_sealed (id, body) VALUES (?, ?)",
            Some(vec!["row".into(), body.into()]),
        )?;
    }
    Ok(())
}

fn persist_outbox<A>(sql: &SqlStorage, cell: &AggregateCell<A>) -> Result<()>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let rows = cell
        .durable_outbox()
        .map_err(|error| Error::RustError(error.to_string()))?;
    sql.exec("DELETE FROM cell_outbox", None)?;
    for message in rows {
        let body = serde_json::to_string(&outbox_item(&message))
            .map_err(|error| Error::RustError(error.to_string()))?;
        sql.exec(
            "INSERT INTO cell_outbox (id, body) VALUES (?, ?)",
            Some(vec![message.id.into(), body.into()]),
        )?;
    }
    Ok(())
}

fn persist_commands<A>(sql: &SqlStorage, cell: &AggregateCell<A>) -> Result<()>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let rows = cell
        .durable_commands()
        .map_err(|error| Error::RustError(error.to_string()))?;
    sql.exec("DELETE FROM cell_commands", None)?;
    for command in rows {
        sql.exec(
            "INSERT INTO cell_commands (id, body) VALUES (?, ?)",
            Some(vec![command.id.into(), command.body.into()]),
        )?;
    }
    Ok(())
}

fn load_commands(sql: &SqlStorage) -> Result<Vec<DurableCellCommand>> {
    sql.exec("SELECT id, body FROM cell_commands ORDER BY id", None)?
        .to_array()
}

fn outbox_item(message: &OutboxMessage) -> Value {
    json!({
        "id": message.id,
        "eventType": message.event_type,
        "payload": message.payload,
        "payloadCodec": message.payload_codec,
        "payloadCodecVersion": message.payload_codec_version,
        "status": message.status.as_str(),
        "metadata": message.metadata,
        "sourceAggregateType": message.source_aggregate_type,
        "sourceAggregateId": message.source_aggregate_id,
        "sourceSequence": message.source_sequence,
    })
}

async fn complete_outbox<A>(
    sql: &SqlStorage,
    storage: &Storage,
    env: &Env,
    cell: &AggregateCell<A>,
    req: &mut Request,
) -> Result<Response>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let body = req.json::<Value>().await.unwrap_or(json!({}));
    let ids = ids_from_body(&body);
    mark_outbox_published(sql, cell, &ids)?;
    arm_drain_alarm(storage, env, has_pending(cell)).await;
    json_status(json!({ "ok": true }), 200)
}

fn drain_outbox<A>(cell: &AggregateCell<A>) -> Result<Response>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    json_status(json!({ "outbox": outbox_wire(cell) }), 200)
}

fn ids_from_body(body: &Value) -> Vec<String> {
    if let Some(ids) = body.get("ids").and_then(Value::as_array) {
        return ids
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
    }
    body.get("id")
        .and_then(Value::as_str)
        .map(|id| vec![id.to_string()])
        .unwrap_or_default()
}

fn mark_outbox_published<A>(sql: &SqlStorage, cell: &AggregateCell<A>, ids: &[String]) -> Result<()>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    if ids.is_empty() {
        return Ok(());
    }
    let mut rows = cell
        .durable_outbox()
        .map_err(|error| Error::RustError(error.to_string()))?;
    let mut changed = false;
    for row in &mut rows {
        if ids.iter().any(|id| id == &row.id) && row.status != OutboxMessageStatus::Published {
            row.status = OutboxMessageStatus::Published;
            changed = true;
        }
    }
    if changed {
        cell.restore_durable_outbox(rows)
            .map_err(|error| Error::RustError(error.to_string()))?;
        persist_outbox(sql, cell)?;
    }
    Ok(())
}

async fn arm_drain_alarm(storage: &Storage, env: &Env, pending: bool) {
    if !pending {
        let _ = storage.delete_alarm().await;
        return;
    }
    if drain_url(env).is_none() {
        return;
    }
    let ms = env
        .var("OUTBOX_DRAIN_INTERVAL_MS")
        .ok()
        .and_then(|value| value.to_string().parse::<u64>().ok())
        .unwrap_or(5_000)
        .max(1_000);
    let _ = storage.set_alarm(Duration::from_millis(ms)).await;
}

async fn run_outbox_alarm<A>(
    storage: &Storage,
    env: &Env,
    cell: &AggregateCell<A>,
    kind: &str,
    id: &str,
) -> Result<Response>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let pending = outbox_wire(cell);
    if pending
        .as_array()
        .map(|rows| !rows.is_empty())
        .unwrap_or(false)
    {
        offer_pending(env, kind, id, &pending).await;
        arm_drain_alarm(storage, env, true).await;
    }
    Response::ok("ok")
}

fn drain_url(env: &Env) -> Option<String> {
    env.var("OUTBOX_DRAIN_URL")
        .ok()
        .map(|value| value.to_string())
        .filter(|url| !url.is_empty())
}

async fn offer_pending(env: &Env, kind: &str, id: &str, outbox: &Value) {
    let Some(url) = drain_url(env) else {
        return;
    };
    let payload = json!({ "kind": kind, "id": id, "outbox": outbox });
    let headers = Headers::new();
    let _ = headers.set("content-type", "application/json");
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(
            &payload.to_string(),
        )));
    if let Ok(req) = Request::new_with_init(&url, &init) {
        let _ = Fetch::Request(req).send().await;
    }
}

fn load_outbox(sql: &SqlStorage) -> Result<Vec<OutboxMessage>> {
    let rows: Vec<OutboxRow> = match sql.exec("SELECT id, body FROM cell_outbox", None) {
        Ok(cursor) => cursor.to_array()?,
        Err(_) => return Ok(Vec::new()),
    };
    rows.into_iter()
        .map(|row| {
            let value: Value = serde_json::from_str(&row.body)
                .map_err(|error| Error::RustError(error.to_string()))?;
            parse_outbox_item(&value)
        })
        .collect()
}

fn parse_outbox_item(item: &Value) -> Result<OutboxMessage> {
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::RustError("outbox id".into()))?;
    let event_type = item
        .get("eventType")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::RustError("outbox eventType".into()))?;
    let payload = item
        .get("payload")
        .and_then(Value::as_array)
        .map(|bytes| {
            bytes
                .iter()
                .filter_map(|byte| byte.as_u64().map(|value| value as u8))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let metadata = item
        .get("metadata")
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let mut message = OutboxMessage::create_with_metadata(
        id.to_string(),
        event_type.to_string(),
        payload,
        metadata,
    )
    .map_err(|error| Error::RustError(error.to_string()))?;
    if let Some(codec) = item.get("payloadCodec").and_then(Value::as_str) {
        message.payload_codec = codec.to_string();
    }
    if let Some(version) = item.get("payloadCodecVersion").and_then(Value::as_u64) {
        message.payload_codec_version = version as u16;
    }
    message.source_aggregate_type = item
        .get("sourceAggregateType")
        .and_then(Value::as_str)
        .map(str::to_string);
    message.source_aggregate_id = item
        .get("sourceAggregateId")
        .and_then(Value::as_str)
        .map(str::to_string);
    message.source_sequence = item.get("sourceSequence").and_then(Value::as_u64);
    if let Some(status) = item.get("status").and_then(Value::as_str) {
        if let Ok(parsed) = status.parse::<OutboxMessageStatus>() {
            message.status = parsed;
        }
    }
    Ok(message)
}

#[derive(Deserialize)]
struct OutboxRow {
    #[allow(dead_code)]
    id: String,
    body: String,
}

fn load_sealed(sql: &SqlStorage) -> Result<Option<Value>> {
    let rows: Vec<SealedRow> = sql
        .exec("SELECT id, body FROM cell_sealed", None)?
        .to_array()?;
    rows.into_iter()
        .next()
        .map(|row| {
            serde_json::from_str(&row.body).map_err(|error| Error::RustError(error.to_string()))
        })
        .transpose()
}

#[derive(Deserialize)]
struct SealedRow {
    #[allow(dead_code)]
    id: String,
    body: String,
}

fn load_events(sql: &SqlStorage) -> Result<Vec<DurableCellEvents>> {
    let rows: Vec<EventRow> = sql
        .exec(
            "SELECT stream, seq, body FROM cell_events ORDER BY stream, seq",
            None,
        )?
        .to_array()?;
    let mut grouped: Vec<DurableCellEvents> = Vec::new();
    for row in rows {
        let event: EventRecord =
            serde_json::from_str(&row.body).map_err(|error| Error::RustError(error.to_string()))?;
        match grouped.last_mut() {
            Some(stream) if stream.stream == row.stream => stream.events.push(event),
            _ => grouped.push(DurableCellEvents {
                stream: row.stream,
                events: vec![event],
            }),
        }
    }
    Ok(grouped)
}

#[derive(Deserialize)]
struct SnapshotRow {
    stream: String,
    body: String,
}

fn load_snapshots(sql: &SqlStorage) -> Result<Vec<DurableCellSnapshot>> {
    let rows: Vec<SnapshotRow> = sql
        .exec("SELECT stream, body FROM cell_snapshots", None)?
        .to_array()?;
    let mut snapshots = Vec::new();
    for row in rows {
        let mut snapshot: DurableCellSnapshot =
            serde_json::from_str(&row.body).map_err(|error| Error::RustError(error.to_string()))?;
        snapshot.stream = row.stream;
        snapshots.push(snapshot);
    }
    Ok(snapshots)
}
