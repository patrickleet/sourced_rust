//! Tower layer: under OidcBearer/Hybrid, **require** a valid access token and
//! inject claim-derived `x-user-id` / `x-roles` for command routes.
//!
//! Security: client-supplied identity headers are stripped before validation so
//! spoofed `x-user-id` cannot pass when Bearer is missing or invalid.
//! GraphQL already uses IdentityConfig; commands only read Session headers —
//! this layer bridges OIDC → DevHeaders-shaped keys for handlers.

use std::collections::HashMap;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Json;
use distributed::graphql::{
    AuthError, IdentityConfig, IdentityMode, IdentityResolver, DEFAULT_IDENTITY_STRIP_HEADERS,
};
use distributed::microsvc::{HandlerError, Service, Session};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use tower::{Layer, Service as TowerService};

#[derive(Clone)]
pub struct OidcIdentityLayer {
    resolver: Arc<IdentityResolver>,
}

impl OidcIdentityLayer {
    pub fn new(identity: IdentityConfig) -> Self {
        Self {
            resolver: Arc::new(IdentityResolver::new(identity)),
        }
    }
}

impl<S> Layer<S> for OidcIdentityLayer {
    type Service = OidcIdentityService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        OidcIdentityService {
            inner,
            resolver: Arc::clone(&self.resolver),
        }
    }
}

#[derive(Clone)]
pub struct OidcIdentityService<S> {
    inner: S,
    resolver: Arc<IdentityResolver>,
}

fn skip_oidc_gate(method: &Method, path: &str) -> bool {
    // Public probes + GraphiQL HTML + WS upgrade (auth on connection_init).
    // Zitadel Action ingress uses shared-secret authenticity (not OIDC bearer).
    matches!(
        path,
        "/health" | "/metrics" | "/graphql/ws" | "/zitadel.ingress.v1" | "/zitadel.scrape.v1"
    ) || (path == "/graphql" && *method == Method::GET)
}

fn unauthorized_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"error":"unauthorized","extensions":{"code":"UNAUTHENTICATED"}}"#,
        ))
        .expect("401 response")
}

/// Strip client-supplied identity headers (same list as TrustedProxy defaults).
fn strip_client_identity(headers: &mut HeaderMap) {
    for name in DEFAULT_IDENTITY_STRIP_HEADERS {
        headers.remove(*name);
    }
    // Also strip common casing variants axum may have normalized differently.
    headers.remove("x-user-id");
    headers.remove("x-role");
    headers.remove("x-roles");
}

impl<S> TowerService<Request<Body>> for OidcIdentityService<S>
where
    S: TowerService<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        let resolver = Arc::clone(&self.resolver);
        Box::pin(async move {
            let path = req.uri().path().to_string();
            let method = req.method().clone();

            if !matches!(
                resolver.config().mode,
                IdentityMode::OidcBearer | IdentityMode::Hybrid
            ) {
                // DevHeaders: ambient headers trusted only for local/offline.
                return inner.call(req).await;
            }

            if skip_oidc_gate(&method, &path) {
                return inner.call(req).await;
            }

            // Fail closed: never trust client identity headers under OidcBearer.
            strip_client_identity(req.headers_mut());

            match resolver.resolve_session(req.headers()).await {
                Ok(session) => {
                    if let Some(uid) = session.user_id() {
                        if let Ok(v) = axum::http::HeaderValue::from_str(uid) {
                            req.headers_mut().insert("x-user-id", v);
                        }
                    }
                    let roles = session.roles();
                    if !roles.is_empty() {
                        let joined = roles.join(",");
                        if let Ok(v) = axum::http::HeaderValue::from_str(&joined) {
                            req.headers_mut().insert("x-roles", v);
                        }
                    }
                    // Authenticated with empty role set stays empty (anonymous-eligible
                    // surfaces only) — no synthetic default role injection.
                    inner.call(req).await
                }
                Err(AuthError::Unauthorized) => Ok(unauthorized_response()),
            }
        })
    }
}

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
        // HandlerError is non_exhaustive.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Dispatch a named HTTP command (Zitadel ingress/scrape only).
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

/// Serve with OIDC identity injection on all routes (commands + GraphQL).
///
/// App writes are GraphQL-only (`Service::without_http_command_routes`). Zitadel
/// Action ingress still needs HTTP, so those two command names are mounted
/// explicitly — `POST /todo.create` stays 404 (suite T0).
///
/// Note: `microsvc::router` already applies `.with_state(service)`, so handlers
/// cannot use `State<Arc<Service>>`. Capture the `Arc` in the route closures.
pub async fn serve_with_oidc(
    service: Arc<Service>,
    identity: IdentityConfig,
    addr: &str,
) -> Result<(), std::io::Error> {
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
        )
        .layer(OidcIdentityLayer::new(identity));

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_gate_paths() {
        assert!(skip_oidc_gate(&Method::GET, "/health"));
        assert!(skip_oidc_gate(&Method::GET, "/graphql/ws"));
        assert!(skip_oidc_gate(&Method::GET, "/graphql"));
        assert!(!skip_oidc_gate(&Method::POST, "/graphql"));
        // Zitadel Action ingress + scrape use shared secret, not OIDC bearer.
        assert!(skip_oidc_gate(&Method::POST, "/zitadel.ingress.v1"));
        assert!(skip_oidc_gate(&Method::POST, "/zitadel.scrape.v1"));
        // Other HTTP command routes still require OIDC under OidcBearer.
        assert!(!skip_oidc_gate(&Method::POST, "/todo.create"));
        assert!(!skip_oidc_gate(&Method::POST, "/graphql"));
    }

    #[test]
    fn strip_removes_spoof_headers() {
        let mut h = HeaderMap::new();
        h.insert("x-user-id", "attacker".parse().unwrap());
        h.insert("x-roles", "admin".parse().unwrap());
        h.insert("x-role", "admin".parse().unwrap()); // legacy spoof — still stripped
        h.insert("authorization", "Bearer tok".parse().unwrap());
        strip_client_identity(&mut h);
        assert!(!h.contains_key("x-user-id"));
        assert!(!h.contains_key("x-roles"));
        assert!(!h.contains_key("x-role"));
        // Authorization must survive for resolve_session
        assert!(h.contains_key("authorization"));
    }
}
