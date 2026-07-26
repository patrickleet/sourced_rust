use super::api::{describe_service, prometheus_text};

const PROMETHEUS_TEXT_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Build an axum router that exposes only `GET /metrics`.
///
/// This is intended for workers and services whose primary transport is not
/// HTTP. Run it on a small side port so Prometheus can scrape the same
/// framework metrics that the bus/outbox/runtime paths record. The endpoint is
/// unauthenticated; bind it only on a private listener or behind equivalent
/// network controls.
#[cfg(feature = "http")]
pub fn http_router() -> axum::Router {
    http_router_with_state(MetricsHttpState::default())
}

/// Build an axum router that exposes only `GET /metrics` and records a stable
/// service label before each scrape.
#[cfg(feature = "http")]
pub fn http_router_for_service(service: impl Into<String>) -> axum::Router {
    http_router_with_state(MetricsHttpState {
        service: Some(service.into()),
    })
}

/// Serve only the metrics scrape endpoint at the given address.
///
/// This helper is deliberately independent of `microsvc::http`, so a NATS,
/// Kafka, RabbitMQ, or outbox worker can expose Prometheus metrics without
/// exposing command dispatch over HTTP. The endpoint is unauthenticated; do not
/// bind it on a public interface unless an ingress or network policy restricts
/// access.
#[cfg(feature = "http")]
pub async fn serve_http(addr: &str, service: Option<&str>) -> Result<(), std::io::Error> {
    let app = match service {
        Some(service) => http_router_for_service(service),
        None => http_router(),
    };
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

/// Return a Prometheus text response for HTTP handlers.
#[cfg(feature = "http")]
pub fn prometheus_response(service: Option<&str>) -> impl axum::response::IntoResponse {
    describe_service(service);
    (
        [(
            axum::http::header::CONTENT_TYPE,
            PROMETHEUS_TEXT_CONTENT_TYPE,
        )],
        prometheus_text(),
    )
}

#[derive(Clone, Default)]
struct MetricsHttpState {
    service: Option<String>,
}

fn http_router_with_state(state: MetricsHttpState) -> axum::Router {
    axum::Router::new()
        .route("/metrics", axum::routing::get(metrics_http_handler))
        .with_state(state)
}

async fn metrics_http_handler(
    axum::extract::State(state): axum::extract::State<MetricsHttpState>,
) -> impl axum::response::IntoResponse {
    prometheus_response(state.service.as_deref())
}
