//! Todo Durable Object class backed by `AggregateCell<Todo>`.
//!
//! HTTP is a thin adapter over domain create/complete + stream load.
//! GraphQL and projectors are not methods on this class (`PCH-REQ-005`).

use distributed::cell_host::{AggregateCell, DurableCellEvents, DurableCellSnapshot};
use distributed::microsvc::{HandlerError, Session, ROLE_KEY, USER_ID_KEY};
use distributed::EventRecord;
use serde::Deserialize;
use serde_json::{json, Value};
use todo_domain::{complete, create, Todo, TodoState};
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

#[durable_object]
pub struct TodoCell {
    cell: AggregateCell<Todo>,
    sql: SqlStorage,
}

impl DurableObject for TodoCell {
    fn new(state: State, _env: Env) -> Self {
        console_error_panic_hook::set_once();
        let sql = state.storage().sql();
        sql.exec(EVENTS_DDL, None).expect("create cell_events");
        sql.exec(SNAPSHOTS_DDL, None)
            .expect("create cell_snapshots");
        let shard = state.id().name().unwrap_or_else(|| "todo".to_string());
        let cell = AggregateCell::<Todo>::new_with_snapshots(shard, 1)
            .expect("todo cell identity")
            .mount(create())
            .mount(complete());
        if let Ok(events) = load_events(&sql) {
            let _ = cell.restore_durable_events(events);
        }
        if let Ok(snapshots) = load_snapshots(&sql) {
            let _ = cell.restore_durable_snapshots(snapshots);
        }
        Self { cell, sql }
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
            (Method::Put, None) => create_todo(&self.sql, &self.cell, &id, &mut req).await,
            (Method::Post, Some("complete")) => complete_todo(&self.sql, &self.cell, &id).await,
            _ => json_status(json!({ "error": "not found" }), 404),
        }
    }
}

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();
    let url = req.url()?;
    let path = url.path();
    if path == "/" || path == "/health" {
        return Response::ok("distributed todo cell\n");
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.first() != Some(&"todo") || parts.get(1).is_none() {
        return Response::error(
            "todo cell. PUT/GET /todo/:id  POST /todo/:id/complete\n",
            404,
        );
    }
    let namespace = env.durable_object("TODO")?;
    let stub = namespace.id_from_name(parts[1])?.get_stub()?;
    stub.fetch_with_request(req).await
}

#[derive(Deserialize)]
struct CreateBody {
    title: Option<String>,
}

fn local_session() -> Session {
    let mut session = Session::new();
    session.set(USER_ID_KEY, "celld-local");
    session.set(ROLE_KEY, "user");
    session
}

async fn get_todo(cell: &AggregateCell<Todo>, id: &str) -> Result<Response> {
    match cell.load().await {
        Ok(Some(todo)) => json_status(http_todo(&TodoState::from(&todo)), 200),
        Ok(None) => json_status(json!({ "error": "not found", "id": id }), 404),
        Err(error) => json_status(json!({ "error": error.to_string() }), 500),
    }
}

async fn create_todo(
    sql: &SqlStorage,
    cell: &AggregateCell<Todo>,
    id: &str,
    req: &mut Request,
) -> Result<Response> {
    let body = req
        .json::<CreateBody>()
        .await
        .unwrap_or(CreateBody { title: None });
    let title = body.title.unwrap_or_default();
    let title = title.trim();
    if title.is_empty() {
        return json_status(json!({ "error": "title required" }), 400);
    }
    match cell
        .dispatch(
            "todo.create",
            json!({ "todo_id": id, "title": title }),
            local_session(),
        )
        .await
    {
        Ok(payload) => {
            persist_working_copy(sql, cell)?;
            json_status(http_from_command(id, &payload, title), 201)
        }
        Err(HandlerError::Rejected(message)) if message.contains("already exists") => {
            json_status(json!({ "error": "already exists", "id": id }), 409)
        }
        Err(error) => map_handler_error(error),
    }
}

async fn complete_todo(sql: &SqlStorage, cell: &AggregateCell<Todo>, id: &str) -> Result<Response> {
    match cell
        .dispatch("todo.complete", json!({ "todo_id": id }), local_session())
        .await
    {
        Ok(payload) => {
            persist_working_copy(sql, cell)?;
            let title = cell
                .load()
                .await
                .ok()
                .flatten()
                .map(|todo| TodoState::from(&todo).title)
                .unwrap_or_default();
            json_status(http_from_command(id, &payload, &title), 200)
        }
        Err(HandlerError::NotFound(_)) => {
            json_status(json!({ "error": "not found", "id": id }), 404)
        }
        Err(HandlerError::Rejected(message)) if message.to_lowercase().contains("not found") => {
            json_status(json!({ "error": "not found", "id": id }), 404)
        }
        Err(HandlerError::Rejected(message)) if message.contains("not open") => json_status(
            json!({ "error": "not open", "id": id, "status": "completed" }),
            422,
        ),
        Err(error) => map_handler_error(error),
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
    json!({
        "id": payload.get("todo_id").cloned().unwrap_or_else(|| json!(id)),
        "title": payload.get("title").cloned().unwrap_or_else(|| json!(fallback_title)),
        "status": payload.get("status").cloned().unwrap_or_else(|| json!("open")),
    })
}

fn map_handler_error(error: HandlerError) -> Result<Response> {
    let status = match &error {
        HandlerError::NotFound(_) => 404,
        HandlerError::Unauthorized(_) | HandlerError::GuardRejected(_) => 401,
        HandlerError::Rejected(_) => 422,
        HandlerError::DecodeFailed(_) => 400,
        _ => 500,
    };
    json_status(json!({ "error": error.to_string() }), status)
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
        .map_err(|error| error.to_string())
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
    Ok(())
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
