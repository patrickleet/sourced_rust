//! Tower layer: validate Bearer (OidcBearer) and inject `x-user-id` / `x-role`
//! so command routes see the same identity as GraphQL.
//!
//! GraphQL already uses IdentityConfig; commands only read Session headers.
//! This layer bridges OIDC access tokens → DevHeaders-shaped session keys.

use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, Response};
use distributed::graphql::{resolve_session, IdentityConfig, IdentityMode};
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
            // Only OidcBearer/Hybrid need claim injection; DevHeaders already have x-user-id.
            if matches!(
                identity.mode,
                IdentityMode::OidcBearer | IdentityMode::Hybrid
            ) {
                if let Ok(session) = resolve_session(req.headers(), &identity).await {
                    if let Some(uid) = session.user_id() {
                        if let Ok(v) = axum::http::HeaderValue::from_str(uid) {
                            req.headers_mut().insert("x-user-id", v);
                        }
                    }
                    if let Some(role) = session.role() {
                        if let Ok(v) = axum::http::HeaderValue::from_str(role) {
                            req.headers_mut().insert("x-role", v);
                        }
                    }
                }
            }
            inner.call(req).await
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
