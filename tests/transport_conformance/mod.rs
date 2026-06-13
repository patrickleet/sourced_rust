//! Reusable async-transport conformance harness.
//!
//! Adapter-neutral fakes plus a contract suite that proves the shared transport
//! behaviour (source runner ack/nack/failure ordering, stable-id handling, and
//! the outbox publisher/dispatcher thresholds) before any real broker exists.
//!
//! Concrete adapters reuse the pieces here: a real *source* adapter can be
//! exercised against [`FakePublisher`], a real *publisher* adapter against
//! [`FakeSource`], and any [`AsyncOutboxStore`] against the dispatcher contract.
//! Other test targets include this module with
//! `#[path = "../transport_conformance/mod.rs"] mod conformance;`.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use distributed::bus::{
    run_source, AsyncMessagePublisher, AsyncMessageSource, FailurePolicy, ReceivedMessage,
    RunOptions, TransportError,
};
use distributed::microsvc::{Context, HandlerError, Message, MessageKind, Service};
use distributed::OutboxDispatcher;
use distributed::{
    CommitBatch, HashMapOutboxStore, HashMapRepository, OutboxMessage, OutboxMessageStatus,
    TransactionalCommit,
};
use serde_json::json;

/// One observable transport effect, recorded in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A handler ran for the named message.
    Handled(String),
    Ack,
    Nack(String),
    DeadLetter(String),
    Park(String),
}

/// Ordered recorder shared by the service handlers and the fake transport.
#[derive(Default)]
pub struct Recorder {
    events: Mutex<Vec<Event>>,
}

