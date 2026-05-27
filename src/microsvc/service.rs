//! Service — command handler registry and dispatch for microsvc.
//!
//! `Service<D>` holds service dependencies and a set of named command handlers.
//! Each handler receives a `Context<D>` and returns `Result<Value, HandlerError>`.
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::microsvc;
//! use serde_json::json;
//!
//! let service = microsvc::Service::new(())
//!     .handler(microsvc::HandlerSpec::command("order.create"), |_| true, |ctx| {
//!         let input = ctx.input::<CreateOrderInput>()?;
//!         Ok(json!({ "id": input.id }))
//!     });
//!
//! let result = service.dispatch("order.create", json!({"id": "1"}), Session::new());
//! ```

use std::collections::HashMap;
use std::{error::Error, fmt, sync::Arc};

use serde_json::Value;

use super::context::Context;
use super::dependencies::{HasReadModelStore, HasRepo, RepoReadModelDependencies};
use super::error::HandlerError;
use super::session::Session;

#[cfg(feature = "bus")]
use crate::bus::Event;

type GuardFn<D> = dyn Fn(&Context<D>) -> bool + Send + Sync;
type HandlerFn<D> = dyn Fn(&Context<D>) -> Result<Value, HandlerError> + Send + Sync;

/// The kind of message a handler consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum MessageKind {
    /// A command addressed to one handler.
    Command,
    /// A published event that may be consumed by many handlers.
    Event,
}

/// How a handler expects the transport to deliver matching messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    /// Point-to-point delivery, normally used for command queues.
    PointToPoint,
    /// Fan-out delivery, normally used for event subscriptions.
    FanOut,
}

/// The input shape delivered to a handler after a message is received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerInput {
    /// Decode the message payload bytes as JSON and pass that JSON value to
    /// the handler. This is the default command-style input.
    PayloadJson,
    /// Pass the full [`MessageEnvelope`] to the handler as JSON.
    MessageEnvelope,
}

/// Static message names attached to a handler spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerNames {
    /// A single command or event name.
    One(&'static str),
    /// Multiple event names handled by one projection-style handler.
    Many(&'static [&'static str]),
}

impl HandlerNames {
    fn to_vec(self) -> Vec<&'static str> {
        match self {
            Self::One(name) => vec![name],
            Self::Many(names) => names.to_vec(),
        }
    }
}

/// Transport-visible metadata for a registered handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandlerSpec {
    names: HandlerNames,
    pub kind: MessageKind,
    pub delivery: DeliveryKind,
    pub input: HandlerInput,
}

impl HandlerSpec {
    /// A command handler that consumes JSON payloads.
    pub const fn command(name: &'static str) -> Self {
        Self {
            names: HandlerNames::One(name),
            kind: MessageKind::Command,
            delivery: DeliveryKind::PointToPoint,
            input: HandlerInput::PayloadJson,
        }
    }

    /// An event handler that consumes JSON payloads.
    pub const fn event(name: &'static str) -> Self {
        Self {
            names: HandlerNames::One(name),
            kind: MessageKind::Event,
            delivery: DeliveryKind::FanOut,
            input: HandlerInput::PayloadJson,
        }
    }

    /// An event handler that consumes several event names.
    pub const fn events(names: &'static [&'static str]) -> Self {
        Self {
            names: HandlerNames::Many(names),
            kind: MessageKind::Event,
            delivery: DeliveryKind::FanOut,
            input: HandlerInput::PayloadJson,
        }
    }

    /// Deliver the full message envelope to the handler.
    pub const fn envelope(mut self) -> Self {
        self.input = HandlerInput::MessageEnvelope;
        self
    }

    /// Deliver only the message JSON payload to the handler.
    pub const fn payload_json(mut self) -> Self {
        self.input = HandlerInput::PayloadJson;
        self
    }

    /// Message names consumed by this handler.
    pub fn names(&self) -> Vec<&'static str> {
        self.names.to_vec()
    }
}

/// Transport subscription metadata derived from registered handlers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubscriptionPlan {
    /// Command names consumed by point-to-point command transports.
    pub commands: Vec<String>,
    /// Event names consumed by fan-out event transports.
    pub events: Vec<String>,
}

