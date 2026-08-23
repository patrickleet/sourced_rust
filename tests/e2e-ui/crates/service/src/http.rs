//! Process HTTP: GraphQL is the user edge (engine `OidcBearer`).
//!
//! App writes are GraphQL-only (HTTP command routes stay off). Zitadel Action
//! ingress still needs HTTP, so those two command names are mounted explicitly
//! — `POST /todo.create` stays 404 (suite T0).
//!
//! Note: `microsvc::router` already applies `.with_state(service)`, so handlers
//! cannot use `State<Arc<Service>>`. Capture the `Arc` in the route closures.

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Json;
use distributed::microsvc::{HandlerError, Service, Session};
use serde_json::{json, Value};

fn session_from_headers(headers: &HeaderMap) -> Session {
    let mut vars = HashMap::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            vars.insert(name.as_str().to_string(), v.to_string());
        }
    }
    Session::from_map(vars)
}

fn status_for_error(error: &HandlerError) -> StatusCode {
    match error {
        HandlerError::UnknownCommand(_) | HandlerError::NotFound(_) => StatusCode::NOT_FOUND,
        HandlerError::DecodeFailed(_) | HandlerError::GuardRejected(_) => StatusCode::BAD_REQUEST,
        HandlerError::Rejected(_) => StatusCode::UNPROCESSABLE_ENTITY,
        HandlerError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
        HandlerError::Repository(_) | HandlerError::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn dispatch_named(
    service: Arc<Service>,
    headers: HeaderMap,
    input: Value,
    command: &'static str,
) -> impl IntoResponse {
    let session = session_from_headers(&headers);
    match service.dispatch(command, input, session).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => {
            let status = status_for_error(&err);
            if status.is_server_error() {
                eprintln!("microsvc command `{command}` failed: {err}");
            }
            let body = json!({ "error": err.client_facing_message() });
            (status, Json(body)).into_response()
        }
    }
}

/// Serve GraphQL (engine identity) plus Zitadel Action HTTP.
pub async fn serve(service: Arc<Service>, addr: &str) -> Result<(), std::io::Error> {
    let ingress = service.clone();
    let scrape = service.clone();
    let app = distributed::microsvc::router(service)
        .route(
            "/zitadel.ingress.v1",
            post(move |headers: HeaderMap, Json(input): Json<Value>| {
                let svc = ingress.clone();
                async move { dispatch_named(svc, headers, input, "zitadel.ingress.v1").await }
            }),
        )
        .route(
            "/zitadel.scrape.v1",
            post(move |headers: HeaderMap, Json(input): Json<Value>| {
                let svc = scrape.clone();
                async move { dispatch_named(svc, headers, input, "zitadel.scrape.v1").await }
            }),
        );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
