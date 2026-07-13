//! Tower layer: under OidcBearer/Hybrid, **require** a valid access token and
//! inject claim-derived `x-user-id` / `x-role` for command routes.
//!
//! Security: client-supplied identity headers are stripped before validation so
//! spoofed `x-user-id` cannot pass when Bearer is missing or invalid.
//! GraphQL already uses IdentityConfig; commands only read Session headers —
//! this layer bridges OIDC → DevHeaders-shaped keys for handlers.

use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{header, Method, Request, Response, StatusCode};
use distributed::graphql::{
    resolve_session, AuthError, IdentityConfig, IdentityMode, DEFAULT_IDENTITY_STRIP_HEADERS,
};
use futures_util::future::BoxFuture;
use tower::{Layer, Service};

#[derive(Clone)]
pub struct OidcIdentityLayer {
    identity: IdentityConfig,
}

impl OidcIdentityLayer {
    pub fn new(identity: IdentityConfig) -> Self {
        Self { identity }
    }
}

impl<S> Layer<S> for OidcIdentityLayer {
    type Service = OidcIdentityService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        OidcIdentityService {
            inner,
            identity: self.identity.clone(),
        }
    }
}

#[derive(Clone)]
pub struct OidcIdentityService<S> {
    inner: S,
    identity: IdentityConfig,
}

fn skip_oidc_gate(method: &Method, path: &str) -> bool {
    // Public probes + GraphiQL HTML + WS upgrade (auth on connection_init).
    matches!(path, "/health" | "/metrics" | "/graphql/ws")
        || (path == "/graphql" && *method == Method::GET)
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
fn strip_client_identity(headers: &mut axum::http::HeaderMap) {
    for name in DEFAULT_IDENTITY_STRIP_HEADERS {
        headers.remove(*name);
    }
    // Also strip common casing variants axum may have normalized differently.
    headers.remove("x-user-id");
    headers.remove("x-role");
    headers.remove("x-roles");
}

impl<S> Service<Request<Body>> for OidcIdentityService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
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
        let identity = self.identity.clone();
        Box::pin(async move {
            let path = req.uri().path().to_string();
            let method = req.method().clone();

            if !matches!(
                identity.mode,
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

            match resolve_session(req.headers(), &identity).await {
                Ok(session) => {
                    if let Some(uid) = session.user_id() {
                        if let Ok(v) = axum::http::HeaderValue::from_str(uid) {
                            req.headers_mut().insert("x-user-id", v);
                        }
                    }
                    if let Some(role) = session.role() {
                        if let Ok(v) = axum::http::HeaderValue::from_str(role) {
                            req.headers_mut().insert("x-role", v);
                        }
                    } else if session.user_id().is_some() {
                        // Authenticated but no role claim → default user.
                        req.headers_mut().insert(
                            "x-role",
                            axum::http::HeaderValue::from_static("user"),
                        );
                    }
                    inner.call(req).await
                }
                Err(AuthError::Unauthorized) => Ok(unauthorized_response()),
            }
        })
    }
}

/// Serve with OIDC identity injection on all routes (commands + GraphQL).
pub async fn serve_with_oidc(
    service: Arc<distributed::microsvc::Service>,
    identity: IdentityConfig,
    addr: &str,
) -> Result<(), std::io::Error> {
    let app = distributed::microsvc::router(service).layer(OidcIdentityLayer::new(identity));
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
        assert!(!skip_oidc_gate(&Method::POST, "/todo.create"));
    }

    #[test]
    fn strip_removes_spoof_headers() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-user-id", "attacker".parse().unwrap());
        h.insert("x-role", "admin".parse().unwrap());
        h.insert("authorization", "Bearer tok".parse().unwrap());
        strip_client_identity(&mut h);
        assert!(!h.contains_key("x-user-id"));
        assert!(!h.contains_key("x-role"));
        // Authorization must survive for resolve_session
        assert!(h.contains_key("authorization"));
    }
}
