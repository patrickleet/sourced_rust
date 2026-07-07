//! Shared telemetry middleware for Distributed-owned HTTP routers.

#[cfg(feature = "metrics")]
use std::time::Instant;

#[cfg(any(feature = "metrics", feature = "otel"))]
use axum::extract::MatchedPath;
use axum::extract::{Request, State};
#[cfg(any(feature = "metrics", feature = "otel"))]
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;

#[derive(Clone, Debug, Default)]
pub(crate) struct HttpTelemetryState {
    #[cfg(feature = "metrics")]
    service: Option<String>,
}

impl HttpTelemetryState {
    pub(crate) fn new(service: Option<String>) -> Self {
        #[cfg(not(feature = "metrics"))]
        let _ = service;
        #[cfg(not(feature = "metrics"))]
        return Self {};

        #[cfg(feature = "metrics")]
        Self { service }
    }

    #[cfg(feature = "metrics")]
    fn service(&self) -> Option<&str> {
        self.service.as_deref()
    }
}

pub(crate) async fn middleware(
    State(state): State<HttpTelemetryState>,
    req: Request,
    next: Next,
) -> Response {
    #[cfg(not(any(feature = "metrics", feature = "otel")))]
    {
        let _ = state;
        return next.run(req).await;
    }

    #[cfg(any(feature = "metrics", feature = "otel"))]
    {
        #[cfg(not(feature = "metrics"))]
        let _ = state;

        let method = normalize_method(req.method());
        let route = route_label(req.extensions().get::<MatchedPath>());
        #[cfg(feature = "metrics")]
        let started = Instant::now();

        #[cfg(feature = "otel")]
        let span = http_server_span(method, route);
        #[cfg(feature = "otel")]
        crate::trace_context::set_span_parent_from_headers_if_no_current_span(&span, req.headers());

        #[cfg(feature = "otel")]
        let response = {
            use tracing::Instrument as _;

            next.run(req).instrument(span.clone()).await
        };
        #[cfg(not(feature = "otel"))]
        let response = next.run(req).await;

        let status = response.status();
        #[cfg(feature = "otel")]
        span.record("http.response.status_code", i64::from(status.as_u16()));
        #[cfg(feature = "metrics")]
        crate::metrics::record_http_server_request(
            state.service(),
            method,
            route,
            status.as_u16(),
            started.elapsed(),
        );

        response
    }
}

#[cfg(any(feature = "metrics", feature = "otel"))]
fn normalize_method(method: &Method) -> &'static str {
    match method.as_str() {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "OPTIONS" => "OPTIONS",
        "TRACE" => "TRACE",
        "CONNECT" => "CONNECT",
        _ => "_OTHER",
    }
}

#[cfg(any(feature = "metrics", feature = "otel"))]
fn route_label(matched_path: Option<&MatchedPath>) -> &'static str {
    match matched_path.map(MatchedPath::as_str) {
        Some("/health") => "/health",
        Some("/metrics") => "/metrics",
        Some("/{command}") => "/{command}",
        Some("/") => "/",
        Some("/cloudevent/{type}") => "/cloudevent/{type}",
        _ => "unmatched",
    }
}

#[cfg(feature = "otel")]
fn http_server_span(method: &'static str, route: &'static str) -> tracing::Span {
    let span_name = format!("{method} {route}");
    tracing::info_span!(
        "distributed.http.server",
        otel.name = span_name.as_str(),
        otel.kind = "server",
        http.request.method = method,
        http.route = route,
        http.response.status_code = tracing::field::Empty,
        network.protocol.name = "http",
    )
}
