//! The direct-source runner.
//!
//! [`run_source`] is the shared receive loop for direct transports. It owns the
//! cross-cutting policy — when execution counts as successful, how retryable vs
//! permanent failures are routed — while the adapter owns how acknowledgement
//! maps back to the transport. The same dispatch/runner boundary is what the
//! Knative/HTTP ingress will call, so consumer execution stays identical across
//! ingress shapes.

use std::future::Future;
use std::sync::Arc;

use super::source::{MessageSource, ReceivedMessage};
use super::{FailureAction, MessageRouter, RunOptions, TransportError, TransportErrorKind};
use super::{Message, MessageKind};

/// Run the receive loop for a direct transport source.
///
/// For each message the runner:
///
/// 1. enforces the inbox stable-id contract (a no-op in idempotent mode);
/// 2. dispatches through [`MessageRouter::dispatch`];
/// 3. on success, acknowledges via the adapter;
/// 4. on failure, routes through [`RunOptions::failure_policy`] — retryable
///    failures are nacked for redelivery, permanent failures take the configured
///    action ([dead-letter](FailureAction::DeadLetter), [park](FailureAction::Park),
///    [log-and-ack](FailureAction::LogAndAck), or [stop](FailureAction::Stop)).
///
/// A message with no registered handler is **intentionally ignored**: the runner
/// acks it and moves on rather than dead-lettering it. Fan-out event transports
/// may deliver events this service does not consume, and acking matches
/// `microsvc::subscribe`; production transports should use
/// [`MessageRouter::subscription_plan`] to avoid delivering unrelated messages at all.
///
/// The runner **acks only after handler effects have completed**, never before.
/// It stops gracefully when the source returns `Ok(None)`, having fully settled
/// the in-flight message first. Receive and settle errors are propagated, not
/// swallowed: a returned `Err` ends the run and the supervisor may restart it
/// (already-committed effects make redelivery safe).
///
/// Inbox note: until the consumer-inbox subtask lands, inbox mode enforces the
/// stable-id requirement and then dispatches like idempotent mode. The
/// receipt-commit wrapping that makes it effectively-once is added there.
///
/// `I: Send` keeps the returned future `Send` so the runner can be spawned on a
/// multi-threaded executor regardless of the inbox hook type.
pub async fn run_source<R, S, I>(
    router: Arc<R>,
    mut source: S,
    options: RunOptions<I>,
) -> Result<(), TransportError>
where
    R: MessageRouter,
    S: MessageSource,
    I: Send,
{
    let service = router.consumer_group();
    let transport = source.transport_name();

    loop {
        let Some(received) = recv_next(&mut source, service, transport).await? else {
            break;
        };

        // A delivery the transport could not decode is a permanent failure: it
        // carries no valid message to dispatch, and it must NOT be treated as an
        // empty message (which would route to ack-and-ignore below and silently
        // drop a corrupt row). Route it through the failure policy directly, the
        // same as a permanent dispatch failure, so it is dead-lettered/parked.
        if let Some(error) = received.decode_error() {
            let action = options.failure_policy.resolve(error);
            let message_context =
                crate::diagnostics::FailureMessageContext::from_message(received.message());
            record_transport_failure(
                service,
                transport,
                Some(&message_context),
                error.kind(),
                action,
                Some(error.message()),
            );
            let kind = received.message().kind;
            match action {
                FailureAction::Nack => {
                    let reason = error.to_string();
                    settle_and_record(
                        service,
                        transport,
                        kind,
                        crate::telemetry::transport_outcome::NACK,
                        crate::telemetry::transport_outcome::NACK,
                        || received.nack(&reason),
                    )
                    .await?;
                }
                FailureAction::DeadLetter => {
                    let reason = error.to_string();
                    settle_and_record(
                        service,
                        transport,
                        kind,
                        crate::telemetry::transport_outcome::DEAD_LETTER,
                        crate::telemetry::transport_outcome::DEAD_LETTER,
                        || received.dead_letter(&reason),
                    )
                    .await?;
                }
                FailureAction::Park => {
                    let reason = error.to_string();
                    settle_and_record(
                        service,
                        transport,
                        kind,
                        crate::telemetry::transport_outcome::PARK,
                        crate::telemetry::transport_outcome::PARK,
                        || received.park(&reason),
                    )
                    .await?;
                }
                FailureAction::LogAndAck => {
                    eprintln!("[bus::runner] dropping undecodable message after permanent failure: {error}");
                    settle_and_record(
                        service,
                        transport,
                        kind,
                        crate::telemetry::transport_outcome::ACK,
                        crate::telemetry::transport_outcome::LOG_AND_ACK,
                        || received.ack(),
                    )
                    .await?;
                }
                FailureAction::Stop => return Err(TransportError::permanent(error.to_string())),
            }
            continue;
        }
        // No handler for this message: intentionally ignore (ack) rather than
        // dead-letter, so unrelated fan-out events don't pile into the DLQ.
        if !router.handles(received.message().kind, received.message().name()) {
            let kind = received.message().kind;
            settle_and_record(
                service,
                transport,
                kind,
                crate::telemetry::transport_outcome::ACK,
                crate::telemetry::transport_outcome::IGNORED,
                || received.ack(),
            )
            .await?;
            continue;
        }
        let kind = received.message().kind;
        match dispatch(router.as_ref(), &options, received.message()).await {
            Ok(()) => {
                settle_and_record(
                    service,
                    transport,
                    kind,
                    crate::telemetry::transport_outcome::ACK,
                    crate::telemetry::transport_outcome::ACK,
                    || received.ack(),
                )
                .await?;
            }
            Err(error) => match options.failure_policy.resolve(&error) {
                action @ FailureAction::Nack => {
                    let message_context =
                        crate::diagnostics::FailureMessageContext::from_message(received.message());
                    record_transport_failure(
                        service,
                        transport,
                        Some(&message_context),
                        error.kind(),
                        action,
                        Some(error.message()),
                    );
                    let reason = error.to_string();
                    settle_and_record(
                        service,
                        transport,
                        kind,
                        crate::telemetry::transport_outcome::NACK,
                        crate::telemetry::transport_outcome::NACK,
                        || received.nack(&reason),
                    )
                    .await?;
                }
                action @ FailureAction::DeadLetter => {
                    let message_context =
                        crate::diagnostics::FailureMessageContext::from_message(received.message());
                    record_transport_failure(
                        service,
                        transport,
                        Some(&message_context),
                        error.kind(),
                        action,
                        Some(error.message()),
                    );
                    let reason = error.to_string();
                    settle_and_record(
                        service,
                        transport,
                        kind,
                        crate::telemetry::transport_outcome::DEAD_LETTER,
                        crate::telemetry::transport_outcome::DEAD_LETTER,
                        || received.dead_letter(&reason),
                    )
                    .await?;
                }
                action @ FailureAction::Park => {
                    let message_context =
                        crate::diagnostics::FailureMessageContext::from_message(received.message());
                    record_transport_failure(
                        service,
                        transport,
                        Some(&message_context),
                        error.kind(),
                        action,
                        Some(error.message()),
                    );
                    let reason = error.to_string();
                    settle_and_record(
                        service,
                        transport,
                        kind,
                        crate::telemetry::transport_outcome::PARK,
                        crate::telemetry::transport_outcome::PARK,
                        || received.park(&reason),
                    )
                    .await?;
                }
                FailureAction::LogAndAck => {
                    record_transport_failure(
                        service,
                        transport,
                        Some(&crate::diagnostics::FailureMessageContext::from_message(
                            received.message(),
                        )),
                        error.kind(),
                        FailureAction::LogAndAck,
                        Some(error.message()),
                    );
                    eprintln!(
                        "[bus::runner] dropping message '{}' after permanent failure: {error}",
                        received.message().name()
                    );
                    settle_and_record(
                        service,
                        transport,
                        kind,
                        crate::telemetry::transport_outcome::ACK,
                        crate::telemetry::transport_outcome::LOG_AND_ACK,
                        || received.ack(),
                    )
                    .await?;
                }
                FailureAction::Stop => {
                    let message_context =
                        crate::diagnostics::FailureMessageContext::from_message(received.message());
                    record_transport_failure(
                        service,
                        transport,
                        Some(&message_context),
                        error.kind(),
                        FailureAction::Stop,
                        Some(error.message()),
                    );
                    return Err(error);
                }
            },
        }
    }
    Ok(())
}