/// Serializable transport message envelope used by projection-style handlers
/// that need message ids, names, payload bytes, and metadata.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MessageEnvelope {
    pub id: String,
    pub name: String,
    pub kind: MessageKind,
    pub payload: Vec<u8>,
    pub content_type: String,
    pub metadata: Vec<(String, String)>,
}

#[cfg(feature = "bus")]
impl From<&Event> for MessageEnvelope {
    fn from(event: &Event) -> Self {
        Self {
            id: event.id.clone(),
            name: event.event_type.clone(),
            kind: MessageKind::Event,
            payload: event.payload.clone(),
            content_type: "application/json".to_string(),
            metadata: event.metadata.clone().unwrap_or_default(),
        }
    }
}

#[cfg(feature = "bus")]
impl From<MessageEnvelope> for Event {
    fn from(envelope: MessageEnvelope) -> Self {
        let metadata = if envelope.metadata.is_empty() {
            None
        } else {
            Some(envelope.metadata)
        };

        Self {
            id: envelope.id,
            event_type: envelope.name,
            payload: envelope.payload,
            metadata,
        }
    }
}

/// A registered command handler with optional guard.
struct CommandHandler<D> {
    input: HandlerInput,
    guard: Option<Arc<GuardFn<D>>>,
    handle: Arc<HandlerFn<D>>,
}

/// A microservice that routes commands to handler functions.
///
/// Generic over `D`, the service dependency type. Prefer
/// [`Service::with_repo`], [`Service::with_read_model_store`], or
/// [`Service::with_repo_and_read_model_store`] for common dependency shapes.
pub struct Service<D> {
    dependencies: D,
    handlers: HashMap<String, CommandHandler<D>>,
    handler_specs: Vec<HandlerSpec>,
}

