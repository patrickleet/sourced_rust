//! Todo and Chat Durable Object classes backed by `AggregateCell`.
//!
//! HTTP is command-named wait-path (`POST /{command}` with
//! `{ commandId, input }`) plus GET of the sealed row. GraphQL and
//! projectors are not methods on this class (`PCH-REQ-005`). Chat `@live`
//! stays on the GraphQL host.

use chat_domain::{post, ChatMessage, ChatMessageState};
use distributed::cell_host::{
    cell_projection_event_evidence, AggregateCell, CellCommandIdentity, CellDispatchError,
    CellDispatchResult, CellWaitPathRequest, CelldOutbox, DurableAggregateCellState,
    InternalHttpSecret, CELL_INTERNAL_SECRET_ENV, CELL_INTERNAL_SECRET_HEADER,
    CELL_PRINCIPAL_PARTITION_HEADER, CELL_SERVICE_ID_HEADER,
};
use distributed::microsvc::{Session, ROLE_KEY, USER_ID_KEY};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use todo_domain::{
    archive, complete, create, force_archive, purge, rename, reopen, Todo, TodoState,
};
use worker::*;

const MAX_CELL_REQUEST_BYTES: usize = 2 * 1024 * 1024;

const STATE_DDL: &str = "CREATE TABLE IF NOT EXISTS cell_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  body TEXT NOT NULL
)";

async fn persist_and_drain_cell<A>(
    sql: &SqlStorage,
    storage: &Storage,
    env: &Env,
    cell: &AggregateCell<A>,
) -> Result<()>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let outcome = cell
        .persist_and_drain_outbox(env, storage, |state| persist_cell_state(sql, state))
        .await
        .map_err(|error| Error::RustError(error.to_string()))?;
    for error in outcome.deferred {
        worker::console_error!(
            "celld outbox immediate drain deferred for {}: {}",
            cell.instance_name(),
            error
        );
    }
    Ok(())
}

#[durable_object]
pub struct TodoCell {
    cell: AggregateCell<Todo>,
    sql: SqlStorage,
    storage: Storage,
    env: Env,
}

