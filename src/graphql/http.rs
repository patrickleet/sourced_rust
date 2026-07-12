//! Axum GraphQL router (POST /graphql, optional GraphiQL + websocket upgrade).

use std::sync::Arc;

use async_graphql::http::GraphiQLSource;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::extract::{DefaultBodyLimit, State};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::post;
use axum::Router;

use crate::microsvc::{Service, MAX_HTTP_BODY_BYTES};

use super::engine::GraphqlEngine;
use super::identity::{resolve_session, AuthError, IdentityMode};

/// HTML for the GraphiQL IDE served on `GET /graphql` when GraphiQL is enabled.
///
/// Default identity headers (`x-role: user`, `x-user-id: demo`) are injected
/// only for local exploration. Production scaffolds use `OidcBearer` (D6) —
/// these headers are **not** a security mechanism. GraphiQL is off under
/// production env policy (`graphiql_enabled_from_env`).
pub fn graphiql_page() -> Html<String> {
    Html(
        GraphiQLSource::build()
            .endpoint("/graphql")
            .header("x-role", "user")
            .header("x-user-id", "demo")
            .title("Distributed GraphQL")
            .finish(),
    )
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
