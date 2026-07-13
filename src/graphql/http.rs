//! Axum GraphQL router (POST /graphql, optional GraphiQL + websocket upgrade).

use std::sync::Arc;

use async_graphql::http::{GraphiQLSource, ALL_WEBSOCKET_PROTOCOLS};
use async_graphql::{Data, Executor, Request, Response as GqlResponse};
use async_graphql_axum::{
    GraphQLProtocol, GraphQLRequest, GraphQLResponse, GraphQLWebSocket,
};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use futures_util::stream::BoxStream;

use crate::microsvc::{Service, Session, MAX_HTTP_BODY_BYTES};

use super::engine::GraphqlEngine;
use super::identity::{resolve_session, AuthError, IdentityMode};

/// HTML for the GraphiQL IDE served on `GET /graphql` when GraphiQL is enabled.
///
/// Default identity headers (`x-role: user`, `x-user-id: demo`) are injected
/// only for local exploration. Production scaffolds use `OidcBearer` (D6) —
/// these headers are **not** a security mechanism. GraphiQL is off under
/// production env policy (`graphiql_enabled_from_env`).
///
/// Subscriptions use the same `/graphql` path over WebSocket (`graphql-ws` /
/// `graphql-transport-ws`).
pub fn graphiql_page() -> Html<String> {
    Html(
        GraphiQLSource::build()
            .endpoint("/graphql")
            // WebSocket subscriptions (graphql-ws / graphql-transport-ws).
            .subscription_endpoint("/graphql/ws")
            .header("x-role", "user")
            .header("x-user-id", "demo")
            .title("Distributed GraphQL")
            .finish(),
    )
}

/// [`Executor`] that runs against a fixed session (for WebSocket subscriptions).
#[derive(Clone)]
pub struct GraphqlSessionExecutor {
    engine: Arc<GraphqlEngine>,
    session: Session,
}

impl GraphqlSessionExecutor {
    pub fn new(engine: Arc<GraphqlEngine>, session: Session) -> Self {
        Self { engine, session }
    }
}

impl Executor for GraphqlSessionExecutor {
    async fn execute(&self, request: Request) -> GqlResponse {
        self.engine.execute(&self.session, request).await
    }

    fn execute_stream(
        &self,
        request: Request,
        _session_data: Option<Arc<Data>>,
    ) -> BoxStream<'static, GqlResponse> {
        self.engine.execute_stream(&self.session, request)
    }
}

/// Standalone GraphQL router with its own body limit.
pub fn graphql_router(engine: Arc<GraphqlEngine>) -> Router {
    let graphiql = engine.graphiql_enabled();
    let mut router = Router::new().route(
        "/graphql",
        post(graphql_handler).get(move || async move {
            if graphiql {
                graphiql_page().into_response()
            } else {
                axum::http::StatusCode::METHOD_NOT_ALLOWED.into_response()
            }
        }),
    );
    router = router.layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES));
    router.with_state(engine)
}

/// GraphQL router that can dispatch command mutations through a [`Service`].
pub fn graphql_router_with_service(engine: Arc<GraphqlEngine>, service: Arc<Service>) -> Router {
    // Validate command names are registered.
    let registered: std::collections::HashSet<String> = service
        .command_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let mut missing = Vec::new();
    for name in engine.inner.commands.command_names() {
        if !registered.contains(name) {
            missing.push(name.to_string());
        }
    }
    if !missing.is_empty() {
        panic!(
            "graphql command mutations reference unregistered commands: {}",
            missing.join(", ")
        );
    }

    let graphiql = engine.graphiql_enabled();
    let state = GraphqlHttpState {
        engine,
        service: Some(service),
    };
    let mut router = Router::new().route(
        "/graphql",
        post(graphql_handler_with_service).get(move || async move {
            if graphiql {
                graphiql_page().into_response()
            } else {
                axum::http::StatusCode::METHOD_NOT_ALLOWED.into_response()
            }
        }),
    );
    router = router.layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES));
    router.with_state(state)
}

#[derive(Clone)]
struct GraphqlHttpState {
    engine: Arc<GraphqlEngine>,
    service: Option<Arc<Service>>,
}

fn unauthorized_response() -> Response {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [("content-type", "application/json")],
        r#"{"errors":[{"message":"unauthorized","extensions":{"code":"UNAUTHENTICATED"}}]}"#,
    )
        .into_response()
}

