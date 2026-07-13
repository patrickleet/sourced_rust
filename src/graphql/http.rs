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

use crate::microsvc::{Service, Session, MAX_HTTP_BODY_BYTES, ROLE_KEY, USER_ID_KEY};

use super::engine::GraphqlEngine;
use super::identity::{resolve_session, AuthError, IdentityMode};

/// DevHeaders baked into GraphiQL for local exploration (not production security).
const GRAPHIQL_DEV_USER: &str = "demo";
const GRAPHIQL_DEV_ROLE: &str = "user";

/// HTML for the GraphiQL IDE served on `GET /graphql` when GraphiQL is enabled.
///
/// Default identity headers (`x-role: user`, `x-user-id: demo`) are injected
/// only for local exploration. Production scaffolds use `OidcBearer` (D6) —
/// these headers are **not** a security mechanism. GraphiQL is off under
/// production env policy (`graphiql_enabled_from_env`).
///
/// **Subscriptions:** GraphiQL runs them over WebSocket at `/graphql/ws`.
/// Browsers cannot set custom WS headers; identity is supplied via
/// `wsConnectionParams` → `connection_init` (and DevHeaders defaults if empty).
///
/// Note: do **not** put `?x-user-id=` query params in `subscription_endpoint` —
/// GraphiQLSource HTML-escapes `=` / `&` (`&#x3D;`, `&amp;`), which breaks the URL.
pub fn graphiql_page() -> Html<String> {
    Html(
        GraphiQLSource::build()
            .endpoint("/graphql")
            .subscription_endpoint("/graphql/ws")
            .header(ROLE_KEY, GRAPHIQL_DEV_ROLE)
            .header(USER_ID_KEY, GRAPHIQL_DEV_USER)
            // Sent as connection_init payload for graphql-transport-ws.
            .ws_connection_param(USER_ID_KEY, GRAPHIQL_DEV_USER)
            .ws_connection_param(ROLE_KEY, GRAPHIQL_DEV_ROLE)
            .title("Distributed GraphQL")
            .finish(),
    )
}

/// [`Executor`] for WebSocket subscriptions.
///
/// Prefers a [`Session`] injected via `connection_init` / `session_data` (GraphiQL
/// wsConnectionParams), then falls back to the session from the HTTP upgrade.
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
        // Subscriptions must not go through execute() — that yields
        // "Subscription root not found". GraphiQL should use the WS path only.
        self.engine.execute(&self.session, request).await
    }

    fn execute_stream(
        &self,
        request: Request,
        session_data: Option<Arc<Data>>,
    ) -> BoxStream<'static, GqlResponse> {
        use std::any::TypeId;
        let session = session_data
            .as_ref()
            .and_then(|d| d.get(&TypeId::of::<Session>()))
            .and_then(|b| b.downcast_ref::<Session>())
            .cloned()
            .unwrap_or_else(|| self.session.clone());
        self.engine.execute_stream(&session, request)
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
/// **Auth best practice (OIDC):** browsers cannot set `Authorization` on the WS
/// upgrade. Accept the upgrade, then require a Bearer access token in
/// `connection_init` (payload `authorization` / `accessToken` / nested
/// `headers.Authorization`). Validate with the same OidcBearer path as HTTP.
///
/// **DevHeaders (local):** identity from upgrade headers, query params, or
/// GraphiQL `wsConnectionParams` (`x-user-id` / `x-role`).
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

    let mut upgrade_headers = headers;
    merge_identity_query_params(&mut upgrade_headers, uri.query());
    let mode = engine.identity_config().mode;

    // OidcBearer: do not fail the upgrade without a token — auth happens in
    // connection_init (graphql-ws best practice). DevHeaders: resolve now.
    let upgrade_session = if mode == IdentityMode::OidcBearer || mode == IdentityMode::Hybrid {
        Session::new()
    } else {
        match resolve_session(&upgrade_headers, engine.identity_config()).await {
            Ok(s) => ensure_graphiql_dev_session(s, mode),
            Err(AuthError::Unauthorized) => return unauthorized_response(),
        }
    };

    let executor = GraphqlSessionExecutor::new(Arc::clone(&engine), upgrade_session.clone());
    let engine_for_init = Arc::clone(&engine);
    upgrade
        .protocols(ALL_WEBSOCKET_PROTOCOLS)
        .on_upgrade(move |socket| {
            let base = upgrade_session;
            let upgrade_headers = upgrade_headers;
            GraphQLWebSocket::new(socket, executor, protocol)
                .on_connection_init(move |payload| {
                    let base = base.clone();
                    let engine = engine_for_init;
                    let upgrade_headers = upgrade_headers;
                    async move {
                        let session = match resolve_ws_session(
                            &engine,
                            &upgrade_headers,
                            base,
                            &payload,
                        )
                        .await
                        {
                            Ok(s) => s,
                            Err(msg) => {
                                return Err(async_graphql::Error::new(msg));
                            }
                        };
                        let mut data = Data::default();
                        data.insert(session);
                        Ok(data)
                    }
                })
                .serve()
        })
        .into_response()
}

/// Resolve session for a GraphQL WS connection after `connection_init`.
async fn resolve_ws_session(
    engine: &GraphqlEngine,
    upgrade_headers: &HeaderMap,
    base: Session,
    payload: &serde_json::Value,
) -> Result<Session, String> {
    let mode = engine.identity_config().mode;
    match mode {
        IdentityMode::OidcBearer | IdentityMode::Hybrid => {
            let mut headers = upgrade_headers.clone();
            if let Some(auth) = bearer_from_connection_init(payload) {
                if let Ok(val) = axum::http::HeaderValue::from_str(&auth) {
                    headers.insert(axum::http::header::AUTHORIZATION, val);
                }
            }
            resolve_session(&headers, engine.identity_config())
                .await
                .map_err(|_| {
                    "unauthorized: provide Authorization Bearer access_token in connection_init"
                        .into()
                })
        }
        IdentityMode::DevHeaders | IdentityMode::TrustedProxy => {
            Ok(session_from_connection_init(base, payload, mode))
        }
    }
}

