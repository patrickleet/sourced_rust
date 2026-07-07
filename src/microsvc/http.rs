//! HTTP transport for microsvc — maps HTTP requests to command dispatch.
//!
//! Requires the `http` feature. Uses axum for routing.
//!
//! ## Routes
//!
//! - `POST /{command}` — dispatch a command. Body = JSON input, request headers → Session.
//! - `GET /health` — health check returning `{ "ok": true, "commands": [...] }`.
//! - `GET /metrics` — Prometheus text metrics (requires the `metrics` feature);
//!   unauthenticated by design and intended for private scrape networks only.
//!
//! ## Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use distributed::{microsvc, InMemoryRepository};
//!
//! let service = Arc::new(
//!     microsvc::Service::new().routes(
//!         microsvc::Routes::new()
//!             .with_repo(InMemoryRepository::new().queued().aggregate::<Counter>())
//!             .command("counter.create")
//!             .handle(|ctx| { /* ... */ })
//!     )
//! );
//!
//! // Get the router to compose with other axum routes
//! let app = microsvc::router(service.clone());
//!
//! // Or serve directly
//! microsvc::serve(service, "0.0.0.0:3000").await?;
//! ```

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use super::error::HandlerError;
use super::service::Service;
use super::session::Session;
use super::MAX_HTTP_BODY_BYTES;
use crate::bus::MessageKind;
use crate::failure_log::{FailureComponent, FailureMessageFields, FailureOperation, FailureRecord};

/// Build an axum `Router` that dispatches commands via the given service.
pub fn router(service: Arc<Service>) -> Router {
    let router = Router::new()
        .route("/health", get(health_handler))
        .route("/{command}", axum::routing::post(command_handler));
    #[cfg(feature = "metrics")]
    let router = router.route("/metrics", get(metrics_handler));

    router
        // Pin the body limit explicitly rather than relying on axum's default;
        // the command handler buffers the JSON body into memory.
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(service)
}

/// Serve the service over HTTP at the given address (e.g. `"0.0.0.0:3000"`).
pub async fn serve(service: Arc<Service>, addr: &str) -> Result<(), std::io::Error> {
    let app = router(service);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

/// `GET /health` — returns `{ "ok": true, "commands": [...] }`.
async fn health_handler(State(service): State<Arc<Service>>) -> impl IntoResponse {
    let commands: Vec<&str> = service.command_names();
    Json(json!({ "ok": true, "commands": commands }))
}

/// `GET /metrics` — returns Prometheus text metrics.
///
/// This endpoint is unauthenticated by design for Prometheus scraping. Keep it
/// behind a private listener, ingress policy, security group, or equivalent
/// network restriction.
#[cfg(feature = "metrics")]
async fn metrics_handler(State(service): State<Arc<Service>>) -> impl IntoResponse {
    crate::metrics::prometheus_response(service.name())
}

/// `POST /{command}` — dispatch a command with JSON body and headers as session.
async fn command_handler(
    State(service): State<Arc<Service>>,
    Path(command): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> impl IntoResponse {
    let session = session_from_headers(&headers);
    let metadata = session_metadata(&session);
    match service.dispatch(&command, input, session).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => {
            let status = status_for_error(&err);
            emit_http_failure(service.as_ref(), &command, &metadata, status, &err);
            let body = json!({ "error": err.client_facing_message() });
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

fn emit_http_failure(
    service: &Service,
    command: &str,
    metadata: &[(String, String)],
    status: StatusCode,
    error: &HandlerError,
) {
    let message = if matches!(error, HandlerError::UnknownCommand(_)) {
        FailureMessageFields::unknown()
    } else {
        FailureMessageFields::for_name_with_metadata(MessageKind::Command, command, metadata)
    };
    FailureRecord::from_handler_error(FailureComponent::Http, FailureOperation::Ingress, error)
        .with_service(service.name())
        .with_message(message)
        .with_http_status(status.as_u16())
        .emit();
}

/// Extract session variables from HTTP headers.
///
/// **Trust boundary (security-critical):** every request header is copied
/// verbatim into the [`Session`] — including `x-hasura-user-id` and
/// `x-hasura-role`. The framework does NOT authenticate. A trusted proxy in
/// front of this service MUST strip any client-supplied `x-hasura-*` headers
/// and inject only authenticated ones. Without that proxy, any client can set
/// these headers and assume any identity/role. See the [`Session`] docs.
fn session_from_headers(headers: &HeaderMap) -> Session {
    let mut vars = std::collections::HashMap::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            vars.insert(name.as_str().to_string(), v.to_string());
        }
    }
    Session::from_map(vars)
}

fn session_metadata(session: &Session) -> Vec<(String, String)> {
    session
        .variables()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
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
    fn client_facing_message_preserves_client_errors() {
        let error = HandlerError::Rejected("invalid command".into());

        assert_eq!(error.client_facing_message(), "rejected: invalid command");
    }

    #[test]
    fn client_facing_message_hides_server_errors() {
        let errors = [
            HandlerError::Repository(RepositoryError::Model("store failed".into())),
            HandlerError::Other(Box::new(std::io::Error::other("handler failed"))),
        ];

        for error in errors {
            assert_eq!(error.client_facing_message(), "Internal server error");
        }
    }

    #[cfg(feature = "failure-logs")]
    #[tokio::test]
    async fn command_handler_masks_5xx_and_emits_sanitized_failure_event() {
        use crate::microsvc::Routes;
        use axum::body;
        use axum::http::HeaderValue;

        let service = Arc::new(
            Service::new().named("orders").routes(
                Routes::new()
                    .with_dependencies(())
                    .command("orders.create")
                    .handle(|_: &crate::microsvc::Context<()>| async move {
                        Err(HandlerError::Repository(RepositoryError::Model(
                            "database failed password=hunter2 postgres://user:pass@db/app".into(),
                        )))
                    }),
            ),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer raw-token"),
        );
        headers.insert("cookie", HeaderValue::from_static("session=raw-cookie"));
        headers.insert(
            "traceparent",
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        headers.insert("tracestate", HeaderValue::from_static("vendor=raw-state"));
        let capture = crate::failure_log::testing::capture_failures();

        let response = command_handler(
            State(service),
            Path("orders.create".to_string()),
            headers,
            Json(json!({ "request_body": "payload-secret" })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body, json!({ "error": "Internal server error" }));

        let events = capture.failure_events();
        let http = events
            .iter()
            .find(|event| event.field("distributed.component") == Some("http"))
            .expect("http failure event");
        assert_eq!(http.field("distributed.failure.action"), Some("return_500"));
        assert_eq!(http.field("error.category"), Some("repository"));
        assert_eq!(
            http.field("trace.trace_id"),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );

        let rendered = format!("{events:?}");
        for forbidden in [
            "payload-secret",
            "raw-token",
            "raw-cookie",
            "raw-state",
            "hunter2",
            "postgres://user:pass@db/app",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "failure event leaked `{forbidden}`: {rendered}"
            );
        }
    }
}