impl<D: Send + Sync + 'static> Service<D> {
    /// Create a new service with custom dependencies.
    pub fn new(dependencies: D) -> Self {
        Self {
            dependencies,
            handlers: HashMap::new(),
            handler_specs: Vec::new(),
        }
    }

    /// Create a service whose dependency type is an aggregate repository.
    pub fn with_repo(repo: D) -> Self
    where
        D: HasRepo,
    {
        Self::new(repo)
    }

    /// Create a service whose dependency type is a read-model store.
    pub fn with_read_model_store(read_model_store: D) -> Self
    where
        D: HasReadModelStore,
    {
        Self::new(read_model_store)
    }

    /// Register a handler from a transport-visible spec.
    pub fn handler<G, F>(self, spec: HandlerSpec, guard: G, handler: F) -> Self
    where
        G: Fn(&Context<D>) -> bool + Send + Sync + 'static,
        F: Fn(&Context<D>) -> Result<Value, HandlerError> + Send + Sync + 'static,
    {
        self.register_handler(spec, Some(Arc::new(guard)), Arc::new(handler))
    }

    fn register_handler(
        mut self,
        spec: HandlerSpec,
        guard: Option<Arc<GuardFn<D>>>,
        handle: Arc<HandlerFn<D>>,
    ) -> Self {
        for name in spec.names() {
            self.handlers.insert(
                name.to_string(),
                CommandHandler {
                    input: spec.input,
                    guard: guard.clone(),
                    handle: handle.clone(),
                },
            );
        }
        self.handler_specs.push(spec);
        self
    }

    /// Dispatch a command by name.
    ///
    /// Builds a `Context` from the input and session, looks up the handler,
    /// runs the guard (if any), then calls the handler.
    pub fn dispatch(
        &self,
        command: &str,
        input: Value,
        session: Session,
    ) -> Result<Value, HandlerError> {
        let handler = self
            .handlers
            .get(command)
            .ok_or_else(|| HandlerError::UnknownCommand(command.to_string()))?;

        let ctx = Context::new(command.to_string(), input, session, &self.dependencies);

        // Run guard if present
        if let Some(guard) = &handler.guard {
            if !guard(&ctx) {
                return Err(HandlerError::GuardRejected(command.to_string()));
            }
        }

        (handler.handle.as_ref())(&ctx)
    }

    /// Dispatch a `CommandRequest`, returning a `CommandResponse`.
    pub fn dispatch_request(&self, request: &CommandRequest) -> CommandResponse {
        let session = Session::from_map(request.session_variables.clone());
        match self.dispatch(&request.command, request.input.clone(), session) {
            Ok(value) => CommandResponse {
                status: 200,
                body: value,
            },
            Err(e) => CommandResponse {
                status: e.status_code(),
                body: serde_json::json!({ "error": e.to_string() }),
            },
        }
    }

    /// Dispatch a transport message.
    pub fn dispatch_message(&self, message: &MessageEnvelope) -> Result<Value, HandlerError> {
        let handler = self
            .handlers
            .get(&message.name)
            .ok_or_else(|| HandlerError::UnknownCommand(message.name.clone()))?;
        let input = message_to_handler_input(message, handler.input)?;
        let session = message_to_session(message);
        self.dispatch(&message.name, input, session)
    }

    /// Dispatch a bus `Event` as a message.
    #[cfg(feature = "bus")]
    pub fn dispatch_event(&self, event: &crate::bus::Event) -> Result<Value, HandlerError> {
        self.dispatch_message(&MessageEnvelope::from(event))
    }

    /// List registered command names.
    pub fn command_names(&self) -> Vec<&str> {
        names_by_kind(&self.handler_specs, MessageKind::Command)
    }

    /// List registered event names.
    pub fn event_names(&self) -> Vec<&str> {
        names_by_kind(&self.handler_specs, MessageKind::Event)
    }

    /// Return transport metadata for registered handlers.
    pub fn handler_specs(&self) -> &[HandlerSpec] {
        &self.handler_specs
    }

    /// Return the command/event names a transport should subscribe to.
    pub fn subscription_plan(&self) -> SubscriptionPlan {
        let mut plan = SubscriptionPlan::default();

        for spec in &self.handler_specs {
            for name in spec.names() {
                let bucket = match spec.kind {
                    MessageKind::Command => &mut plan.commands,
                    MessageKind::Event => &mut plan.events,
                };
                if !bucket.iter().any(|existing| existing == name) {
                    bucket.push(name.to_string());
                }
            }
        }

        plan
    }

    /// Return whether this service has a handler for the message name.
    pub fn handles(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Get a reference to the service dependencies.
    pub fn dependencies(&self) -> &D {
        &self.dependencies
    }

    /// Get the aggregate repository for services whose dependencies expose one.
    pub fn repo(&self) -> &D::Repo
    where
        D: HasRepo,
    {
        self.dependencies.repo()
    }

    /// Get the read-model store for services whose dependencies expose one.
    pub fn read_model_store(&self) -> &D::ReadModelStore
    where
        D: HasReadModelStore,
    {
        self.dependencies.read_model_store()
    }
}

impl<R: Send + Sync + 'static, S: Send + Sync + 'static> Service<RepoReadModelDependencies<R, S>> {
    /// Create a service whose handlers need both an aggregate repository and a
    /// read-model store.
    pub fn with_repo_and_read_model_store(repo: R, read_model_store: S) -> Self {
        Self::new(RepoReadModelDependencies::new(repo, read_model_store))
    }
}

// =============================================================================
// Bus transports (requires "bus" feature)
// =============================================================================

/// Statistics from a bus transport thread.
#[cfg(feature = "bus")]
#[derive(Debug, Default, Clone)]
pub struct TransportStats {
    /// Number of commands successfully handled.
    pub handled: usize,
    /// Number of commands that failed handling.
    pub failed: usize,
    /// Number of poll cycles completed.
    pub polls: usize,
}

/// Error returned when a bus transport thread fails during shutdown.
#[cfg(feature = "bus")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportJoinError;

#[cfg(feature = "bus")]
impl fmt::Display for TransportJoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "microsvc transport thread panicked during shutdown")
    }
}

#[cfg(feature = "bus")]
impl Error for TransportJoinError {}

/// Handle to a background listener thread. Drop or call `stop()` to shut down.
#[cfg(feature = "bus")]
pub struct TransportHandle {
    stop_tx: std::sync::mpsc::Sender<()>,
    handle: Option<std::thread::JoinHandle<TransportStats>>,
}

