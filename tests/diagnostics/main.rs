#![cfg(all(feature = "http", feature = "metrics"))]

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{
    run_source, Handlers, MessagePublisher, MessageSource, ReceivedMessage, RunOptions,
    TransportError,
};
use distributed::diagnostics::{Diagnostics, DiagnosticsOptions, DEFAULT_DIAGNOSTICS_PATH};
use distributed::microsvc::{self, Context, Message, MessageKind, Routes, Service, Session};
use distributed::{
    CommitBatch, InMemoryRepository, OutboxDispatcher, OutboxMessage, TransactionalCommit,
};
use serde_json::json;

const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

fn orders_service(name: &str) -> Arc<Service> {
    Arc::new(
        Service::new().named(name).routes(
            Routes::new()
                .with_dependencies(())
                .command("orders.create")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({ "ok": true })) }),
        ),
    )
}

async fn start_app(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn default_router_does_not_expose_diagnostics() {
    let base = start_app(microsvc::router(orders_service("diag-disabled"))).await;
    let response = reqwest::Client::new()
        .get(format!("{base}{DEFAULT_DIAGNOSTICS_PATH}"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn diagnostics_route_requires_configured_access_hook_and_disables_caching() {
    let service = orders_service("diag-http-auth");
    let base = start_app(microsvc::router_with_diagnostics(
        service,
        DiagnosticsOptions::new().with_bearer_token("correct-token"),
    ))
    .await;
    let client = reqwest::Client::new();

    let unauthorized = client
        .get(format!("{base}{DEFAULT_DIAGNOSTICS_PATH}"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);
    assert_eq!(
        unauthorized
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    let authorized = client
        .get(format!("{base}{DEFAULT_DIAGNOSTICS_PATH}"))
        .bearer_auth("correct-token")
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), 200);
    assert_eq!(
        authorized
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body: serde_json::Value = authorized.json().await.unwrap();
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["telemetry"]["diagnostics"]["visibility"], "private");
}

struct SecretFailPublisher;

impl MessagePublisher for SecretFailPublisher {
    async fn publish(&self, message: Message) -> Result<(), TransportError> {
        if message.id().is_some_and(|id| id.contains("poison")) {
            return Err(TransportError::retryable(
                "DATABASE_URL=postgres://user:pass@localhost/db token=super-secret",
            ));
        }
        Ok(())
    }
}

struct OneShotSource {
    queue: VecDeque<Message>,
}

impl OneShotSource {
    fn new(message: Message) -> Self {
        Self {
            queue: VecDeque::from([message]),
        }
    }
}

impl MessageSource for OneShotSource {
    type Received = OneShotReceived;

    fn transport_name(&self) -> &'static str {
        "nats"
    }

    async fn recv(&mut self) -> Result<Option<Self::Received>, TransportError> {
        Ok(self
            .queue
            .pop_front()
            .map(|message| OneShotReceived { message }))
    }
}

struct OneShotReceived {
    message: Message,
}

impl ReceivedMessage for OneShotReceived {
    fn message(&self) -> &Message {
        &self.message
    }

    async fn ack(self) -> Result<(), TransportError> {
        Ok(())
    }

    async fn nack(self, _reason: &str) -> Result<(), TransportError> {
        Ok(())
    }
}

#[tokio::test]
async fn diagnostics_snapshot_agrees_with_metrics_backlog_and_redacts_private_data() {
    let service_name = "diag-integration";
    let service = orders_service(service_name);
    let repo = InMemoryRepository::new();
    std::env::set_var("DISTRIBUTED_DIAGNOSTICS_SECRET", "env-secret");

    service
        .dispatch(
            "orders.create",
            json!({ "id": "ok", "decoded": "decoded-input-secret" }),
            Session::new(),
        )
        .await
        .unwrap();
    let mut session = std::collections::HashMap::new();
    session.insert("x-hasura-user-id".to_string(), "session-secret".to_string());
    session.insert(
        "authorization".to_string(),
        "Bearer auth-secret".to_string(),
    );
    let _ = service
        .dispatch(
            "orders.missing",
            json!({ "payload": "payload-secret" }),
            Session::from_map(session),
        )
        .await;

    let mut batch = CommitBatch::empty();
    for id in ["evt-ok", "evt-poison"] {
        batch
            .outbox_messages
            .push(OutboxMessage::create(id, "orders.created", b"payload-secret".to_vec()).unwrap());
    }
    repo.commit_batch(batch).await.unwrap();
    let dispatcher = OutboxDispatcher::new(
        repo.outbox_store(),
        SecretFailPublisher,
        "diag-worker",
        Duration::from_secs(60),
        3,
    )
    .with_service(service_name);
    let outcome = dispatcher.dispatch_batch(10).await.unwrap();
    assert_eq!(outcome.published, 1);
    assert_eq!(outcome.released, 1);

    let transport_message = Message::new(
        "orders.created",
        MessageKind::Event,
        br#"{ "payload": "payload-secret" }"#.to_vec(),
    )
    .with_metadata("correlation_id", "corr-123")
    .with_metadata("causation_id", "cause-456")
    .with_metadata("traceparent", TRACEPARENT)
    .with_metadata("authorization", "Bearer auth-secret")
    .with_metadata("cookie", "cookie-secret")
    .with_metadata("x-raw", "raw-meta-secret");
    let handlers = Arc::new(Handlers::new().named(service_name).on_event(
        "orders.created",
        |_message: &Message| async move {
            Err(TransportError::retryable(
                "nats timeout token=transport-secret",
            ))
        },
    ));
    run_source(
        handlers,
        OneShotSource::new(transport_message),
        RunOptions::idempotent(),
    )
    .await
    .unwrap();

    let diagnostics = Diagnostics::new(
        DiagnosticsOptions::new()
            .with_outbox_store(repo.outbox_store())
            .with_transport("http")
            .with_transport("nats"),
    );
    let snapshot = diagnostics.snapshot(&service).await;

    assert_eq!(snapshot.schema_version, 1);
    assert!(snapshot
        .service
        .commands
        .contains(&"orders.create".to_string()));
    assert!(snapshot.telemetry.metrics.enabled);
    assert!(snapshot
        .telemetry
        .metrics
        .families
        .contains(&"distributed_microsvc_dispatch_total".to_string()));
    assert_eq!(snapshot.backlogs.outbox.pending, Some(1));
    assert!(snapshot
        .recent_failures
        .iter()
        .any(|failure| failure.kind == "microsvc"));
    assert!(snapshot
        .recent_failures
        .iter()
        .any(|failure| failure.kind == "transport"));
    assert!(snapshot
        .recent_failures
        .iter()
        .any(|failure| failure.kind == "outbox"));
    assert!(snapshot
        .causal_hints
        .last_trace_ids
        .contains(&"4bf92f3577b34da6a3ce929d0e0e4736".to_string()));
    assert!(snapshot
        .causal_hints
        .last_correlation_ids
        .contains(&"corr-123".to_string()));
    assert!(snapshot
        .actions
        .iter()
        .any(|action| action.id == "outbox_backlog_present"));

    let metrics = distributed::metrics::prometheus_text();
    assert!(
        metrics.contains("distributed_outbox_pending_messages{service=\"diag-integration\"} 1"),
        "metrics should agree with diagnostics backlog:\n{metrics}"
    );
    assert!(
        metrics.contains("message=\"unknown\""),
        "unknown command metric should use the bounded label:\n{metrics}"
    );

    let json = serde_json::to_string(&snapshot).unwrap();
    for forbidden in [
        "payload-secret",
        "decoded-input-secret",
        "session-secret",
        "auth-secret",
        "cookie-secret",
        "raw-meta-secret",
        "transport-secret",
        "super-secret",
        "postgres://user:pass",
        "DATABASE_URL",
        "env-secret",
        "authorization",
        "cookie",
        "x-hasura",
    ] {
        assert!(
            !json.contains(forbidden),
            "diagnostics leaked forbidden value `{forbidden}`:\n{json}"
        );
    }
    assert!(json.len() <= snapshot.snapshot.limits.max_response_bytes);
}