impl DurableObject for TodoCell {
    fn new(state: State, env: Env) -> Self {
        console_error_panic_hook::set_once();
        let storage = state.storage();
        let sql = storage.sql();
        sql.exec(STATE_DDL, None).expect("create cell_state");
        let shard = state.id().name().unwrap_or_else(|| "todo".to_string());
        let outbox = CelldOutbox::from_env(&env, "OUTBOX").expect("OUTBOX Queue binding");
        let cell = AggregateCell::<Todo>::new_with_snapshots(shard.clone(), 1)
            .expect("todo cell identity")
            .mount(create())
            .mount(rename())
            .mount(complete())
            .mount(reopen())
            .mount(archive())
            .mount(force_archive())
            .mount(purge())
            .with_celld_outbox(outbox);
        let _ = restore_cell_state(&sql, &cell);
        Self {
            cell,
            sql,
            storage,
            env,
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if let Err(error) = authenticate_internal_request(&req, &self.env) {
            return internal_auth_error(error);
        }
        if let Err(error) = restore_cell_state(&self.sql, &self.cell) {
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
            _ => json_status(json!({ "error": "not found" }), 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if let Err(error) = restore_cell_state(&self.sql, &self.cell) {
            return json_status(json!({ "error": error }), 500);
        }
        persist_and_drain_cell(&self.sql, &self.storage, &self.env, &self.cell).await?;
        Response::ok("ok")
    }
}

#[durable_object]
pub struct ChatCell {
    cell: AggregateCell<ChatMessage>,
    sql: SqlStorage,
    storage: Storage,
    env: Env,
}

impl DurableObject for ChatCell {
    fn new(state: State, env: Env) -> Self {
        console_error_panic_hook::set_once();
        let storage = state.storage();
        let sql = storage.sql();
        sql.exec(STATE_DDL, None).expect("create cell_state");
        let shard = state.id().name().unwrap_or_else(|| "chat".to_string());
        let outbox = CelldOutbox::from_env(&env, "OUTBOX").expect("OUTBOX Queue binding");
        let cell = AggregateCell::<ChatMessage>::new(shard.clone())
            .expect("chat cell identity")
            .mount(post())
            .with_celld_outbox(outbox);
        let _ = restore_cell_state(&sql, &cell);
        Self {
            cell,
            sql,
            storage,
            env,
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if let Err(error) = authenticate_internal_request(&req, &self.env) {
            return internal_auth_error(error);
        }
        if let Err(error) = restore_cell_state(&self.sql, &self.cell) {
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
            _ => json_status(json!({ "error": "not found" }), 404),
        }
    }

    async fn alarm(&self) -> Result<Response> {
        if let Err(error) = restore_cell_state(&self.sql, &self.cell) {
            return json_status(json!({ "error": error }), 500);
        }
        persist_and_drain_cell(&self.sql, &self.storage, &self.env, &self.cell).await?;
        Response::ok("ok")
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
    if let Err(error) = authenticate_internal_request(&req, &env) {
        return internal_auth_error(error);
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let (binding, id) = match (parts.first().copied(), parts.get(1).copied()) {
        (Some("todo"), Some(id)) => ("TODO", id),
        (Some("chat"), Some(id)) => ("CHAT", id),
        _ => {
            return Response::error("cells: authenticated internal command/read routes\n", 404);
        }
    };
    let namespace = env.durable_object(binding)?;
    let stub = namespace.id_from_name(id)?.get_stub()?;
    stub.fetch_with_request(req).await
}

fn authenticate_internal_request(
    req: &Request,
    env: &Env,
) -> std::result::Result<(), CellDispatchError> {
    let configured = env
        .secret(CELL_INTERNAL_SECRET_ENV)
        .map(|value| value.to_string())
        .or_else(|_| {
            env.var(CELL_INTERNAL_SECRET_ENV)
                .map(|value| value.to_string())
        })
        .map_err(|_| {
            CellDispatchError::Internal(format!("{CELL_INTERNAL_SECRET_ENV} is required"))
        })?;
    let secret = InternalHttpSecret::new(configured).map_err(CellDispatchError::Internal)?;
    let candidate = req
        .headers()
        .get(CELL_INTERNAL_SECRET_HEADER)
        .map_err(|_| CellDispatchError::Unauthorized)?
        .ok_or(CellDispatchError::Unauthorized)?;
    if !secret.matches(&candidate) {
        return Err(CellDispatchError::Unauthorized);
    }
    Ok(())
}

fn internal_auth_error(error: CellDispatchError) -> Result<Response> {
    match error {
        CellDispatchError::Unauthorized => json_status(
            json!({ "code": "UNAUTHORIZED", "error": "unauthorized" }),
            401,
        ),
        _ => json_status(
            json!({ "code": "INTERNAL", "error": "internal cell authentication is unavailable" }),
            500,
        ),
    }
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
    let body = match bounded_json::<Value>(req).await {
        Ok(body) => body,
        Err(error) => return map_cell_error(CellDispatchError::BadRequest(error.into()), cell),
    };
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
            persist_and_drain_cell(sql, storage, env, cell).await?;
            let events = projection_events_wire(cell, dispatch.causation_id())?;
            wait_path_ok(dispatch.payload().clone(), &dispatch, 201, events)
        }
        Err(error) => {
            persist_and_drain_cell(sql, storage, env, cell).await?;
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

fn restore_cell_state<A>(
    sql: &SqlStorage,
    cell: &AggregateCell<A>,
) -> std::result::Result<(), String>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    if let Some(state) = load_cell_state(sql).map_err(|error| error.to_string())? {
        cell.restore_durable_state(state)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn seal_chat_from_load(cell: &AggregateCell<ChatMessage>) {
    if let Ok(Some(message)) = cell.load().await {
        let _ = cell.replace_sealed_row(http_chat(&ChatMessageState::from(&message)));
    }
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
    let request =
        CellWaitPathRequest::parse(body.clone()).map_err(CellDispatchError::BadRequest)?;
    Ok((request.command_id, request.input))
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
    events: Value,
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
            "events": events,
        }),
        status,
    )
}

fn projection_events_wire<A>(cell: &AggregateCell<A>, causation_id: &str) -> Result<Value>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let rows = cell
        .durable_outbox()
        .map_err(|error| Error::RustError(error.to_string()))?;
    serde_json::to_value(
        cell_projection_event_evidence(&rows, causation_id).map_err(Error::RustError)?,
    )
    .map_err(|error| Error::RustError(error.to_string()))
}

async fn create_todo(
    sql: &SqlStorage,
    storage: &Storage,
    env: &Env,
    cell: &AggregateCell<Todo>,
    id: &str,
    req: &mut Request,
) -> Result<Response> {
    let body = match bounded_json::<Value>(req).await {
        Ok(body) => body,
        Err(error) => return map_cell_error(CellDispatchError::BadRequest(error.into()), cell),
    };
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
            persist_and_drain_cell(sql, storage, env, cell).await?;
            let events = projection_events_wire(cell, dispatch.causation_id())?;
            wait_path_ok(
                http_from_command(id, dispatch.payload(), &title),
                &dispatch,
                201,
                events,
            )
        }
        Err(error) => {
            persist_and_drain_cell(sql, storage, env, cell).await?;
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
    let body = match bounded_json::<Value>(req).await {
        Ok(body) => body,
        Err(error) => return map_cell_error(CellDispatchError::BadRequest(error.into()), cell),
    };
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
            persist_and_drain_cell(sql, storage, env, cell).await?;
            let events = projection_events_wire(cell, dispatch.causation_id())?;
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
                events,
            )
        }
        Err(error) => {
            persist_and_drain_cell(sql, storage, env, cell).await?;
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

fn map_cell_error<A>(error: CellDispatchError, _cell: &AggregateCell<A>) -> Result<Response>
where
    A: distributed::Aggregate + Send + Sync + 'static,
{
    let status = error.status_code();
    json_status(
        json!({
            "error": error.client_message(),
            "code": error.code(),
        }),
        status,
    )
}

fn json_status(body: Value, status: u16) -> Result<Response> {
    Ok(Response::from_json(&body)?.with_status(status))
}

async fn bounded_json<T: DeserializeOwned>(
    req: &mut Request,
) -> std::result::Result<T, &'static str> {
    if req
        .headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_CELL_REQUEST_BYTES)
    {
        return Err("cell request exceeds 2 MiB");
    }
    let bytes = req
        .bytes()
        .await
        .map_err(|_| "could not read cell request body")?;
    if bytes.len() > MAX_CELL_REQUEST_BYTES {
        return Err("cell request exceeds 2 MiB");
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid cell request JSON")
}

async fn seal_from_load(cell: &AggregateCell<Todo>) {
    if let Ok(Some(todo)) = cell.load().await {
        let _ = cell.replace_sealed_row(http_todo(&TodoState::from(&todo)));
    }
}

fn persist_cell_state(sql: &SqlStorage, state: &DurableAggregateCellState) -> Result<()> {
    let body = serde_json::to_string(state).map_err(|error| Error::RustError(error.to_string()))?;
    sql.exec(
        "INSERT INTO cell_state (id, body) VALUES (1, ?) \
         ON CONFLICT(id) DO UPDATE SET body = excluded.body",
        Some(vec![body.into()]),
    )?;
    Ok(())
}

fn load_cell_state(sql: &SqlStorage) -> Result<Option<DurableAggregateCellState>> {
    let rows: Vec<StateRow> = sql
        .exec("SELECT body FROM cell_state WHERE id = 1", None)?
        .to_array()?;
    rows.into_iter()
        .next()
        .map(|row| {
            serde_json::from_str(&row.body).map_err(|error| Error::RustError(error.to_string()))
        })
        .transpose()
}

#[derive(Deserialize)]
struct StateRow {
    body: String,
}
