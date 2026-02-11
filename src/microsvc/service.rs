//! Service — command handler registry and dispatch for microsvc.
//!
//! `Service<R>` holds a repository and a set of named command handlers.
//! Each handler receives a `Context<R>` and returns `Result<Value, HandlerError>`.
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::microsvc;
//! use serde_json::json;
//!
//! let service = microsvc::in_memory()
//!     .command("order.create", |ctx| {
//!         let input = ctx.input::<CreateOrderInput>()?;
//!         Ok(json!({ "id": input.id }))
//!     });
//!
//! let result = service.dispatch("order.create", json!({"id": "1"}), Session::new());
//! ```

use std::collections::HashMap;

use serde_json::Value;

use super::context::Context;
use super::error::HandlerError;
use super::session::Session;

/// A registered command handler with optional guard.
struct CommandHandler<R> {
    guard: Option<Box<dyn Fn(&Context<R>) -> bool + Send + Sync>>,
    handle: Box<dyn Fn(&Context<R>) -> Result<Value, HandlerError> + Send + Sync>,
}

/// A microservice that routes commands to handler functions.
///
/// Generic over `R`, the repository type. Handlers receive a `Context<R>`
/// and can access the repo via `ctx.repo()`.
pub struct Service<R> {
    repo: R,
    handlers: HashMap<String, CommandHandler<R>>,
}

