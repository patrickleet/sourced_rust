//! Service — handler registry and dispatch for microsvc.
//!
//! `Service<D>` holds service dependencies and a set of named command/event handlers.
//! Each handler receives a `Context<D>` and returns `Result<Value, HandlerError>`.
//!
//! ## Example
//!
//! ```ignore
//! use distributed::microsvc;
//! use serde_json::json;
//!
//! let service = microsvc::Service::new(())
//!     .command("order.create")
//!     .handle(|ctx| {
//!         let input = ctx.input::<CreateOrderInput>()?;
//!         Ok(json!({ "id": input.id }))
//!     });
//!
//! let result = service.dispatch("order.create", json!({"id": "1"}), Session::new());
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use super::context::Context;
use super::dependencies::{HasReadModelStore, HasRepo, RepoReadModelDependencies};
use super::error::HandlerError;
use super::session::Session;
use crate::bus::{Message, MessageKind, SubscriptionPlan};

type GuardFn<D> = dyn Fn(&Context<D>) -> bool + Send + Sync;
type HandlerFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, HandlerError>> + Send + 'a>>;
type HandlerFn<D> = dyn for<'a> Fn(&'a Context<'a, D>) -> HandlerFuture<'a> + Send + Sync;

/// Lets an `async fn handle(ctx: &Context<D>) -> Result<Value, HandlerError>`
/// register directly as a handler. The higher-ranked bound ties the returned
/// future's lifetime to the borrowed [`Context`], which a plain generic future
/// parameter cannot express.
pub trait Handler<'a, D: 'a>: Send + Sync {
    /// The future returned by the handler for a context borrowed for `'a`.
    type Future: Future<Output = Result<Value, HandlerError>> + Send + 'a;
    fn call(&self, ctx: &'a Context<'a, D>) -> Self::Future;
}

impl<'a, D, F, Fut> Handler<'a, D> for F
where
    D: 'a,
    F: Fn(&'a Context<'a, D>) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, HandlerError>> + Send + 'a,
{
    type Future = Fut;
    fn call(&self, ctx: &'a Context<'a, D>) -> Fut {
        self(ctx)
    }
}

fn boxed_handler<D, F>(handler: F) -> Arc<HandlerFn<D>>
where
    F: for<'a> Handler<'a, D> + 'static,
{
    Arc::new(move |ctx| Box::pin(handler.call(ctx)) as HandlerFuture<'_>)
}

/// How a handler expects the transport to deliver matching messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    /// Point-to-point delivery, normally used for command queues.
    PointToPoint,
    /// Fan-out delivery, normally used for event subscriptions.
    FanOut,
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
}

impl HandlerSpec {
    /// A command handler that consumes JSON payloads.
    pub const fn command(name: &'static str) -> Self {
        Self {
            names: HandlerNames::One(name),
            kind: MessageKind::Command,
            delivery: DeliveryKind::PointToPoint,
        }
    }

    /// An event handler that consumes JSON payloads.
    pub const fn event(name: &'static str) -> Self {
        Self {
            names: HandlerNames::One(name),
            kind: MessageKind::Event,
            delivery: DeliveryKind::FanOut,
        }
    }

    /// An event handler that consumes several event names.
    pub const fn events(names: &'static [&'static str]) -> Self {
        Self {
            names: HandlerNames::Many(names),
            kind: MessageKind::Event,
            delivery: DeliveryKind::FanOut,
        }
    }

    /// Message names consumed by this handler.
    pub fn names(&self) -> Vec<&'static str> {
        self.names.to_vec()
    }
}

/// A registered handler with optional guard.
struct RegisteredHandler<D> {
    guard: Option<Arc<GuardFn<D>>>,
    handle: Arc<HandlerFn<D>>,
}

/// Builder returned by [`Service::command`], [`Service::event`],
/// [`Service::events`], and [`Service::handler`].
pub struct HandlerBuilder<D> {
    service: Service<D>,
    spec: HandlerSpec,
}