impl Recorder {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn push(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

/// A received message that records how it was settled.
pub struct FakeReceived {
    message: Message,
    recorder: Arc<Recorder>,
    settle_ok: bool,
}

impl FakeReceived {
    fn settle(self, event: Event) -> Result<(), TransportError> {
        self.recorder.push(event);
        if self.settle_ok {
            Ok(())
        } else {
            Err(TransportError::retryable("settle failed"))
        }
    }
}

impl ReceivedMessage for FakeReceived {
    fn message(&self) -> &Message {
        &self.message
    }
    async fn ack(self) -> Result<(), TransportError> {
        self.settle(Event::Ack)
    }
    async fn nack(self, reason: &str) -> Result<(), TransportError> {
        self.settle(Event::Nack(reason.to_string()))
    }
    async fn dead_letter(self, reason: &str) -> Result<(), TransportError> {
        self.settle(Event::DeadLetter(reason.to_string()))
    }
    async fn park(self, reason: &str) -> Result<(), TransportError> {
        self.settle(Event::Park(reason.to_string()))
    }
}

/// A source that yields a preset queue of messages, then `None`.
pub struct FakeSource {
    queue: VecDeque<Message>,
    recorder: Arc<Recorder>,
    settle_ok: bool,
    recv_error: bool,
}

impl FakeSource {
    pub fn new(recorder: Arc<Recorder>, messages: Vec<Message>) -> Self {
        Self {
            queue: messages.into_iter().collect(),
            recorder,
            settle_ok: true,
            recv_error: false,
        }
    }
    pub fn with_settle_failure(mut self) -> Self {
        self.settle_ok = false;
        self
    }
    pub fn with_recv_error(mut self) -> Self {
        self.recv_error = true;
        self
    }
}

impl AsyncMessageSource for FakeSource {
    type Received = FakeReceived;
    async fn recv(&mut self) -> Result<Option<FakeReceived>, TransportError> {
        if self.recv_error {
            return Err(TransportError::retryable("recv failed"));
        }
        Ok(self.queue.pop_front().map(|message| FakeReceived {
            message,
            recorder: self.recorder.clone(),
            settle_ok: self.settle_ok,
        }))
    }
}

/// How a [`FakePublisher`] resolves each publish.
#[derive(Debug, Clone, Copy)]
pub enum PublishMode {
    Succeed,
    /// Unknown outcome: a retryable error (the row must stay retryable).
    FailUnknown,
}

/// A publisher that records published message ids and follows a [`PublishMode`].
pub struct FakePublisher {
    published: Mutex<Vec<String>>,
    mode: PublishMode,
}

impl FakePublisher {
    pub fn new(mode: PublishMode) -> Self {
        Self {
            published: Mutex::new(Vec::new()),
            mode,
        }
    }
    pub fn published_ids(&self) -> Vec<String> {
        self.published.lock().unwrap().clone()
    }
}

impl AsyncMessagePublisher for FakePublisher {
    async fn publish(&self, message: Message) -> Result<(), TransportError> {
        match self.mode {
            PublishMode::Succeed => {
                self.published
                    .lock()
                    .unwrap()
                    .push(message.id().unwrap_or_default().to_string());
                Ok(())
            }
            PublishMode::FailUnknown => Err(TransportError::retryable("publish outcome unknown")),
        }
    }
}

/// A service with conventional handlers: `ok` succeeds, `retryable` fails with a
/// retryable error, `permanent` fails with a permanent error. Each records that
/// it ran so tests can assert dispatch-before-ack ordering.
pub fn recording_service(recorder: &Arc<Recorder>) -> Arc<Service<()>> {
    let ok = recorder.clone();
    let retryable = recorder.clone();
    let permanent = recorder.clone();
    Arc::new(
        Service::new()
            .event("delivery.succeeded")
            .handle(move |ctx: &Context<()>| {
                ok.push(Event::Handled(ctx.message().name().to_string()));
                async move { Ok(json!({})) }
            })
            .event("delivery.retry_requested")
            .handle(move |ctx: &Context<()>| {
                retryable.push(Event::Handled(ctx.message().name().to_string()));
                async move { Err(HandlerError::Other("infra".into())) }
            })
            .event("delivery.permanently_failed")
            .handle(move |ctx: &Context<()>| {
                permanent.push(Event::Handled(ctx.message().name().to_string()));
                async move { Err(HandlerError::Rejected("nope".into())) }
            }),
    )
}

pub fn event_message(name: &str, id: Option<&str>) -> Message {
    let mut message = Message::new(name, MessageKind::Event, b"{}".to_vec());
    if let Some(id) = id {
        message = message.with_id(id);
    }
    message
}

// =============================================================================
// Source-runner contract
// =============================================================================

pub async fn source_dispatches_before_ack() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("delivery.succeeded", Some("m1"))],
    );
    run_source(service, source, RunOptions::idempotent())
        .await
        .unwrap();
    assert_eq!(
        recorder.events(),
        vec![Event::Handled("delivery.succeeded".into()), Event::Ack],
        "handler must run before ack"
    );
}

pub async fn source_retryable_failure_nacks_without_ack() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("delivery.retry_requested", Some("m1"))],
    );
    run_source(service, source, RunOptions::idempotent())
        .await
        .unwrap();
    let events = recorder.events();
    assert_eq!(
        events.first(),
        Some(&Event::Handled("delivery.retry_requested".into()))
    );
    assert!(matches!(events.get(1), Some(Event::Nack(_))));
    assert!(!events.contains(&Event::Ack));
}

pub async fn source_permanent_failure_dead_letters_by_default() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("delivery.permanently_failed", Some("m1"))],
    );
    run_source(service, source, RunOptions::idempotent())
        .await
        .unwrap();
    assert!(matches!(
        recorder.events().get(1),
        Some(Event::DeadLetter(_))
    ));
}

pub async fn source_permanent_failure_stops_under_stop_policy() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![
            event_message("delivery.permanently_failed", Some("m1")),
            event_message("delivery.succeeded", Some("m2")),
        ],
    );
    let outcome = run_source(
        service,
        source,
        RunOptions::idempotent().with_failure_policy(FailurePolicy::Stop),
    )
    .await;
    assert!(outcome.unwrap_err().is_permanent());
    // Second message never processed; first was not settled.
    assert_eq!(
        recorder.events(),
        vec![Event::Handled("delivery.permanently_failed".into())]
    );
}