impl<R: Send + Sync + 'static> Service<R> {
    /// Create a new service with the given repository.
    pub fn new(repo: R) -> Self {
        Self {
            repo,
            handlers: HashMap::new(),
        }
    }

    /// Register a command handler.
    ///
    /// Uses builder pattern — returns `self` for chaining.
    pub fn command<F>(mut self, name: &str, handler: F) -> Self
    where
        F: Fn(&Context<R>) -> Result<Value, HandlerError> + Send + Sync + 'static,
    {
        self.handlers.insert(
            name.to_string(),
            CommandHandler {
                guard: None,
                handle: Box::new(handler),
            },
        );
        self
    }

    /// Register a command handler with a guard function.
    ///
    /// The guard is called before the handler. If it returns `false`,
    /// the command is rejected with `HandlerError::GuardRejected`.
    pub fn command_guarded<G, F>(mut self, name: &str, guard: G, handler: F) -> Self
    where
        G: Fn(&Context<R>) -> bool + Send + Sync + 'static,
        F: Fn(&Context<R>) -> Result<Value, HandlerError> + Send + Sync + 'static,
    {
        self.handlers.insert(
            name.to_string(),
            CommandHandler {
                guard: Some(Box::new(guard)),
                handle: Box::new(handler),
            },
        );
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

        let ctx = Context::new(command.to_string(), input, session, &self.repo);

        // Run guard if present
        if let Some(guard) = &handler.guard {
            if !guard(&ctx) {
                return Err(HandlerError::GuardRejected(command.to_string()));
            }
        }

        (handler.handle)(&ctx)
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

    /// Dispatch a bus `Event` as a command.
    ///
    /// Maps the event fields to a dispatch call:
    /// - `event.event_type` → command name
    /// - `event.payload` → JSON input (parsed from bytes)
    /// - `event.metadata` → session variables
    #[cfg(feature = "bus")]
    pub fn dispatch_event(&self, event: &crate::bus::Event) -> Result<Value, HandlerError> {
        let input = event_to_json_input(event);
        let session = event_to_session(event);
        self.dispatch(&event.event_type, input, session)
    }

    /// List registered command names.
    pub fn commands(&self) -> Vec<&str> {
        self.handlers.keys().map(|s| s.as_str()).collect()
    }

    /// Get a reference to the repository.
    pub fn repo(&self) -> &R {
        &self.repo
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

/// Handle to a background listener thread. Drop or call `stop()` to shut down.
#[cfg(feature = "bus")]
pub struct TransportHandle {
    stop_tx: std::sync::mpsc::Sender<()>,
    handle: Option<std::thread::JoinHandle<TransportStats>>,
}

#[cfg(feature = "bus")]
impl TransportHandle {
    /// Stop the transport and wait for it to finish. Returns stats.
    pub fn stop(mut self) -> TransportStats {
        let _ = self.stop_tx.send(());
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap_or_default()
        } else {
            TransportStats::default()
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
///     microsvc::in_memory()
///         .command("counter.create", handlers::counter_create::handle)
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
/// let stats = handle.stop();
/// ```
#[cfg(feature = "bus")]
pub fn listen<R, L>(
    service: std::sync::Arc<Service<R>>,
    queue_name: &str,
    listener: L,
    poll_interval: std::time::Duration,
) -> TransportHandle
where
    R: Send + Sync + 'static,
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
///
/// ## Example
///
/// ```ignore
/// use std::sync::Arc;
/// use sourced_rust::microsvc;
/// use sourced_rust::bus::InMemoryQueue;
///
/// let service = Arc::new(
///     microsvc::in_memory()
///         .command("order.created", handlers::on_order_created::handle)
/// );
///
/// let queue = InMemoryQueue::new();
/// let handle = microsvc::subscribe(
///     service.clone(),
///     queue.new_subscriber(),
///     Duration::from_millis(50),
/// );
///
/// let stats = handle.stop();
/// ```
#[cfg(feature = "bus")]
pub fn subscribe<R, S>(
    service: std::sync::Arc<Service<R>>,
    subscriber: S,
    poll_interval: std::time::Duration,
) -> TransportHandle
where
    R: Send + Sync + 'static,
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
// Helpers: convert bus Event to dispatch inputs
// =============================================================================

/// Parse a bus Event's payload as JSON. Falls back to wrapping raw bytes.
#[cfg(feature = "bus")]
fn event_to_json_input(event: &crate::bus::Event) -> Value {
    // Try JSON first
    if let Ok(value) = serde_json::from_slice::<Value>(&event.payload) {
        return value;
    }
    // Fall back to string payload
    if let Some(s) = event.payload_str() {
        return Value::String(s.to_string());
    }
    // Last resort: null
    Value::Null
}

/// Extract session variables from a bus Event's metadata.
#[cfg(feature = "bus")]
fn event_to_session(event: &crate::bus::Event) -> Session {
    match &event.metadata {
        Some(meta) => {
            let vars: HashMap<String, String> = meta
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Session::from_map(vars)
        }
        None => Session::new(),
    }
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
        let service = test_service().command("ping", |_ctx| Ok(json!({ "pong": true })));
        let result = service.dispatch("ping", json!({}), Session::new()).unwrap();
        assert_eq!(result, json!({ "pong": true }));
    }

    #[test]
    fn unknown_command() {
        let service = test_service().command("ping", |_ctx| Ok(json!({})));
        let result = service.dispatch("unknown", json!({}), Session::new());
        assert!(matches!(result, Err(HandlerError::UnknownCommand(ref s)) if s == "unknown"));
    }

    #[test]
    fn handler_error_propagates() {
        let service = test_service()
            .command("fail", |_ctx| Err(HandlerError::Rejected("nope".into())));
        let result = service.dispatch("fail", json!({}), Session::new());
        assert!(matches!(result, Err(HandlerError::Rejected(ref s)) if s == "nope"));
    }

    #[test]
    fn decode_error_from_bad_payload() {
        #[derive(serde::Deserialize)]
        struct Input { _name: String }

        let service = test_service().command("typed", |ctx| {
            let _input = ctx.input::<Input>()?;
            Ok(json!({}))
        });
        let result = service.dispatch("typed", json!({ "wrong": 1 }), Session::new());
        assert!(matches!(result, Err(HandlerError::DecodeFailed(_))));
    }

    #[test]
    fn commands_list() {
        let service = test_service()
            .command("a", |_| Ok(json!({})))
            .command("b", |_| Ok(json!({})));
        let mut cmds = service.commands();
        cmds.sort();
        assert_eq!(cmds, vec!["a", "b"]);
    }

    #[test]
    fn guard_passes() {
        let service = test_service().command_guarded(
            "greet",
            |ctx| ctx.has_fields(&["name"]),
            |ctx| {
                let name = ctx.raw_input()["name"].as_str().unwrap();
                Ok(json!({ "hello": name }))
            },
        );
        let result = service.dispatch("greet", json!({ "name": "Pat" }), Session::new()).unwrap();
        assert_eq!(result, json!({ "hello": "Pat" }));
    }

    #[test]
    fn guard_rejects() {
        let service = test_service().command_guarded(
            "greet",
            |ctx| ctx.has_fields(&["name"]),
            |_ctx| panic!("handler should not run"),
        );
        let result = service.dispatch("greet", json!({ "wrong": 1 }), Session::new());
        assert!(matches!(result, Err(HandlerError::GuardRejected(ref s)) if s == "greet"));
    }

    #[test]
    fn guard_checks_session() {
        let service = test_service().command_guarded(
            "admin",
            |ctx| ctx.role() == Some("admin"),
            |_ctx| Ok(json!({ "ok": true })),
        );

        // No role
        assert!(service.dispatch("admin", json!({}), Session::new()).is_err());

        // Admin role
        let mut session = Session::new();
        session.set("x-hasura-role", "admin");
        assert!(service.dispatch("admin", json!({}), session).is_ok());
    }

    #[test]
    fn dispatch_request_success() {
        let service = test_service().command("ping", |_ctx| Ok(json!({ "pong": true })));
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
            .command("reject", |_| Err(HandlerError::Rejected("no".into())))
            .command("unauth", |ctx| { let _ = ctx.user_id()?; Ok(json!({})) });

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
        let service = test_service().command("whoami", |ctx| {
            let user_id = ctx.user_id()?;
            Ok(json!({ "user_id": user_id }))
        });
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
    #[serde(default)]
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