async fn settle_and_record<F, Fut>(
    service: Option<&str>,
    transport: &str,
    kind: MessageKind,
    settle_action: &'static str,
    outcome: &'static str,
    settle: F,
) -> Result<(), TransportError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), TransportError>>,
{
    match settle().await {
        Ok(()) => {
            record_transport_message(service, transport, kind, outcome);
            Ok(())
        }
        Err(error) => {
            record_transport_failure(
                service,
                transport,
                None,
                error.kind(),
                crate::telemetry::settle_failure_action(settle_action),
                Some(error.message()),
            );
            Err(error)
        }
    }
}

async fn recv_next<S: MessageSource>(
    source: &mut S,
    service: Option<&str>,
    transport: &str,
) -> Result<Option<S::Received>, TransportError> {
    match source.recv().await {
        Ok(received) => Ok(received),
        Err(error) => {
            record_transport_failure(
                service,
                transport,
                None,
                error.kind(),
                crate::telemetry::failure_action::RECV_ERROR,
                Some(error.message()),
            );
            Err(error)
        }
    }
}

/// Run consumer execution for one message and classify the outcome.
///
/// Enforces the inbox stable-id contract first (idempotent mode yields no key
/// and skips it), then dispatches. A failed stable-id check is a permanent
/// failure — redelivery cannot supply a missing or malformed id.
async fn dispatch<R: MessageRouter, I>(
    router: &R,
    options: &RunOptions<I>,
    message: &Message,
) -> Result<(), TransportError> {
    #[cfg(feature = "otel")]
    {
        use tracing::Instrument as _;

        let span = transport_receive_span(message);
        crate::trace_context::set_span_parent_from_metadata_if_no_current_span(
            &span,
            &message.metadata,
        );
        return async {
            options
                .validate_message_id(message)
                .map_err(|err| TransportError::permanent(err.to_string()).with_source(err))?;
            router.dispatch(message).await
        }
        .instrument(span)
        .await;
    }

    #[cfg(not(feature = "otel"))]
    {
        options
            .validate_message_id(message)
            .map_err(|err| TransportError::permanent(err.to_string()).with_source(err))?;
        router.dispatch(message).await
    }
}