impl<D: Send + Sync + 'static> HandlerBuilder<D> {
    /// Register an async handler without a guard.
    pub fn handle<F>(self, handler: F) -> Service<D>
    where
        F: for<'a> Handler<'a, D> + 'static,
    {
        self.service
            .register_handler(self.spec, None, boxed_handler(handler))
    }

    /// Register an async handler with a (synchronous) guard.
    pub fn guarded<G, F>(self, guard: G, handler: F) -> Service<D>
    where
        G: Fn(&Context<D>) -> bool + Send + Sync + 'static,
        F: for<'a> Handler<'a, D> + 'static,
    {
        self.service
            .register_handler(self.spec, Some(Arc::new(guard)), boxed_handler(handler))
    }
}

/// A microservice that routes commands to handler functions.
///
/// Generic over `D`, the service dependency type. Prefer
/// [`Service::with_repo`], [`Service::with_read_model_store`], or
/// [`Service::with_repo_and_read_model_store`] for common dependency shapes.
pub struct Service<D> {
    dependencies: D,
    handlers: HashMap<(MessageKind, String), RegisteredHandler<D>>,
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

    /// Mutable access to the dependencies, used by `with_bus` to install the
    /// outbox publisher on the repository before the service is shared.
    pub(crate) fn dependencies_mut(&mut self) -> &mut D {
        &mut self.dependencies
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

    /// Start registering a command handler that consumes JSON payload input.
    pub fn command(self, name: &'static str) -> HandlerBuilder<D> {
        self.handler(HandlerSpec::command(name))
    }

    /// Start registering an event handler that consumes JSON payload input.
    pub fn event(self, name: &'static str) -> HandlerBuilder<D> {
        self.handler(HandlerSpec::event(name))
    }

    /// Start registering an event handler for several event names that consume JSON
    /// payload input.
    pub fn events(self, names: &'static [&'static str]) -> HandlerBuilder<D> {
        self.handler(HandlerSpec::events(names))
    }

    /// Start registering a handler from a transport-visible spec.
    pub fn handler(self, spec: HandlerSpec) -> HandlerBuilder<D> {
        HandlerBuilder {
            service: self,
            spec,
        }
    }