#[cfg(feature = "bus")]
impl TransportHandle {
    /// Stop the transport and wait for it to finish. Returns stats.
    ///
    /// Returns [`TransportJoinError`] if the transport thread panicked before
    /// shutdown completed.
    pub fn stop(mut self) -> Result<TransportStats, TransportJoinError> {
        let _ = self.stop_tx.send(());
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| TransportJoinError)
        } else {
            Ok(TransportStats::default())
        }
    }

    /// Signal stop without waiting.
    pub fn signal_stop(&self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "bus")]
impl Drop for TransportHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

/// Start listening on a named queue (point-to-point) and dispatching to handlers.
///
/// Spawns a background thread that polls the queue. Each message is delivered
/// to exactly one listener (competing consumers pattern).
///
/// The service is wrapped in `Arc` so it can be shared between the transport
/// thread and the caller (for HTTP dispatch, etc.).
///
/// ## Example
///
/// ```ignore
/// use std::sync::Arc;
/// use sourced_rust::microsvc;
/// use sourced_rust::bus::{InMemoryQueue, Sender, Event};
///
/// let service = Arc::new(
///     sourced_rust::register_handlers!(
///         microsvc::Service::with_repo(repo),
///         handlers::counter_create,
///     )
/// );
///
/// let queue = InMemoryQueue::new();
/// let handle = microsvc::listen(
///     service.clone(),
///     "counters",
///     queue.clone(),
///     Duration::from_millis(50),
/// );
///
/// // Send commands to the queue
/// queue.send("counters", Event::with_string_payload("cmd-1", "counter.create", r#"{"id":"c1"}"#))?;
///
/// // HTTP dispatch still works on the same service
/// service.dispatch("counter.create", json!({"id":"c2"}), Session::new())?;
///
/// let stats = handle.stop()?;
/// ```
#[cfg(feature = "bus")]
pub fn listen<D, L>(
    service: std::sync::Arc<Service<D>>,
    queue_name: &str,
    listener: L,
    poll_interval: std::time::Duration,
) -> TransportHandle
where
    D: Send + Sync + 'static,
    L: crate::bus::Listener + 'static,
{
    let queue_name = queue_name.to_string();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();

    let handle = std::thread::spawn(move || {
        let mut stats = TransportStats::default();

        loop {
            match stop_rx.try_recv() {
                Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }

            stats.polls += 1;

            match listener.listen(&queue_name, poll_interval.as_millis() as u64) {
                Ok(Some(event)) => match service.dispatch_event(&event) {
                    Ok(_) => stats.handled += 1,
                    Err(_) => stats.failed += 1,
                },
                Ok(None) => {}
                Err(_) => {}
            }
        }

        stats
    });

    TransportHandle {
        stop_tx,
        handle: Some(handle),
    }
}

/// Start subscribing to events (pub/sub fan-out) and dispatching to handlers.
///
/// Spawns a background thread that polls the subscriber. Unlike `listen`
/// (point-to-point), every subscriber sees every event — use this when
/// multiple services need to react to the same events.
///
/// Successfully handled events are acknowledged. Failed events are nacked.
/// Events with no registered handler are acknowledged and ignored; production
/// transports should use [`Service::subscription_plan`] to avoid delivering
/// unrelated event types to the service.
///
/// ## Example
///
/// ```ignore
/// use std::sync::Arc;
/// use sourced_rust::microsvc;
/// use sourced_rust::bus::InMemoryQueue;
///
/// let service = Arc::new(
///     sourced_rust::register_handlers!(
///         microsvc::Service::new(()),
///         event handlers::on_order_created,
///     )
/// );
///
/// let queue = InMemoryQueue::new();
/// let handle = microsvc::subscribe(
///     service.clone(),
///     queue.new_subscriber(),
///     Duration::from_millis(50),
/// );
///
/// let stats = handle.stop()?;
/// ```
#[cfg(feature = "bus")]
pub fn subscribe<D, S>(
    service: std::sync::Arc<Service<D>>,
    subscriber: S,
    poll_interval: std::time::Duration,
) -> TransportHandle
where
    D: Send + Sync + 'static,
    S: crate::bus::Subscriber + 'static,
{
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();

    let handle = std::thread::spawn(move || {
        let mut stats = TransportStats::default();

        loop {
            match stop_rx.try_recv() {
                Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }

            stats.polls += 1;

            match subscriber.poll(poll_interval.as_millis() as u64) {
                Ok(Some(event)) if !service.handles(&event.event_type) => {
                    let _ = subscriber.ack(&event.id);
                }
                Ok(Some(event)) => match service.dispatch_event(&event) {
                    Ok(_) => {
                        let _ = subscriber.ack(&event.id);
                        stats.handled += 1;
                    }
                    Err(_) => {
                        let _ = subscriber.nack(&event.id, "handler error");
                        stats.failed += 1;
                    }
                },
                Ok(None) => {}
                Err(_) => {}
            }
        }

        stats
    });

    TransportHandle {
        stop_tx,
        handle: Some(handle),
    }
}

