//! HTTP transport for microsvc — maps HTTP requests to command dispatch.
//!
//! Requires the `http` feature. Uses axum for routing.
//!
//! ## Routes
//!
//! - `POST /:command` — dispatch a command. Body = JSON input, request headers → Session.
//! - `GET /health` — health check returning `{ "ok": true, "commands": [...] }`.
//!
//! ## Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use sourced_rust::{microsvc, HashMapRepository};
//!
//! let service = Arc::new(
//!     microsvc::Service::new(HashMapRepository::new())
//!         .command("counter.create", |ctx| { /* ... */ })
//! );
//!
//! // Get the router to compose with other axum routes
//! let app = microsvc::router(service.clone());
//!
//! // Or serve directly
//! microsvc::serve(service, "0.0.0.0:3000").await?;
//! ```

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use super::session::Session;
use super::service::Service;

/// Build an axum `Router` that dispatches commands via the given service.
pub fn router<R: Send + Sync + 'static>(service: Arc<Service<R>>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/:command", axum::routing::post(command_handler))
        .with_state(service)
}

/// Serve the service over HTTP at the given address (e.g. `"0.0.0.0:3000"`).
pub async fn serve<R: Send + Sync + 'static>(
    service: Arc<Service<R>>,
    addr: &str,
) -> Result<(), std::io::Error> {
    let app = router(service);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

/// `GET /health` — returns `{ "ok": true, "commands": [...] }`.
async fn health_handler<R: Send + Sync + 'static>(
    State(service): State<Arc<Service<R>>>,
) -> impl IntoResponse {
    let commands: Vec<&str> = service.commands();
    Json(json!({ "ok": true, "commands": commands }))
}

/// `POST /:command` — dispatch a command with JSON body and headers as session.
async fn command_handler<R: Send + Sync + 'static>(
    State(service): State<Arc<Service<R>>>,
    Path(command): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> impl IntoResponse {
    let session = session_from_headers(&headers);
    match service.dispatch(&command, input, session) {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(e) => {
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body = json!({ "error": e.to_string() });
            (status, Json(body)).into_response()
        }
    }
}

/// Extract session variables from HTTP headers.
///
/// All headers are lowercased and included as session variables.
fn session_from_headers(headers: &HeaderMap) -> Session {
    let mut vars = std::collections::HashMap::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            vars.insert(name.as_str().to_string(), v.to_string());
        }
    }
    Session::from_map(vars)
}
