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

use serde_json::json;
use sourced_rust::microsvc::transport::{
    run_source, AsyncMessagePublisher, AsyncMessageSource, FailurePolicy, OutboxDispatcher,
    ReceivedMessage, RunOptions, TransportError,
};
use sourced_rust::microsvc::{Context, HandlerError, Message, MessageKind, Service};
use sourced_rust::{
    CommitBatch, HashMapOutboxStore, HashMapRepository, OutboxMessage, OutboxMessageStatus,
    TransactionalCommit,
};

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
        Service::new(())
            .event("ok")
            .handle(move |ctx: &Context<()>| {
                ok.push(Event::Handled(ctx.message().name().to_string()));
                async move { Ok(json!({})) }
            })
            .event("retryable")
            .handle(move |ctx: &Context<()>| {
                retryable.push(Event::Handled(ctx.message().name().to_string()));
                async move { Err(HandlerError::Other("infra".into())) }
            })
            .event("permanent")
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
    let source = FakeSource::new(recorder.clone(), vec![event_message("ok", Some("m1"))]);
    run_source(service, source, RunOptions::idempotent())
        .await
        .unwrap();
    assert_eq!(
        recorder.events(),
        vec![Event::Handled("ok".into()), Event::Ack],
        "handler must run before ack"
    );
}

pub async fn source_retryable_failure_nacks_without_ack() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("retryable", Some("m1"))],
    );
    run_source(service, source, RunOptions::idempotent())
        .await
        .unwrap();
    let events = recorder.events();
    assert_eq!(events.first(), Some(&Event::Handled("retryable".into())));
    assert!(matches!(events.get(1), Some(Event::Nack(_))));
    assert!(!events.contains(&Event::Ack));
}

pub async fn source_permanent_failure_dead_letters_by_default() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(
        recorder.clone(),
        vec![event_message("permanent", Some("m1"))],
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
            event_message("permanent", Some("m1")),
            event_message("ok", Some("m2")),
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
    assert_eq!(recorder.events(), vec![Event::Handled("permanent".into())]);
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
    let source = FakeSource::new(recorder.clone(), vec![event_message("ok", None)]);
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
    let source = FakeSource::new(recorder.clone(), vec![event_message("ok", Some("m1"))]);
    run_source(service, source, RunOptions::inbox(()))
        .await
        .unwrap();
    assert_eq!(
        recorder.events(),
        vec![Event::Handled("ok".into()), Event::Ack]
    );
}

pub async fn source_propagates_recv_errors() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source =
        FakeSource::new(recorder.clone(), vec![event_message("ok", Some("m1"))]).with_recv_error();
    let outcome = run_source(service, source, RunOptions::idempotent()).await;
    assert!(outcome.is_err(), "recv errors must not be swallowed");
    assert!(recorder.events().is_empty());
}

pub async fn source_propagates_settle_errors() {
    let recorder = Recorder::new();
    let service = recording_service(&recorder);
    let source = FakeSource::new(recorder.clone(), vec![event_message("ok", Some("m1"))])
        .with_settle_failure();
    let outcome = run_source(service, source, RunOptions::idempotent()).await;
    assert!(outcome.is_err(), "settle errors must not be swallowed");
    // The ack was attempted before the error surfaced.
    assert_eq!(
        recorder.events(),
        vec![Event::Handled("ok".into()), Event::Ack]
    );
}

// =============================================================================
// Publisher / outbox dispatcher contract
// =============================================================================

fn store_outbox(repo: &HashMapRepository, id: &str) -> String {
    let message = OutboxMessage::create(id, "OrderCreated", b"\x01".to_vec()).unwrap();
    let mut batch = CommitBatch::empty();
    batch.outbox_messages.push(message);
    repo.commit_batch(batch).unwrap();
    id.to_string()
}

fn outbox_status(repo: &HashMapRepository, id: &str) -> Option<OutboxMessageStatus> {
    use sourced_rust::OutboxStore;
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
    let id = store_outbox(&repo, "evt-1");
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
    let id = store_outbox(&repo, "evt-1");
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
    let wanted = store_outbox(&repo, "evt-1");
    let other = store_outbox(&repo, "evt-2");
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