    fn register_handler(
        mut self,
        spec: HandlerSpec,
        guard: Option<Arc<GuardFn<D>>>,
        handle: Arc<HandlerFn<D>>,
    ) -> Self {
        for name in spec.names() {
            self.handlers.insert(
                handler_key(spec.kind, name),
                RegisteredHandler {
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
    pub async fn dispatch(
        &self,
        command: &str,
        input: Value,
        session: Session,
    ) -> Result<Value, HandlerError> {
        if !self.handles_message(MessageKind::Command, command) {
            return Err(HandlerError::UnknownCommand(command.to_string()));
        }

        let payload = serde_json::to_vec(&input).map_err(|e| {
            HandlerError::DecodeFailed(format!("invalid JSON input for command '{command}': {e}"))
        })?;
        let metadata = session
            .variables()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let message = Message {
            id: None,
            name: command.to_string(),
            kind: MessageKind::Command,
            payload,
            content_type: "application/json".to_string(),
            metadata,
        };

        self.invoke(message, input, session).await
    }

    /// Dispatch a `CommandRequest`, returning a `CommandResponse`.
    pub async fn dispatch_request(&self, request: &CommandRequest) -> CommandResponse {
        let session = Session::from_map(request.session_variables.clone());
        match self
            .dispatch(&request.command, request.input.clone(), session)
            .await
        {
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
    pub async fn dispatch_message(&self, message: &Message) -> Result<Value, HandlerError> {
        if !self.handles_message(message.kind, &message.name) {
            return Err(HandlerError::UnknownCommand(message.name.clone()));
        }

        let input = match message_to_json_input(message) {
            Ok(input) => input,
            Err(_) => Value::Null,
        };
        let session = message_to_session(message);
        self.invoke(message.clone(), input, session).await
    }

    async fn invoke(
        &self,
        message: Message,
        input: Value,
        session: Session,
    ) -> Result<Value, HandlerError> {
        // Clone the handler/guard Arcs so the handler map is not borrowed across
        // the (awaited) handler future.
        let (guard, handle) = {
            let handler = self
                .handlers
                .get(&handler_key(message.kind, &message.name))
                .ok_or_else(|| HandlerError::UnknownCommand(message.name.clone()))?;
            (handler.guard.clone(), handler.handle.clone())
        };
        let name = message.name.clone();
        let ctx = Context::new(message, input, session, &self.dependencies);

        // Run guard (synchronous) if present.
        if let Some(guard) = &guard {
            if !guard(&ctx) {
                return Err(HandlerError::GuardRejected(name));
            }
        }

        handle(&ctx).await
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
        self.handlers
            .keys()
            .any(|(_, registered_name)| registered_name == name)
    }

    /// Return whether this service has a handler for this message kind and name.
    pub fn handles_message(&self, kind: MessageKind, name: &str) -> bool {
        self.handlers.contains_key(&handler_key(kind, name))
    }

    /// Return whether this service has an event handler for the message name.
    pub fn handles_event(&self, name: &str) -> bool {
        self.handles_message(MessageKind::Event, name)
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

fn handler_key(kind: MessageKind, name: &str) -> (MessageKind, String) {
    (kind, name.to_string())
}

fn message_to_json_input(message: &Message) -> Result<Value, HandlerError> {
    serde_json::from_slice::<Value>(&message.payload).map_err(|e| {
        HandlerError::DecodeFailed(format!(
            "invalid JSON payload for message '{}': {}",
            message.name, e
        ))
    })
}

fn message_to_session(message: &Message) -> Session {
    let vars: HashMap<String, String> = message
        .metadata
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.clone()))
        .collect();
    Session::from_map(vars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_service() -> Service<()> {
        Service::new(())
    }

    #[tokio::test]
    async fn dispatch_returns_handler_result() {
        let service = test_service()
            .command("ping")
            .handle(|_ctx: &Context<()>| async move { Ok(json!({ "pong": true })) });
        let result = service
            .dispatch("ping", json!({}), Session::new())
            .await
            .unwrap();
        assert_eq!(result, json!({ "pong": true }));
    }

    #[tokio::test]
    async fn unknown_command() {
        let service = test_service()
            .command("ping")
            .handle(|_ctx: &Context<()>| async move { Ok(json!({})) });
        let result = service.dispatch("unknown", json!({}), Session::new()).await;
        assert!(matches!(result, Err(HandlerError::UnknownCommand(ref s)) if s == "unknown"));
    }

    #[tokio::test]
    async fn handler_error_propagates() {
        let service = test_service()
            .command("fail")
            .handle(|_ctx: &Context<()>| async move { Err(HandlerError::Rejected("nope".into())) });
        let result = service.dispatch("fail", json!({}), Session::new()).await;
        assert!(matches!(result, Err(HandlerError::Rejected(ref s)) if s == "nope"));
    }

    #[tokio::test]
    async fn decode_error_from_bad_payload() {
        #[derive(serde::Deserialize)]
        struct Input {
            _name: String,
        }

        let service = test_service().command("typed").handle(|ctx: &Context<()>| {
            let input = ctx.input::<Input>();
            async move {
                let _input = input?;
                Ok(json!({}))
            }
        });
        let result = service
            .dispatch("typed", json!({ "wrong": 1 }), Session::new())
            .await;
        assert!(matches!(result, Err(HandlerError::DecodeFailed(_))));
    }

    #[test]
    fn command_names_list() {
        let service = test_service()
            .command("a")
            .handle(|_: &Context<()>| async move { Ok(json!({})) })
            .command("b")
            .handle(|_: &Context<()>| async move { Ok(json!({})) });
        let mut cmds = service.command_names();
        cmds.sort();
        assert_eq!(cmds, vec!["a", "b"]);
    }

    #[test]
    fn subscription_plan_separates_commands_and_events() {
        const EVENTS: &[&str] = &["checkout.started", "seat.reserved"];

        let service = test_service()
            .command("checkout.start")
            .handle(|_: &Context<()>| async move { Ok(json!({})) })
            .events(EVENTS)
            .guarded(|_| true, |_: &Context<()>| async move { Ok(json!({})) });

        assert_eq!(
            service.subscription_plan(),
            SubscriptionPlan {
                commands: vec!["checkout.start".to_string()],
                events: vec!["checkout.started".to_string(), "seat.reserved".to_string()],
            }
        );
    }

    #[test]
    fn event_conveniences_record_event_names() {
        const EVENTS: &[&str] = &["seat.added", "seat.reserved"];

        let service = test_service()
            .event("checkout.started")
            .handle(|_: &Context<()>| async move { Ok(json!({})) })
            .events(EVENTS)
            .handle(|_: &Context<()>| async move { Ok(json!({})) });

        let mut events = service.event_names();
        events.sort();
        assert_eq!(
            events,
            vec!["checkout.started", "seat.added", "seat.reserved"]
        );
    }

    #[tokio::test]
    async fn command_and_event_handlers_can_share_a_name() {
        let service = test_service()
            .command("shared")
            .handle(|ctx: &Context<()>| {
                let kind = format!("{:?}", ctx.message().kind);
                async move { Ok(json!({ "kind": kind })) }
            })
            .event("shared")
            .handle(|ctx: &Context<()>| {
                let event_id = ctx.message().id().map(|s| s.to_string());
                async move { Ok(json!({ "event_id": event_id })) }
            });
        let event_message =
            Message::new("shared", MessageKind::Event, br#"{}"#.to_vec()).with_id("evt-1");

        let command_result = service
            .dispatch("shared", json!({}), Session::new())
            .await
            .unwrap();
        let event_result = service.dispatch_message(&event_message).await.unwrap();

        assert_eq!(command_result, json!({ "kind": "Command" }));
        assert_eq!(event_result, json!({ "event_id": "evt-1" }));
        assert!(service.handles_message(MessageKind::Command, "shared"));
        assert!(service.handles_message(MessageKind::Event, "shared"));
    }

    #[tokio::test]
    async fn dispatch_message_delivers_payload_json_by_default() {
        let service = test_service()
            .event("checkout.started")
            .handle(|ctx: &Context<()>| {
                let has_checkout_id = ctx.has_fields(&["checkout_id"]);
                let event_id = ctx.message().id().map(|s| s.to_string());
                let checkout_id = ctx.raw_input()["checkout_id"]
                    .as_str()
                    .map(|s| s.to_string());
                let user_id = ctx.user_id().map(|s| s.to_string());
                async move {
                    if !has_checkout_id {
                        return Err(HandlerError::Rejected("missing checkout_id".into()));
                    }

                    Ok(json!({
                        "event_id": event_id,
                        "checkout_id": checkout_id.unwrap(),
                        "user_id": user_id?,
                    }))
                }
            });
        let message = Message {
            id: Some("evt-1".to_string()),
            name: "checkout.started".to_string(),
            kind: MessageKind::Event,
            payload: br#"{"checkout_id":"checkout-1"}"#.to_vec(),
            content_type: "application/json".to_string(),
            metadata: vec![("X-Hasura-User-Id".to_string(), "user-1".to_string())],
        };

        let result = service.dispatch_message(&message).await.unwrap();

        assert_eq!(
            result,
            json!({ "event_id": "evt-1", "checkout_id": "checkout-1", "user_id": "user-1" })
        );
    }

    #[tokio::test]
    async fn dispatch_message_always_exposes_message_metadata() {
        let service = test_service().event("seat.reserved").guarded(
            |ctx| ctx.message().id().is_some(),
            |ctx: &Context<()>| {
                let input: Result<Value, _> = ctx.input();
                let message = ctx.message();
                let event_id = message.id().map(|s| s.to_string());
                let name = message.name().to_string();
                let correlation_id = message.correlation_id().map(|s| s.to_string());
                async move {
                    let input = input?;
                    Ok(json!({
                        "event_id": event_id,
                        "name": name,
                        "correlation_id": correlation_id,
                        "seat_id": input["seat_id"].as_str().unwrap(),
                    }))
                }
            },
        );
        let message = Message {
            id: Some("evt-2".to_string()),
            name: "seat.reserved".to_string(),
            kind: MessageKind::Event,
            payload: br#"{"seat_id":"A-7"}"#.to_vec(),
            content_type: "application/json".to_string(),
            metadata: vec![("Correlation_ID".to_string(), "checkout-1".to_string())],
        };

        let result = service.dispatch_message(&message).await.unwrap();

        assert_eq!(
            result,
            json!({
                "event_id": "evt-2",
                "name": "seat.reserved",
                "correlation_id": "checkout-1",
                "seat_id": "A-7",
            })
        );
    }

    #[tokio::test]
    async fn guard_passes() {
        let service = test_service().command("greet").guarded(
            |ctx| ctx.has_fields(&["name"]),
            |ctx: &Context<()>| {
                let name = ctx.raw_input()["name"].as_str().map(|s| s.to_string());
                async move { Ok(json!({ "hello": name.unwrap() })) }
            },
        );
        let result = service
            .dispatch("greet", json!({ "name": "Pat" }), Session::new())
            .await
            .unwrap();
        assert_eq!(result, json!({ "hello": "Pat" }));
    }

    #[tokio::test]
    async fn guard_rejects() {
        let service = test_service().command("greet").guarded(
            |ctx| ctx.has_fields(&["name"]),
            |_ctx: &Context<()>| async move {
                panic!("handler should not run");
                #[allow(unreachable_code)]
                Ok(json!({}))
            },
        );
        let result = service
            .dispatch("greet", json!({ "wrong": 1 }), Session::new())
            .await;
        assert!(matches!(result, Err(HandlerError::GuardRejected(ref s)) if s == "greet"));
    }

    #[tokio::test]
    async fn guard_checks_session() {
        let service = test_service().command("admin").guarded(
            |ctx| ctx.role() == Some("admin"),
            |_ctx: &Context<()>| async move { Ok(json!({ "ok": true })) },
        );

        // No role
        assert!(service
            .dispatch("admin", json!({}), Session::new())
            .await
            .is_err());

        // Admin role
        let mut session = Session::new();
        session.set("x-hasura-role", "admin");
        assert!(service.dispatch("admin", json!({}), session).await.is_ok());
    }

    #[tokio::test]
    async fn dispatch_request_success() {
        let service = test_service()
            .command("ping")
            .handle(|_ctx: &Context<()>| async move { Ok(json!({ "pong": true })) });
        let request = CommandRequest {
            command: "ping".to_string(),
            input: json!({}),
            session_variables: HashMap::new(),
        };
        let response = service.dispatch_request(&request).await;
        assert_eq!(response.status, 200);
        assert_eq!(response.body, json!({ "pong": true }));
    }

    #[tokio::test]
    async fn dispatch_request_error_codes() {
        let service = test_service()
            .command("reject")
            .handle(|_: &Context<()>| async move { Err(HandlerError::Rejected("no".into())) })
            .command("unauth")
            .handle(|ctx: &Context<()>| {
                let user_id = ctx.user_id().map(|s| s.to_string());
                async move {
                    let _ = user_id?;
                    Ok(json!({}))
                }
            });

        let resp = service
            .dispatch_request(&CommandRequest {
                command: "unknown".to_string(),
                input: json!({}),
                session_variables: HashMap::new(),
            })
            .await;
        assert_eq!(resp.status, 404);

        let resp = service
            .dispatch_request(&CommandRequest {
                command: "reject".to_string(),
                input: json!({}),
                session_variables: HashMap::new(),
            })
            .await;
        assert_eq!(resp.status, 422);

        let resp = service
            .dispatch_request(&CommandRequest {
                command: "unauth".to_string(),
                input: json!({}),
                session_variables: HashMap::new(),
            })
            .await;
        assert_eq!(resp.status, 401);
    }

    #[tokio::test]
    async fn dispatch_request_passes_session() {
        let service = test_service()
            .command("whoami")
            .handle(|ctx: &Context<()>| {
                let user_id = ctx.user_id().map(|s| s.to_string());
                async move {
                    let user_id = user_id?;
                    Ok(json!({ "user_id": user_id }))
                }
            });
        let mut vars = HashMap::new();
        vars.insert("x-hasura-user-id".to_string(), "user-99".to_string());
        let request = CommandRequest {
            command: "whoami".to_string(),
            input: json!({}),
            session_variables: vars,
        };
        let response = service.dispatch_request(&request).await;
        assert_eq!(response.status, 200);
        assert_eq!(response.body, json!({ "user_id": "user-99" }));
    }

    #[test]
    fn command_request_requires_session_variables_field() {
        let json = r#"{"command":"ping","input":{}}"#;
        let result: Result<CommandRequest, _> = serde_json::from_str(json);
        assert!(result.is_err());
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