// =============================================================================
// Helpers: convert transport messages to dispatch inputs
// =============================================================================

fn names_by_kind(specs: &[HandlerSpec], kind: MessageKind) -> Vec<&str> {
    let mut names = Vec::new();

    for spec in specs.iter().filter(|spec| spec.kind == kind) {
        for name in spec.names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }

    names
}

fn message_to_handler_input(
    message: &MessageEnvelope,
    input: HandlerInput,
) -> Result<Value, HandlerError> {
    match input {
        HandlerInput::PayloadJson => message_to_json_input(message),
        HandlerInput::MessageEnvelope => serde_json::to_value(message)
            .map_err(|e| HandlerError::DecodeFailed(format!("invalid message envelope: {e}"))),
    }
}

fn message_to_json_input(message: &MessageEnvelope) -> Result<Value, HandlerError> {
    serde_json::from_slice::<Value>(&message.payload).map_err(|e| {
        HandlerError::DecodeFailed(format!(
            "invalid JSON payload for message '{}': {}",
            message.name, e
        ))
    })
}

fn message_to_session(message: &MessageEnvelope) -> Session {
    let vars: HashMap<String, String> = message.metadata.iter().cloned().collect();
    Session::from_map(vars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_service() -> Service<()> {
        Service::new(())
    }

    #[test]
    fn dispatch_returns_handler_result() {
        let service = test_service().handler(
            HandlerSpec::command("ping"),
            |_| true,
            |_ctx| Ok(json!({ "pong": true })),
        );
        let result = service.dispatch("ping", json!({}), Session::new()).unwrap();
        assert_eq!(result, json!({ "pong": true }));
    }

    #[test]
    fn unknown_command() {
        let service =
            test_service().handler(HandlerSpec::command("ping"), |_| true, |_ctx| Ok(json!({})));
        let result = service.dispatch("unknown", json!({}), Session::new());
        assert!(matches!(result, Err(HandlerError::UnknownCommand(ref s)) if s == "unknown"));
    }

    #[test]
    fn handler_error_propagates() {
        let service = test_service().handler(
            HandlerSpec::command("fail"),
            |_| true,
            |_ctx| Err(HandlerError::Rejected("nope".into())),
        );
        let result = service.dispatch("fail", json!({}), Session::new());
        assert!(matches!(result, Err(HandlerError::Rejected(ref s)) if s == "nope"));
    }

    #[test]
    fn decode_error_from_bad_payload() {
        #[derive(serde::Deserialize)]
        struct Input {
            _name: String,
        }

        let service = test_service().handler(
            HandlerSpec::command("typed"),
            |_| true,
            |ctx| {
                let _input = ctx.input::<Input>()?;
                Ok(json!({}))
            },
        );
        let result = service.dispatch("typed", json!({ "wrong": 1 }), Session::new());
        assert!(matches!(result, Err(HandlerError::DecodeFailed(_))));
    }

    #[test]
    fn command_names_list() {
        let service = test_service()
            .handler(HandlerSpec::command("a"), |_| true, |_| Ok(json!({})))
            .handler(HandlerSpec::command("b"), |_| true, |_| Ok(json!({})));
        let mut cmds = service.command_names();
        cmds.sort();
        assert_eq!(cmds, vec!["a", "b"]);
    }

    #[test]
    fn subscription_plan_separates_commands_and_events() {
        const EVENTS: &[&str] = &["checkout.started", "seat.reserved"];

        let service = test_service()
            .handler(
                HandlerSpec::command("checkout.start"),
                |_| true,
                |_| Ok(json!({})),
            )
            .handler(
                HandlerSpec::events(EVENTS).envelope(),
                |_| true,
                |_| Ok(json!({})),
            );

        assert_eq!(
            service.subscription_plan(),
            SubscriptionPlan {
                commands: vec!["checkout.start".to_string()],
                events: vec!["checkout.started".to_string(), "seat.reserved".to_string()],
            }
        );
    }

    #[test]
    fn dispatch_message_delivers_payload_json_by_default() {
        let service = test_service().handler(
            HandlerSpec::event("checkout.started"),
            |ctx| ctx.has_fields(&["checkout_id"]),
            |ctx| {
                Ok(json!({
                    "checkout_id": ctx.raw_input()["checkout_id"].as_str().unwrap(),
                    "user_id": ctx.user_id()?,
                }))
            },
        );
        let message = MessageEnvelope {
            id: "evt-1".to_string(),
            name: "checkout.started".to_string(),
            kind: MessageKind::Event,
            payload: br#"{"checkout_id":"checkout-1"}"#.to_vec(),
            content_type: "application/json".to_string(),
            metadata: vec![("x-hasura-user-id".to_string(), "user-1".to_string())],
        };

        let result = service.dispatch_message(&message).unwrap();

        assert_eq!(
            result,
            json!({ "checkout_id": "checkout-1", "user_id": "user-1" })
        );
    }

    #[test]
    fn dispatch_message_can_deliver_full_envelope() {
        let service = test_service().handler(
            HandlerSpec::event("seat.reserved").envelope(),
            |ctx| ctx.has_fields(&["id", "name", "payload"]),
            |ctx| {
                let envelope = ctx.input::<MessageEnvelope>()?;
                Ok(json!({
                    "event_id": envelope.id,
                    "name": envelope.name,
                    "metadata": envelope.metadata,
                }))
            },
        );
        let message = MessageEnvelope {
            id: "evt-2".to_string(),
            name: "seat.reserved".to_string(),
            kind: MessageKind::Event,
            payload: br#"{"seat_id":"A-7"}"#.to_vec(),
            content_type: "application/json".to_string(),
            metadata: vec![("correlation_id".to_string(), "checkout-1".to_string())],
        };

        let result = service.dispatch_message(&message).unwrap();

        assert_eq!(
            result,
            json!({
                "event_id": "evt-2",
                "name": "seat.reserved",
                "metadata": [["correlation_id", "checkout-1"]],
            })
        );
    }

    #[test]
    fn guard_passes() {
        let service = test_service().handler(
            HandlerSpec::command("greet"),
            |ctx| ctx.has_fields(&["name"]),
            |ctx| {
                let name = ctx.raw_input()["name"].as_str().unwrap();
                Ok(json!({ "hello": name }))
            },
        );
        let result = service
            .dispatch("greet", json!({ "name": "Pat" }), Session::new())
            .unwrap();
        assert_eq!(result, json!({ "hello": "Pat" }));
    }

    #[test]
    fn guard_rejects() {
        let service = test_service().handler(
            HandlerSpec::command("greet"),
            |ctx| ctx.has_fields(&["name"]),
            |_ctx| panic!("handler should not run"),
        );
        let result = service.dispatch("greet", json!({ "wrong": 1 }), Session::new());
        assert!(matches!(result, Err(HandlerError::GuardRejected(ref s)) if s == "greet"));
    }

    #[test]
    fn guard_checks_session() {
        let service = test_service().handler(
            HandlerSpec::command("admin"),
            |ctx| ctx.role() == Some("admin"),
            |_ctx| Ok(json!({ "ok": true })),
        );

        // No role
        assert!(service
            .dispatch("admin", json!({}), Session::new())
            .is_err());

        // Admin role
        let mut session = Session::new();
        session.set("x-hasura-role", "admin");
        assert!(service.dispatch("admin", json!({}), session).is_ok());
    }

    #[test]
    fn dispatch_request_success() {
        let service = test_service().handler(
            HandlerSpec::command("ping"),
            |_| true,
            |_ctx| Ok(json!({ "pong": true })),
        );
        let request = CommandRequest {
            command: "ping".to_string(),
            input: json!({}),
            session_variables: HashMap::new(),
        };
        let response = service.dispatch_request(&request);
        assert_eq!(response.status, 200);
        assert_eq!(response.body, json!({ "pong": true }));
    }

    #[test]
    fn dispatch_request_error_codes() {
        let service = test_service()
            .handler(
                HandlerSpec::command("reject"),
                |_| true,
                |_| Err(HandlerError::Rejected("no".into())),
            )
            .handler(
                HandlerSpec::command("unauth"),
                |_| true,
                |ctx| {
                    let _ = ctx.user_id()?;
                    Ok(json!({}))
                },
            );

        let resp = service.dispatch_request(&CommandRequest {
            command: "unknown".to_string(),
            input: json!({}),
            session_variables: HashMap::new(),
        });
        assert_eq!(resp.status, 404);

        let resp = service.dispatch_request(&CommandRequest {
            command: "reject".to_string(),
            input: json!({}),
            session_variables: HashMap::new(),
        });
        assert_eq!(resp.status, 422);

        let resp = service.dispatch_request(&CommandRequest {
            command: "unauth".to_string(),
            input: json!({}),
            session_variables: HashMap::new(),
        });
        assert_eq!(resp.status, 401);
    }

    #[test]
    fn dispatch_request_passes_session() {
        let service = test_service().handler(
            HandlerSpec::command("whoami"),
            |_| true,
            |ctx| {
                let user_id = ctx.user_id()?;
                Ok(json!({ "user_id": user_id }))
            },
        );
        let mut vars = HashMap::new();
        vars.insert("x-hasura-user-id".to_string(), "user-99".to_string());
        let request = CommandRequest {
            command: "whoami".to_string(),
            input: json!({}),
            session_variables: vars,
        };
        let response = service.dispatch_request(&request);
        assert_eq!(response.status, 200);
        assert_eq!(response.body, json!({ "user_id": "user-99" }));
    }

    #[test]
    fn command_request_requires_session_variables_field() {
        let json = r#"{"command":"ping","input":{}}"#;
        let result: Result<CommandRequest, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[cfg(feature = "bus")]
    #[test]
    fn dispatch_event_rejects_non_json_payload() {
        let service = test_service().handler(
            HandlerSpec::command("ping"),
            |_| true,
            |_ctx| Ok(json!({ "ok": true })),
        );
        let event = crate::bus::Event::with_string_payload("evt-1", "ping", "not-json");
        let result = service.dispatch_event(&event);
        assert!(matches!(result, Err(HandlerError::DecodeFailed(_))));
    }

    #[cfg(feature = "bus")]
    #[test]
    fn transport_stop_returns_stats_when_thread_exits_cleanly() {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _ = stop_rx.recv();
            TransportStats {
                handled: 2,
                failed: 1,
                polls: 3,
            }
        });
        let transport = TransportHandle {
            stop_tx,
            handle: Some(handle),
        };

        let stats = transport.stop().unwrap();

        assert_eq!(stats.handled, 2);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.polls, 3);
    }

    #[cfg(feature = "bus")]
    #[test]
    fn transport_stop_returns_error_when_thread_panics() {
        let (stop_tx, _stop_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(|| -> TransportStats {
            panic!("transport panic");
        });
        let transport = TransportHandle {
            stop_tx,
            handle: Some(handle),
        };

        let err = transport
            .stop()
            .expect_err("transport thread panic should be returned");

        assert_eq!(err, TransportJoinError);
    }
}

// =============================================================================
// Request / Response types
// =============================================================================

/// An inbound command request.
///
/// Maps to a Hasura Action payload:
/// ```json
/// {
///   "action": { "name": "CreateOrder" },
///   "input": { "product_id": "SKU-1" },
///   "session_variables": { "x-hasura-user-id": "user-42" }
/// }
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandRequest {
    /// Command name (from `action.name` or URL path).
    pub command: String,
    /// JSON input payload.
    pub input: Value,
    /// Session variables (user ID, role, etc.).
    pub session_variables: HashMap<String, String>,
}

/// Response from dispatching a command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandResponse {
    /// HTTP-style status code.
    pub status: u16,
    /// Response body (handler result or error).
    pub body: Value,
}
