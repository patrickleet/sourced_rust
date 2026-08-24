//! Process HTTP: GraphQL is the user edge (engine `OidcBearer`).
//!
//! Zitadel Action ingress/scrape and cell outbox drain are internal — shared
//! secret or process-local, not a user Bearer. User writes stay on `/graphql`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use distributed::cell_host::{
    InternalHttpSecret, CELL_INTERNAL_SECRET_HEADER, CELL_OUTBOX_DRAIN_PATH,
};
use distributed::command_dispatch::SharedCommandHost;
use distributed::graphql::graphql_router_with_host;
use distributed::microsvc::{HandlerError, Service, Session};
use futures_util::future::BoxFuture;
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

async fn require_internal(
    State(secret): State<InternalHttpSecret>,
    request: Request,
    next: Next,
) -> Response {
    if authorized_internal(request.headers(), &secret) {
        return next.run(request).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "code": "UNAUTHORIZED", "error": "unauthorized" })),
    )
        .into_response()
}

fn authorized_internal(headers: &HeaderMap, secret: &InternalHttpSecret) -> bool {
    headers
        .get(CELL_INTERNAL_SECRET_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|candidate| secret.matches(candidate))
}

/// Cell alarm POSTs pending outbox here; GraphQL publishes via MessagePublisher.
pub type InternalOutboxDrain =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// GraphQL wait-dispatches through an explicit [`SharedCommandHost`].
///
/// Identity is the engine's (`OidcBearer` on `POST /graphql` / WS `connection_init`).
/// HTTP command routes stay off — `POST /todo.create` is 404.
pub async fn serve(
    service: Arc<Service>,
    host: SharedCommandHost,
    addr: &str,
    outbox_drain: Option<InternalOutboxDrain>,
    internal_secret: InternalHttpSecret,
) -> Result<(), std::io::Error> {
    let engine = service
        .graphql_engine()
        .ok_or_else(|| std::io::Error::other("serve requires GraphQL"))?;
    let ingress = service.clone();
    let scrape = service.clone();
    let commands: Vec<String> = service
        .command_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let health_body = json!({
        "ok": true,
        "profile": "celld",
        "graphql": true,
        "commands": commands,
    });
    let mut app = Router::new()
        .route(
            "/health",
            get(move || {
                let body = health_body.clone();
                async move { Json(body) }
            }),
        )
        .merge(graphql_router_with_host(engine, host));
    let mut internal = Router::new()
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
    if let Some(drain) = outbox_drain {
        internal = internal.route(
            CELL_OUTBOX_DRAIN_PATH,
            post(move |Json(body): Json<Value>| {
                let drain = Arc::clone(&drain);
                async move {
                    match drain(body).await {
                        Ok(()) => (
                            StatusCode::ACCEPTED,
                            Json(json!({ "ok": true })),
                        ),
                        Err(error)
                            if error.contains("capacity") || error.contains("not running") =>
                        {
                            (
                                StatusCode::SERVICE_UNAVAILABLE,
                                Json(json!({ "code": "UNAVAILABLE", "error": "outbox scheduler unavailable" })),
                            )
                        }
                        Err(_) => (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "code": "BAD_REQUEST", "error": "invalid outbox hint" })),
                        ),
                    }
                }
            }),
        );
    }
    app = app.merge(internal.route_layer(middleware::from_fn_with_state(
        internal_secret,
        require_internal,
    )));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