pub async fn source_unhandled_message_is_acked_and_ignored() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("unrelated", Some("m1"))],
    );
    run_source(service, source, RunOptions::idempotent())
        .await
        .unwrap();
    // Acked without dispatching or dead-lettering.
    assert_eq!(recorder.events(), vec![Event::Ack]);
}

pub async fn source_inbox_mode_rejects_missing_stable_id() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    // No id on the message; inbox mode requires a stable id.
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("delivery.succeeded", None)],
    );
    run_source(service, source, RunOptions::inbox(()))
        .await
        .unwrap();
    let events = recorder.events();
    // Handler never ran; the missing id is a permanent failure (dead-lettered).
    assert!(!events.iter().any(|e| matches!(e, Event::Handled(_))));
    assert!(matches!(events.first(), Some(Event::DeadLetter(_))));
}

pub async fn source_inbox_mode_dispatches_with_stable_id() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("delivery.succeeded", Some("m1"))],
    );
    run_source(service, source, RunOptions::inbox(()))
        .await
        .unwrap();
    assert_eq!(
        recorder.events(),
        vec![Event::Handled("delivery.succeeded".into()), Event::Ack]
    );
}

pub async fn source_propagates_recv_errors() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("delivery.succeeded", Some("m1"))],
    )
    .with_recv_error();
    let outcome = run_source(service, source, RunOptions::idempotent()).await;
    assert!(outcome.is_err(), "recv errors must not be swallowed");
    assert!(recorder.events().is_empty());
}

pub async fn source_propagates_settle_errors() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("delivery.succeeded", Some("m1"))],
    )
    .with_settle_failure();
    let outcome = run_source(service, source, RunOptions::idempotent()).await;
    assert!(outcome.is_err(), "settle errors must not be swallowed");
    // The ack was attempted before the error surfaced.
    assert_eq!(
        recorder.events(),
        vec![Event::Handled("delivery.succeeded".into()), Event::Ack]
    );
}

// =============================================================================
// Publisher / outbox dispatcher contract
// =============================================================================

async fn store_outbox(repo: &HashMapRepository, id: &str) -> String {
    let message = OutboxMessage::create(id, "order.initialized", b"\x01".to_vec()).unwrap();
    let mut batch = CommitBatch::empty();
    batch.outbox_messages.push(message);
    repo.commit_batch(batch).await.unwrap();
    id.to_string()
}

fn outbox_status(repo: &HashMapRepository, id: &str) -> Option<OutboxMessageStatus> {
    use distributed::OutboxStore;
    let store = repo.outbox_store();
    [
        OutboxMessageStatus::Pending,
        OutboxMessageStatus::InFlight,
        OutboxMessageStatus::Published,
        OutboxMessageStatus::Failed,
    ]
    .into_iter()
    .find(|status| {
        store
            .messages_by_status(status.clone())
            .unwrap()
            .iter()
            .any(|message| message.id() == id)
    })
}

fn dispatcher(
    repo: &HashMapRepository,
    mode: PublishMode,
    max_attempts: u32,
) -> OutboxDispatcher<HashMapOutboxStore, FakePublisher> {
    OutboxDispatcher::new(
        repo.outbox_store(),
        FakePublisher::new(mode),
        "immediate:conformance",
        Duration::from_secs(60),
        max_attempts,
    )
}

pub async fn dispatcher_completes_only_after_publish_success() {
    let repo = HashMapRepository::new();
    let id = store_outbox(&repo, "evt-1").await;
    let dispatcher = dispatcher(&repo, PublishMode::Succeed, 3);

    let outcome = dispatcher
        .dispatch_ids(std::slice::from_ref(&id))
        .await
        .unwrap();
    assert_eq!(outcome.published, 1);
    assert_eq!(
        dispatcher.publisher().published_ids(),
        vec!["evt-1".to_string()]
    );
    assert_eq!(
        outbox_status(&repo, &id),
        Some(OutboxMessageStatus::Published)
    );
}

