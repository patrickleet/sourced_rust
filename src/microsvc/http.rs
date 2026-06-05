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
//! use distributed::{microsvc, HashMapRepository};
//!
//! let service = Arc::new(
//!     microsvc::Service::new().with_repo(HashMapRepository::new())
//!         .command("counter.create")
//!         .handle(|ctx| { /* ... */ })
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

use super::error::HandlerError;
use super::service::Service;
use super::session::Session;

/// Build an axum `Router` that dispatches commands via the given service.
pub fn router<D: Send + Sync + 'static>(service: Arc<Service<D>>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/:command", axum::routing::post(command_handler))
        .with_state(service)
}

/// Serve the service over HTTP at the given address (e.g. `"0.0.0.0:3000"`).
pub async fn serve<D: Send + Sync + 'static>(
    service: Arc<Service<D>>,
    addr: &str,
) -> Result<(), std::io::Error> {
    let app = router(service);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

/// `GET /health` — returns `{ "ok": true, "commands": [...] }`.
async fn health_handler<D: Send + Sync + 'static>(
    State(service): State<Arc<Service<D>>>,
) -> impl IntoResponse {
    let commands: Vec<&str> = service.command_names();
    Json(json!({ "ok": true, "commands": commands }))
}

/// `POST /:command` — dispatch a command with JSON body and headers as session.
async fn command_handler<D: Send + Sync + 'static>(
    State(service): State<Arc<Service<D>>>,
    Path(command): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> impl IntoResponse {
    let session = session_from_headers(&headers);
    match service.dispatch(&command, input, session).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => {
            let status = status_for_error(&err);
            if status.is_server_error() {
                eprintln!("microsvc command `{command}` failed: {err}");
            }
            let body = json!({ "error": error_message_for_response(&err) });
            (status, Json(body)).into_response()
        }
    }
}

fn status_for_error(error: &HandlerError) -> StatusCode {
    match error {
        HandlerError::UnknownCommand(_) | HandlerError::NotFound(_) => StatusCode::NOT_FOUND,
        HandlerError::DecodeFailed(_) | HandlerError::GuardRejected(_) => StatusCode::BAD_REQUEST,
        HandlerError::Rejected(_) => StatusCode::UNPROCESSABLE_ENTITY,
        HandlerError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
        HandlerError::Repository(_) | HandlerError::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn error_message_for_response(error: &HandlerError) -> String {
    if status_for_error(error).is_server_error() {
        "Internal server error".to_string()
    } else {
        error.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::RepositoryError;

    #[test]
    fn status_for_error_maps_all_handler_errors() {
        let cases = vec![
            (
                HandlerError::UnknownCommand("missing".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                HandlerError::DecodeFailed("bad json".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                HandlerError::Rejected("invalid command".into()),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                HandlerError::NotFound("counter-1".into()),
                StatusCode::NOT_FOUND,
            ),
            (
                HandlerError::Unauthorized("missing user".into()),
                StatusCode::UNAUTHORIZED,
            ),
            (
                HandlerError::Repository(RepositoryError::Model("store failed".into())),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                HandlerError::GuardRejected("counter.create".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                HandlerError::Other(Box::new(std::io::Error::other("handler failed"))),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (error, expected) in cases {
            let status = status_for_error(&error);
            assert_eq!(status, expected);
            assert_eq!(status.as_u16(), error.status_code());
            assert!(!status.is_success());
        }
    }

    #[test]
    fn error_message_for_response_preserves_client_errors() {
        let error = HandlerError::Rejected("invalid command".into());

        assert_eq!(
            error_message_for_response(&error),
            "rejected: invalid command"
        );
    }

    #[test]
    fn error_message_for_response_hides_server_errors() {
        let errors = [
            HandlerError::Repository(RepositoryError::Model("store failed".into())),
            HandlerError::Other(Box::new(std::io::Error::other("handler failed"))),
        ];

        for error in errors {
            assert_eq!(error_message_for_response(&error), "Internal server error");
        }
    }
}