#[cfg(feature = "otel")]
fn transport_receive_span(message: &Message) -> tracing::Span {
    crate::telemetry::transport_receive_span(message)
}

fn record_transport_message(
    service: Option<&str>,
    transport: &str,
    kind: MessageKind,
    outcome: &str,
) {
    #[cfg(feature = "metrics")]
    crate::metrics::record_transport_message(service, transport, kind, outcome);
    #[cfg(not(feature = "metrics"))]
    let _ = (service, transport, kind, outcome);
}

fn record_transport_failure<A>(
    service: Option<&str>,
    transport: &str,
    message: Option<&crate::diagnostics::FailureMessageContext>,
    kind: TransportErrorKind,
    action: A,
    error_summary: Option<&str>,
) where
    A: IntoFailureActionLabel,
{
    let action = action.into_failure_action_label();
    #[cfg(feature = "metrics")]
    crate::metrics::record_transport_failure(
        service,
        transport,
        crate::telemetry::transport_failure_class(kind),
        action,
    );
    #[cfg(not(feature = "metrics"))]
    {
        let _ = (service, transport, kind);
    }
    crate::diagnostics::record_transport_failure(
        service,
        transport,
        message,
        kind,
        action,
        error_summary,
    );
}

trait IntoFailureActionLabel {
    fn into_failure_action_label(self) -> &'static str;
}

impl IntoFailureActionLabel for FailureAction {
    fn into_failure_action_label(self) -> &'static str {
        crate::telemetry::failure_action_label(self)
    }
}