pub async fn dispatcher_unknown_outcome_stays_retryable() {
    let repo = HashMapRepository::new();
    let id = store_outbox(&repo, "evt-1").await;
    let dispatcher = dispatcher(&repo, PublishMode::FailUnknown, 3);

    let outcome = dispatcher
        .dispatch_ids(std::slice::from_ref(&id))
        .await
        .unwrap();
    assert_eq!(outcome.published, 0);
    assert_eq!(outcome.released, 1);
    assert_eq!(
        outbox_status(&repo, &id),
        Some(OutboxMessageStatus::Pending),
        "row must stay retryable"
    );
}

pub async fn dispatcher_claims_explicit_ids_before_publish() {
    let repo = HashMapRepository::new();
    let wanted = store_outbox(&repo, "evt-1").await;
    let other = store_outbox(&repo, "evt-2").await;
    let dispatcher = dispatcher(&repo, PublishMode::Succeed, 3);

    let outcome = dispatcher
        .dispatch_ids(std::slice::from_ref(&wanted))
        .await
        .unwrap();
    assert_eq!(outcome.claimed, 1);
    assert_eq!(outcome.published, 1);
    assert_eq!(
        outbox_status(&repo, &wanted),
        Some(OutboxMessageStatus::Published)
    );
    // The unrequested row is untouched (claimed before publish, by id).
    assert_eq!(
        outbox_status(&repo, &other),
        Some(OutboxMessageStatus::Pending)
    );
}

// ============================================================================
// Broker-test helpers
// ============================================================================
//
// Shared by the real-broker test mains (kafka/nats/rabbitmq/postgres), which
// include this module the same way the in-memory harness does. The
// per-transport scenarios stay in their own files — only the genuinely
// identical scaffolding lives here.

use std::sync::atomic::{AtomicU64, Ordering};

/// Per-process sequence counter feeding [`unique`], so names do not collide
/// between tests in the same run.
static SEQ: AtomicU64 = AtomicU64::new(1);

/// A `Service` whose single handler records each message's id into `rec`;
/// `kind` selects command vs event registration.
pub fn recording_for(
    name: &str,
    kind: MessageKind,
    rec: Arc<Mutex<Vec<String>>>,
) -> Arc<Service<()>> {
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    let builder = Service::new();
    let registered = match kind {
        MessageKind::Command => builder.command(leaked),
        MessageKind::Event => builder.event(leaked),
    };
    Arc::new(registered.handle(move |ctx: &Context<()>| {
        rec.lock()
            .unwrap()
            .push(ctx.message().id().unwrap_or_default().to_string());
        async move { Ok(json!({})) }
    }))
}

/// Like [`recording_for`], but names the service so consumer-group / queue
/// identity is explicit (used where two replicas share a group).
pub fn named_recording_for(
    service_name: &str,
    name: &str,
    kind: MessageKind,
    rec: Arc<Mutex<Vec<String>>>,
) -> Arc<Service<()>> {
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    let builder = Service::new().named(service_name.to_string());
    let registered = match kind {
        MessageKind::Command => builder.command(leaked),
        MessageKind::Event => builder.event(leaked),
    };
    Arc::new(registered.handle(move |ctx: &Context<()>| {
        rec.lock()
            .unwrap()
            .push(ctx.message().id().unwrap_or_default().to_string());
        async move { Ok(json!({})) }
    }))
}

/// A per-process token mixing wall-clock nanos with the pid, so names are unique
/// even across separate runs against a persistent broker.
pub fn run_token() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ u128::from(std::process::id())
}

/// A unique subject/queue/stream name: `{prefix}_{run_token:x}_{seq}`. Both the
/// run token and the sequence are included so names collide neither within a
/// run nor across runs against persistent broker state.
pub fn unique(prefix: &str) -> String {
    format!(
        "{prefix}_{:x}_{}",
        run_token(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    )
}