/// Extract Bearer token from connection_init payload (several client conventions).
fn bearer_from_connection_init(payload: &serde_json::Value) -> Option<String> {
    let pick = |v: &serde_json::Value| -> Option<String> {
        let s = v.as_str()?.trim();
        if s.is_empty() {
            return None;
        }
        if s.len() > 7 && s[..7].eq_ignore_ascii_case("bearer ") {
            Some(s.to_string())
        } else {
            Some(format!("Bearer {s}"))
        }
    };
    if let Some(s) = payload
        .get("Authorization")
        .or_else(|| payload.get("authorization"))
        .and_then(pick)
    {
        return Some(s);
    }
    if let Some(s) = payload
        .get("accessToken")
        .or_else(|| payload.get("access_token"))
        .and_then(pick)
    {
        return Some(s);
    }
    if let Some(headers) = payload.get("headers").and_then(|h| h.as_object()) {
        if let Some(s) = headers
            .get("Authorization")
            .or_else(|| headers.get("authorization"))
            .and_then(pick)
        {
            return Some(s);
        }
    }
    None
}

fn ensure_graphiql_dev_session(mut session: Session, mode: IdentityMode) -> Session {
    if mode != IdentityMode::DevHeaders {
        return session;
    }
    if session.user_id().is_none() {
        session.set(USER_ID_KEY, GRAPHIQL_DEV_USER);
    }
    if session.role().is_none() {
        session.set(ROLE_KEY, GRAPHIQL_DEV_ROLE);
    }
    session
}

/// Merge `connection_init` / GraphiQL wsConnectionParams into the upgrade session.
fn session_from_connection_init(
    mut session: Session,
    payload: &serde_json::Value,
    mode: IdentityMode,
) -> Session {
    if mode != IdentityMode::DevHeaders {
        // OIDC: allow Authorization in connection_init for WS clients that put the
        // Bearer token there (GraphiQL Headers panel may not reach the upgrade).
        if let Some(auth) = payload
            .get("Authorization")
            .or_else(|| payload.get("authorization"))
            .and_then(|v| v.as_str())
        {
            session.set("authorization", auth);
        }
        if let Some(headers) = payload.get("headers").and_then(|h| h.as_object()) {
            if let Some(auth) = headers
                .get("Authorization")
                .or_else(|| headers.get("authorization"))
                .and_then(|v| v.as_str())
            {
                session.set("authorization", auth);
            }
        }
        return session;
    }

    let apply = |session: &mut Session, key: &str, val: &str| {
        if key.eq_ignore_ascii_case(USER_ID_KEY) || key.eq_ignore_ascii_case("x-user-id") {
            session.set(USER_ID_KEY, val);
        } else if key.eq_ignore_ascii_case(ROLE_KEY) || key.eq_ignore_ascii_case("x-role") {
            session.set(ROLE_KEY, val);
        }
    };

    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                apply(&mut session, k, s);
            }
        }
        if let Some(headers) = obj.get("headers").and_then(|h| h.as_object()) {
            for (k, v) in headers {
                if let Some(s) = v.as_str() {
                    apply(&mut session, k, s);
                }
            }
        }
    }

    ensure_graphiql_dev_session(session, mode)
}

fn merge_identity_query_params(headers: &mut HeaderMap, query: Option<&str>) {
    let Some(q) = query else {
        return;
    };
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = urlencoding_decode(it.next().unwrap_or(""));
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

#[cfg(test)]
mod connection_init_tests {
    use super::*;
    use crate::graphql::IdentityMode;
    use serde_json::json;

    #[test]
    fn connection_init_sets_dev_headers() {
        let session = session_from_connection_init(
            Session::new(),
            &json!({"x-user-id": "alice", "x-role": "user"}),
            IdentityMode::DevHeaders,
        );
        assert_eq!(session.user_id(), Some("alice"));
        assert_eq!(session.role(), Some("user"));
    }

    #[test]
    fn connection_init_nested_headers() {
        let session = session_from_connection_init(
            Session::new(),
            &json!({"headers": {"x-user-id": "bob", "x-role": "admin"}}),
            IdentityMode::DevHeaders,
        );
        assert_eq!(session.user_id(), Some("bob"));
        assert_eq!(session.role(), Some("admin"));
    }

    #[test]
    fn empty_init_gets_graphiql_defaults_in_dev() {
        let session =
            session_from_connection_init(Session::new(), &json!({}), IdentityMode::DevHeaders);
        assert_eq!(session.user_id(), Some(GRAPHIQL_DEV_USER));
        assert_eq!(session.role(), Some(GRAPHIQL_DEV_ROLE));
    }

    #[test]
    fn bearer_from_connection_init_shapes() {
        assert_eq!(
            bearer_from_connection_init(&json!({"authorization": "Bearer abc"})).as_deref(),
            Some("Bearer abc")
        );
        assert_eq!(
            bearer_from_connection_init(&json!({"accessToken": "tok"})).as_deref(),
            Some("Bearer tok")
        );
        assert_eq!(
            bearer_from_connection_init(&json!({"headers": {"Authorization": "Bearer z"}}))
                .as_deref(),
            Some("Bearer z")
        );
        assert!(bearer_from_connection_init(&json!({})).is_none());
    }
}