impl IntoFailureActionLabel for &'static str {
    fn into_failure_action_label(self) -> &'static str {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{FailurePolicy, Handlers, MessageKind};
    use std::collections::VecDeque;
    use std::future::Future;
    use std::sync::Mutex;

    // --- minimal runtime-free executor -------------------------------------
    // The transport module is not feature-gated, so its tests run without an
    // async runtime. The fake futures never suspend, so a busy-poll with a
    // no-op waker drives them to completion.
    fn block_on<F: Future>(future: F) -> F::Output {
        use std::ptr;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let raw = RawWaker::new(ptr::null(), &VTABLE);
        let waker = unsafe { Waker::from_raw(raw) };
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
        }
    }

    // --- recorder + fakes ---------------------------------------------------
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Event {
        Handled(String),
        Ack,
        Nack(String),
        DeadLetter(String),
        Park(String),
    }

    struct Recorder {
        events: Mutex<Vec<Event>>,
    }

    impl Recorder {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Vec::new()),
            })
        }
        fn push(&self, event: Event) {
            self.events.lock().unwrap().push(event);
        }
        fn events(&self) -> Vec<Event> {
            self.events.lock().unwrap().clone()
        }
    }

    struct FakeReceived {
        message: Message,
        recorder: Arc<Recorder>,
        settle_ok: bool,
        decode_error: Option<TransportError>,
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
        fn decode_error(&self) -> Option<&TransportError> {
            self.decode_error.as_ref()
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

    struct FakeSource {
        queue: VecDeque<Message>,
        recorder: Arc<Recorder>,
        settle_ok: bool,
        recv_error: bool,
        // When set, every received message reports this as a decode failure,
        // modeling a transport that claims a row/offset before decoding it.
        decode_error: bool,
    }

    impl MessageSource for FakeSource {
        type Received = FakeReceived;
        async fn recv(&mut self) -> Result<Option<FakeReceived>, TransportError> {
            if self.recv_error {
                return Err(TransportError::retryable("recv failed"));
            }
            let decode_error = self.decode_error;
            Ok(self.queue.pop_front().map(|message| FakeReceived {
                message,
                recorder: self.recorder.clone(),
                settle_ok: self.settle_ok,
                decode_error: decode_error
                    .then(|| TransportError::permanent("corrupt row: name failed to decode")),
            }))
        }
    }

    // --- helpers ------------------------------------------------------------
    fn event_message(name: &str, id: Option<&str>) -> Message {
        let mut message = Message::new(name, MessageKind::Event, b"{}".to_vec());
        if let Some(id) = id {
            message = message.with_id(id);
        }
        message
    }

    fn router(recorder: &Arc<Recorder>) -> Arc<Handlers> {
        let ok = recorder.clone();
        let retryable = recorder.clone();
        let permanent = recorder.clone();
        Arc::new(
            Handlers::new()
                .on_event("ok", move |msg: &Message| {
                    let ok = ok.clone();
                    let name = msg.name().to_string();
                    async move {
                        ok.push(Event::Handled(name));
                        Ok(())
                    }
                })
                .on_event("retryable", move |msg: &Message| {
                    let retryable = retryable.clone();
                    let name = msg.name().to_string();
                    async move {
                        retryable.push(Event::Handled(name));
                        Err(TransportError::retryable("infra"))
                    }
                })
                .on_event("permanent", move |msg: &Message| {
                    let permanent = permanent.clone();
                    let name = msg.name().to_string();
                    async move {
                        permanent.push(Event::Handled(name));
                        Err(TransportError::permanent("nope"))
                    }
                }),
        )
    }

    struct RunResult {
        outcome: Result<(), TransportError>,
        events: Vec<Event>,
    }

    fn run<I: Send>(messages: Vec<Message>, options: RunOptions<I>) -> RunResult {
        run_with(messages, options, true, false)
    }

    fn run_with<I: Send>(
        messages: Vec<Message>,
        options: RunOptions<I>,
        settle_ok: bool,
        recv_error: bool,
    ) -> RunResult {
        let recorder = Recorder::new();
        let svc = router(&recorder);
        let source = FakeSource {
            queue: messages.into_iter().collect(),
            recorder: recorder.clone(),
            settle_ok,
            recv_error,
            decode_error: false,
        };
        let outcome = block_on(run_source(svc, source, options));
        RunResult {
            outcome,
            events: recorder.events(),
        }
    }

    // --- tests --------------------------------------------------------------
    #[test]
    fn success_dispatches_then_acks_in_order() {
        let result = run(vec![event_message("ok", None)], RunOptions::idempotent());
        assert!(result.outcome.is_ok());
        // Handler effect is recorded before the ack: ack happens after success.
        assert_eq!(
            result.events,
            vec![Event::Handled("ok".to_string()), Event::Ack]
        );
    }

    #[test]
    fn processes_every_message_then_stops_on_none() {
        let result = run(
            vec![event_message("ok", None), event_message("ok", None)],
            RunOptions::idempotent(),
        );
        assert!(result.outcome.is_ok());
        assert_eq!(
            result.events,
            vec![
                Event::Handled("ok".to_string()),
                Event::Ack,
                Event::Handled("ok".to_string()),
                Event::Ack,
            ]
        );
    }

    #[cfg(feature = "metrics")]
    #[test]
    fn metrics_record_success_and_failure_settlement_outcomes() {
        let _guard = crate::metrics::lock_for_tests();
        crate::metrics::reset_for_tests();

        let recorder = Recorder::new();
        let svc = Arc::new(
            router(&recorder)
                .as_ref()
                .clone()
                .named("metrics-runner-settlement"),
        );
        let source = FakeSource {
            queue: vec![event_message("ok", None), event_message("retryable", None)]
                .into_iter()
                .collect(),
            recorder,
            settle_ok: true,
            recv_error: false,
            decode_error: false,
        };

        let outcome = block_on(run_source(svc, source, RunOptions::idempotent()));
        assert!(outcome.is_ok());

        let text = crate::metrics::prometheus_text();
        assert!(
            text.contains(
                "distributed_transport_messages_total{service=\"metrics-runner-settlement\",transport=\"unknown\",message_kind=\"event\",outcome=\"ack\"} 1"
            ),
            "metrics should include the ack outcome:\n{text}"
        );
        assert!(
            text.contains(
                "distributed_transport_messages_total{service=\"metrics-runner-settlement\",transport=\"unknown\",message_kind=\"event\",outcome=\"nack\"} 1"
            ),
            "metrics should include the nack outcome:\n{text}"
        );
        assert!(
            text.contains(
                "distributed_transport_failures_total{service=\"metrics-runner-settlement\",transport=\"unknown\",failure_class=\"retryable\",action=\"nack\"} 1"
            ),
            "metrics should include the retryable failure action:\n{text}"
        );
    }

    #[cfg(feature = "metrics")]
    #[test]
    fn metrics_record_settle_failures_before_propagating() {
        let _guard = crate::metrics::lock_for_tests();
        crate::metrics::reset_for_tests();

        let recorder = Recorder::new();
        let svc = Arc::new(
            router(&recorder)
                .as_ref()
                .clone()
                .named("metrics-runner-settle-failure"),
        );
        let source = FakeSource {
            queue: vec![event_message("ok", None)].into_iter().collect(),
            recorder,
            settle_ok: false,
            recv_error: false,
            decode_error: false,
        };

        let outcome = block_on(run_source(svc, source, RunOptions::idempotent()));
        assert!(outcome
            .expect_err("settle error should propagate")
            .is_retryable());

        let text = crate::metrics::prometheus_text();
        assert!(
            text.contains(
                "distributed_transport_failures_total{service=\"metrics-runner-settle-failure\",transport=\"unknown\",failure_class=\"retryable\",action=\"settle_ack\"} 1"
            ),
            "metrics should include the settle failure:\n{text}"
        );
        assert!(
            !text.contains(
                "distributed_transport_messages_total{service=\"metrics-runner-settle-failure\",transport=\"unknown\",message_kind=\"event\",outcome=\"ack\"} 1"
            ),
            "settle failure should not record an ack outcome:\n{text}"
        );
    }

    #[test]
    fn retryable_failure_nacks_without_acking() {
        let result = run(
            vec![event_message("retryable", None)],
            RunOptions::idempotent(),
        );
        assert!(result.outcome.is_ok());
        assert_eq!(
            result.events.first(),
            Some(&Event::Handled("retryable".to_string()))
        );
        assert!(matches!(result.events.get(1), Some(Event::Nack(_))));
        assert!(!result.events.contains(&Event::Ack));
    }

    #[test]
    fn permanent_failure_dead_letters_under_default_policy() {
        let result = run(
            vec![event_message("permanent", None)],
            RunOptions::idempotent(),
        );
        assert!(result.outcome.is_ok());
        assert_eq!(
            result.events.first(),
            Some(&Event::Handled("permanent".to_string()))
        );
        assert!(matches!(result.events.get(1), Some(Event::DeadLetter(_))));
    }

    #[test]
    fn permanent_failure_parks_under_park_policy() {
        let result = run(
            vec![event_message("permanent", None)],
            RunOptions::idempotent().with_failure_policy(FailurePolicy::Park),
        );
        assert!(result.outcome.is_ok());
        assert!(matches!(result.events.get(1), Some(Event::Park(_))));
    }

    #[test]
    fn permanent_failure_logs_and_acks_under_log_and_ack_policy() {
        let result = run(
            vec![event_message("permanent", None)],
            RunOptions::idempotent().with_failure_policy(FailurePolicy::LogAndAck),
        );
        assert!(result.outcome.is_ok());
        assert_eq!(result.events.get(1), Some(&Event::Ack));
    }

    #[test]
    fn permanent_failure_nacks_under_retry_policy() {
        let result = run(
            vec![event_message("permanent", None)],
            RunOptions::idempotent().with_failure_policy(FailurePolicy::Retry),
        );
        assert!(result.outcome.is_ok());
        assert!(matches!(result.events.get(1), Some(Event::Nack(_))));
    }

    #[test]
    fn stop_policy_returns_error_without_settling() {
        let result = run(
            vec![event_message("permanent", None), event_message("ok", None)],
            RunOptions::idempotent().with_failure_policy(FailurePolicy::Stop),
        );
        let err = result
            .outcome
            .expect_err("stop policy should surface the error");
        assert!(err.is_permanent());
        // The handler ran, but the message was not settled and the second
        // message was never processed.
        assert_eq!(result.events, vec![Event::Handled("permanent".to_string())]);
    }

    #[test]
    fn inbox_mode_rejects_message_without_stable_id_before_dispatch() {
        let result = run(vec![event_message("ok", None)], RunOptions::inbox(()));
        assert!(result.outcome.is_ok());
        // Handler never ran (no Handled event); the missing id is a permanent
        // failure routed to the default dead-letter policy, carrying the reason.
        assert_eq!(result.events.len(), 1);
        match &result.events[0] {
            Event::DeadLetter(reason) => {
                assert!(reason.contains("stable message id is required but missing"))
            }
            other => panic!("expected dead-letter, got {other:?}"),
        }
        assert!(!result.events.iter().any(|e| matches!(e, Event::Handled(_))));
    }

    #[test]
    fn inbox_mode_dispatches_when_stable_id_is_present() {
        let result = run(
            vec![event_message("ok", Some("evt-1"))],
            RunOptions::inbox(()),
        );
        assert!(result.outcome.is_ok());
        assert_eq!(
            result.events,
            vec![Event::Handled("ok".to_string()), Event::Ack]
        );
    }

    #[test]
    fn recv_error_propagates_and_is_not_swallowed() {
        let result = run_with(
            vec![event_message("ok", None)],
            RunOptions::idempotent(),
            true,
            true,
        );
        let err = result.outcome.expect_err("recv error should propagate");
        assert!(err.is_retryable());
        assert!(result.events.is_empty());
    }

    #[test]
    fn settle_error_propagates_and_is_not_swallowed() {
        let result = run_with(
            vec![event_message("ok", None)],
            RunOptions::idempotent(),
            false,
            false,
        );
        let err = result.outcome.expect_err("settle error should propagate");
        assert!(err.is_retryable());
        // The ack was attempted (recorded) before the error surfaced.
        assert_eq!(
            result.events,
            vec![Event::Handled("ok".to_string()), Event::Ack]
        );
    }

    #[test]
    fn settle_error_on_failure_path_propagates() {
        // A settle failure on the nack/failure-routing branch must propagate too,
        // not just on the ack branch.
        let result = run_with(
            vec![event_message("retryable", None)],
            RunOptions::idempotent(),
            false,
            false,
        );
        let err = result
            .outcome
            .expect_err("nack settle error should propagate");
        assert!(err.is_retryable());
        assert_eq!(
            result.events.first(),
            Some(&Event::Handled("retryable".to_string()))
        );
        assert!(matches!(result.events.get(1), Some(Event::Nack(_))));
    }

    #[test]
    fn unhandled_message_is_acked_and_ignored() {
        // No handler registered for "unrelated": ack-and-ignore, do not dispatch
        // or dead-letter.
        let result = run(
            vec![event_message("unrelated", None), event_message("ok", None)],
            RunOptions::idempotent(),
        );
        assert!(result.outcome.is_ok());
        assert_eq!(
            result.events,
            vec![Event::Ack, Event::Handled("ok".to_string()), Event::Ack]
        );
    }

    fn run_decode_error<I: Send>(messages: Vec<Message>, options: RunOptions<I>) -> RunResult {
        let recorder = Recorder::new();
        let svc = router(&recorder);
        let source = FakeSource {
            queue: messages.into_iter().collect(),
            recorder: recorder.clone(),
            settle_ok: true,
            recv_error: false,
            decode_error: true,
        };
        let outcome = block_on(run_source(svc, source, options));
        RunResult {
            outcome,
            events: recorder.events(),
        }
    }

    #[test]
    fn corrupt_row_dead_letters_under_default_policy_not_acked_and_ignored() {
        // The corrupt delivery has an unhandled (empty) name, which would
        // otherwise fall into the ack-and-ignore path and silently drop it. The
        // decode error must instead route it through the failure policy.
        let result = run_decode_error(vec![event_message("", None)], RunOptions::idempotent());
        assert!(result.outcome.is_ok());
        assert_eq!(result.events.len(), 1);
        match &result.events[0] {
            Event::DeadLetter(reason) => assert!(reason.contains("corrupt row")),
            other => panic!("expected dead-letter, got {other:?}"),
        }
        // It was never handled and never plain-acked.
        assert!(!result.events.iter().any(|e| matches!(e, Event::Handled(_))));
        assert!(!result.events.contains(&Event::Ack));
    }

    #[test]
    fn corrupt_row_parks_under_park_policy() {
        let result = run_decode_error(
            vec![event_message("", None)],
            RunOptions::idempotent().with_failure_policy(FailurePolicy::Park),
        );
        assert!(result.outcome.is_ok());
        assert!(matches!(result.events.first(), Some(Event::Park(_))));
    }

    #[test]
    fn corrupt_row_stops_under_stop_policy_with_permanent_error() {
        let result = run_decode_error(
            vec![event_message("", None)],
            RunOptions::idempotent().with_failure_policy(FailurePolicy::Stop),
        );
        let err = result
            .outcome
            .expect_err("stop policy surfaces the decode error");
        assert!(err.is_permanent());
        assert!(
            result.events.is_empty(),
            "stop does not settle the corrupt row"
        );
    }

    #[test]
    fn run_source_future_is_send() {
        // Guards the documented multi-threaded-executor contract for the common
        // (no-inbox) path: the runner future must be Send.
        fn assert_send<T: Send>(_: &T) {}
        let recorder = Recorder::new();
        let svc = router(&recorder);
        let source = FakeSource {
            queue: VecDeque::new(),
            recorder,
            settle_ok: true,
            recv_error: false,
            decode_error: false,
        };
        let future = run_source(svc, source, RunOptions::idempotent());
        assert_send(&future);
        // Drive it to completion (empty source -> immediate Ok).
        assert!(block_on(future).is_ok());
    }

    // A fake that relies on the trait's DEFAULT dead_letter/park (which forward
    // to nack), proving the "never silently dropped" degrade-to-redelivery
    // property of the provided methods.
    struct DefaultReceived {
        message: Message,
        recorder: Arc<Recorder>,
    }

    impl ReceivedMessage for DefaultReceived {
        fn message(&self) -> &Message {
            &self.message
        }
        async fn ack(self) -> Result<(), TransportError> {
            self.recorder.push(Event::Ack);
            Ok(())
        }
        async fn nack(self, reason: &str) -> Result<(), TransportError> {
            self.recorder.push(Event::Nack(reason.to_string()));
            Ok(())
        }
        // dead_letter and park intentionally NOT overridden.
    }

    #[test]
    fn default_dead_letter_and_park_degrade_to_nack() {
        let recorder = Recorder::new();
        let dl = DefaultReceived {
            message: event_message("ok", None),
            recorder: recorder.clone(),
        };
        block_on(dl.dead_letter("boom")).unwrap();

        let park = DefaultReceived {
            message: event_message("ok", None),
            recorder: recorder.clone(),
        };
        block_on(park.park("hold")).unwrap();

        // Both defaults route to nack rather than dropping the message.
        assert_eq!(
            recorder.events(),
            vec![
                Event::Nack("boom".to_string()),
                Event::Nack("hold".to_string())
            ]
        );
    }
}