async fn graphql_handler(
    State(engine): State<Arc<GraphqlEngine>>,
    headers: axum::http::HeaderMap,
    req: GraphQLRequest,
) -> Response {
    let session = match resolve_session(&headers, engine.identity_config()).await {
        Ok(s) => s,
        Err(AuthError::Unauthorized) => return unauthorized_response(),
    };
    // GraphiQL demo headers only apply under DevHeaders; other modes ignore them for AuthZ.
    let _ = engine.identity_config().mode == IdentityMode::DevHeaders;
    let response = engine.execute(&session, req.into_inner()).await;
    GraphQLResponse::from(response).into_response()
}

async fn graphql_handler_with_service(
    State(state): State<GraphqlHttpState>,
    headers: axum::http::HeaderMap,
    req: GraphQLRequest,
) -> Response {
    let session = match resolve_session(&headers, state.engine.identity_config()).await {
        Ok(s) => s,
        Err(AuthError::Unauthorized) => return unauthorized_response(),
    };
    let mut request = req.into_inner();
    if let Some(service) = &state.service {
        request = request.data(Arc::clone(service));
    }
    let response = state.engine.execute(&session, request).await;
    GraphQLResponse::from(response).into_response()
}

/// Handler used when GraphQL is mounted on the microsvc router.
pub async fn microsvc_graphql_handler(
    State(service): State<Arc<Service>>,
    headers: axum::http::HeaderMap,
    req: GraphQLRequest,
) -> Response {
    let engine = service
        .graphql_engine()
        .expect("graphql route mounted without engine");
    let session = match resolve_session(&headers, engine.identity_config()).await {
        Ok(s) => s,
        Err(AuthError::Unauthorized) => return unauthorized_response(),
    };
    let request = req.into_inner().data(Arc::clone(&service));
    let response = engine.execute(&session, request).await;
    GraphQLResponse::from(response).into_response()
}

/// `GET /graphql` — GraphiQL HTML when not a WebSocket upgrade.
///
/// WebSocket upgrades are handled by [`microsvc_graphql_ws`] on the same path
/// (registered separately via `on_upgrade` routing in microsvc).
pub async fn microsvc_graphql_get(State(service): State<Arc<Service>>) -> Response {
    let graphiql = service
        .graphql_engine()
        .map(|e| e.graphiql_enabled())
        .unwrap_or(false);
    if graphiql {
        graphiql_page().into_response()
    } else {
        StatusCode::METHOD_NOT_ALLOWED.into_response()
    }
}

/// WebSocket upgrade for GraphQL subscriptions (`graphql-ws` / `graphql-transport-ws`).
///
/// Identity: HTTP headers on the upgrade request (DevHeaders / Bearer), plus
/// browser-friendly query params `x-user-id` / `x-role` (browsers cannot set
/// custom WebSocket headers).
pub async fn microsvc_graphql_ws(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    protocol: GraphQLProtocol,
    upgrade: WebSocketUpgrade,
) -> Response {
    let engine = match service.graphql_engine() {
        Some(e) => e,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    // Merge query-string identity into a header map for resolve_session.
    let mut headers = headers;
    if let Some(q) = uri.query() {
        for pair in q.split('&') {
            let mut it = pair.splitn(2, '=');
            let k = it.next().unwrap_or("");
            let v = it.next().unwrap_or("");
            let v = urlencoding_decode(v);
            match k {
                "x-user-id" if !headers.contains_key("x-user-id") => {
                    if let Ok(val) = axum::http::HeaderValue::from_str(&v) {
                        headers.insert("x-user-id", val);
                    }
                }
                "x-role" if !headers.contains_key("x-role") => {
                    if let Ok(val) = axum::http::HeaderValue::from_str(&v) {
                        headers.insert("x-role", val);
                    }
                }
                _ => {}
            }
        }
    }

    let session = match resolve_session(&headers, engine.identity_config()).await {
        Ok(s) => s,
        Err(AuthError::Unauthorized) => return unauthorized_response(),
    };

    let executor = GraphqlSessionExecutor::new(engine, session);
    upgrade
        .protocols(ALL_WEBSOCKET_PROTOCOLS)
        .on_upgrade(move |socket| GraphQLWebSocket::new(socket, executor, protocol).serve())
        .into_response()
}

fn urlencoding_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let h = |c: u8| -> Option<u8> {
                    match c {
                        b'0'..=b'9' => Some(c - b'0'),
                        b'a'..=b'f' => Some(c - b'a' + 10),
                        b'A'..=b'F' => Some(c - b'A' + 10),
                        _ => None,
                    }
                };
                if let (Some(hi), Some(lo)) = (h(b[i + 1]), h(b[i + 2])) {
                    out.push((hi << 4 | lo) as char);
                    i += 3;
                } else {
                    out.push('%');
                    i += 1;
                }
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
}
