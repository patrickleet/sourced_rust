//! Routes and service dispatch for microsvc.
//!
//! `Routes<D>` holds one dependency value and its command/event handlers.
//! `Service` is the deployment-level router that collects one or more route
//! bundles. Each handler receives a `Context<D>` and returns
//! `Result<Value, HandlerError>`.
//!
//! ## Example
//!
//! The handler closure returns a future, and `dispatch` is awaited:
//!
//! ```ignore
//! use distributed::microsvc;
//! use serde_json::json;
//!
//! let routes = microsvc::Routes::new()
//!     .with_dependencies(())
//!     .command("order.create")
//!     .handle(|ctx| {
//!         let input = ctx.input::<CreateOrderInput>();
//!         async move { Ok(json!({ "id": input?.id })) }
//!     });
//! let service = microsvc::Service::new().routes(routes);
//!
//! let result = service
//!     .dispatch("order.create", json!({"id": "1"}), Session::new())
//!     .await?;
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
#[cfg(feature = "metrics")]
use std::time::Instant;

use serde_json::Value;

use super::context::Context;
use super::dependencies::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasReadModelStore, HasRepo,
    RepoReadModelDependencies,
};
use super::error::HandlerError;
use super::session::Session;
use crate::bus::{
    Bus, Message, MessageKind, MessagePublisher, RunOptions, SubscriptionPlan, TransportError,
};
use crate::failure_log::{FailureComponent, FailureMessageFields, FailureOperation, FailureRecord};
use crate::outbox::OutboxPublisherConfig;
use crate::outbox_worker::BusOutboxPublishHook;

/// The bus run behavior captured by [`Service::with_bus`](crate::microsvc::Service::with_bus).
pub(crate) type ServiceRunner = Box<
    dyn Fn(
            Arc<Service>,
            RunOptions,
        ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send>>
        + Send
        + Sync,
>;

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

type OutboxConfigurator<D> = fn(&mut D, DynBusPublisher, String, Duration, u32, Option<String>);

trait ErasedRoutes: Send + Sync {
    fn handler_specs(&self) -> &[HandlerSpec];

    fn dispatch<'a>(
        &'a self,
        message: &'a Message,
        input: Value,
        session: Session,
    ) -> HandlerFuture<'a>;

    fn configure_outbox_publisher(
        &mut self,
        publisher: DynBusPublisher,
        worker_id: String,
        lease: Duration,
        max_attempts: u32,
        service_name: Option<String>,
    );
}

trait DynPublish: Send + Sync {
    fn publish<'a>(
        &'a self,
        message: Message,
    ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>>;
}

struct BusDynPublisher<B> {
    bus: Arc<B>,
}

impl<B: Bus> DynPublish for BusDynPublisher<B> {
    fn publish<'a>(
        &'a self,
        message: Message,
    ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
        Box::pin(async move {
            match message.kind {
                MessageKind::Command => self.bus.send_message(message).await,
                MessageKind::Event => self.bus.publish_message(message).await,
            }
        })
    }
}

#[derive(Clone)]
pub(crate) struct DynBusPublisher {
    inner: Arc<dyn DynPublish>,
}

impl DynBusPublisher {
    pub(crate) fn new<B>(bus: Arc<B>) -> Self
    where
        B: Bus + 'static,
    {
        Self {
            inner: Arc::new(BusDynPublisher { bus }),
        }
    }
}

impl MessagePublisher for DynBusPublisher {
    fn publish(
        &self,
        message: Message,
    ) -> impl Future<Output = Result<(), TransportError>> + Send + '_ {
        self.inner.publish(message)
    }
}

fn configure_outbox_for<D>(
    dependencies: &mut D,
    publisher: DynBusPublisher,
    worker_id: String,
    lease: Duration,
    max_attempts: u32,
    service_name: Option<String>,
) where
    D: HasOutboxStore + ConfigurableOutboxPublisher,
    D::OutboxStore: 'static,
{
    let hook = BusOutboxPublishHook::new(dependencies.outbox_store(), publisher, max_attempts)
        .with_service(service_name);
    dependencies.configure_outbox_publisher(OutboxPublisherConfig::new(
        Arc::new(hook),
        worker_id,
        lease,
    ));
}

/// Builder returned by [`Routes::command`], [`Routes::event`],
/// [`Routes::events`], and [`Routes::handler`].
pub struct RouteBuilder<D> {
    routes: Routes<D>,
    spec: HandlerSpec,
}

impl<D: Send + Sync + 'static> RouteBuilder<D> {
    /// Register an async handler without a guard.
    pub fn handle<F>(self, handler: F) -> Routes<D>
    where
        F: for<'a> Handler<'a, D> + 'static,
    {
        self.routes
            .register_handler(self.spec, None, boxed_handler(handler))
    }

    /// Register an async handler with a (synchronous) guard.
    pub fn guarded<G, F>(self, guard: G, handler: F) -> Routes<D>
    where
        G: Fn(&Context<D>) -> bool + Send + Sync + 'static,
        F: for<'a> Handler<'a, D> + 'static,
    {
        self.routes
            .register_handler(self.spec, Some(Arc::new(guard)), boxed_handler(handler))
    }
}

/// A typed bundle of command/event handlers and the dependency value they use.
///
/// Handlers are keyed by kind, then name, so dispatch looks up by `&str`
/// without allocating a key.
pub struct Routes<D> {
    dependencies: D,
    handlers: HashMap<MessageKind, HashMap<String, RegisteredHandler<D>>>,
    handler_specs: Vec<HandlerSpec>,
    outbox_configurator: Option<OutboxConfigurator<D>>,
}

impl<D: Send + Sync + 'static> Routes<D> {
    /// Build routes around an already-assembled dependency value.
    pub(crate) fn from_dependencies(dependencies: D) -> Self {
        Self {
            dependencies,
            handlers: HashMap::new(),
            handler_specs: Vec::new(),
            outbox_configurator: None,
        }
    }

    fn with_outbox_configurator(mut self, configurator: OutboxConfigurator<D>) -> Self {
        self.outbox_configurator = Some(configurator);
        self
    }

    /// Fail fast if handlers are already registered. Dependency builders
    /// reconstruct the route bundle around a new dependency type, which would
    /// otherwise silently drop previously registered handlers.
    fn assert_no_registrations(&self, builder: &str) {
        assert!(
            self.handlers.is_empty() && self.handler_specs.is_empty(),
            "Routes::{builder} must be called before registering handlers"
        );
    }

    /// Get a reference to the route dependencies.
    pub fn dependencies(&self) -> &D {
        &self.dependencies
    }

    /// Get the aggregate repository for routes whose dependencies expose one.
    pub fn repo(&self) -> &D::Repo
    where
        D: HasRepo,
    {
        self.dependencies.repo()
    }

    /// Get the read-model store for routes whose dependencies expose one.
    pub fn read_model_store(&self) -> &D::ReadModelStore
    where
        D: HasReadModelStore,
    {
        self.dependencies.read_model_store()
    }

    /// Start registering a command handler that consumes JSON payload input.
    pub fn command(self, name: &'static str) -> RouteBuilder<D> {
        self.handler(HandlerSpec::command(name))
    }

    /// Start registering an event handler that consumes JSON payload input.
    pub fn event(self, name: &'static str) -> RouteBuilder<D> {
        self.handler(HandlerSpec::event(name))
    }

    /// Start registering an event handler for several event names that consume JSON
    /// payload input.
    pub fn events(self, names: &'static [&'static str]) -> RouteBuilder<D> {
        self.handler(HandlerSpec::events(names))
    }

    /// Start registering a handler from a transport-visible spec.
    pub fn handler(self, spec: HandlerSpec) -> RouteBuilder<D> {
        RouteBuilder { routes: self, spec }
    }

    fn register_handler(
        mut self,
        spec: HandlerSpec,
        guard: Option<Arc<GuardFn<D>>>,
        handle: Arc<HandlerFn<D>>,
    ) -> Self {
        let by_name = self.handlers.entry(spec.kind).or_default();
        let names = spec.names();
        for (position, name) in names.iter().enumerate() {
            assert!(
                !by_name.contains_key(*name) && !names[..position].contains(name),
                "duplicate route registration for {:?} `{}`",
                spec.kind,
                name
            );
        }

        for name in names {
            by_name.insert(
                name.to_string(),
                RegisteredHandler {
                    guard: guard.clone(),
                    handle: handle.clone(),
                },
            );
        }
        self.handler_specs.push(spec);
        self
    }

    fn registered_keys(&self) -> Vec<(MessageKind, String)> {
        self.handlers
            .iter()
            .flat_map(|(kind, by_name)| by_name.keys().map(move |name| (*kind, name.clone())))
            .collect()
    }

    async fn invoke(
        &self,
        message: &Message,
        input: Value,
        session: Session,
    ) -> Result<Value, HandlerError> {
        // Clone the handler/guard Arcs so the handler map is not borrowed across
        // the (awaited) handler future.
        let (guard, handle) = {
            let handler = self
                .handlers
                .get(&message.kind)
                .and_then(|by_name| by_name.get(message.name.as_str()))
                .ok_or_else(|| HandlerError::UnknownCommand(message.name.clone()))?;
            (handler.guard.clone(), handler.handle.clone())
        };
        let ctx = Context::new(message, input, session, &self.dependencies);

        // Run guard (synchronous) if present.
        if let Some(guard) = &guard {
            if !guard(&ctx) {
                return Err(HandlerError::GuardRejected(message.name.clone()));
            }
        }

        handle(&ctx).await
    }
}

impl<D> ErasedRoutes for Routes<D>
where
    D: Send + Sync + 'static,
{
    fn handler_specs(&self) -> &[HandlerSpec] {
        &self.handler_specs
    }

    fn dispatch<'a>(
        &'a self,
        message: &'a Message,
        input: Value,
        session: Session,
    ) -> HandlerFuture<'a> {
        Box::pin(self.invoke(message, input, session))
    }

    fn configure_outbox_publisher(
        &mut self,
        publisher: DynBusPublisher,
        worker_id: String,
        lease: Duration,
        max_attempts: u32,
        service_name: Option<String>,
    ) {
        if let Some(configurator) = self.outbox_configurator {
            configurator(
                &mut self.dependencies,
                publisher,
                worker_id,
                lease,
                max_attempts,
                service_name,
            );
        }
    }
}

/// A microservice deployment that routes messages to one or more route bundles.
pub struct Service {
    name: Option<String>,
    routes: Vec<Box<dyn ErasedRoutes>>,
    index: HashMap<MessageKind, HashMap<String, usize>>,
    handler_specs: Vec<HandlerSpec>,
    runner: Option<ServiceRunner>,
}

impl Service {
    /// Start building a deployment-level service.
    pub fn new() -> Self {
        Self {
            name: None,
            routes: Vec::new(),
            index: HashMap::new(),
            handler_specs: Vec::new(),
            runner: None,
        }
    }

    /// Build a service from a single route bundle.
    pub fn route<D>(routes: Routes<D>) -> Self
    where
        D: Send + Sync + 'static,
    {
        Self::new().routes(routes)
    }

    /// Assign a stable service/deployment identity.
    ///
    /// Broker-backed buses use this as the default durable consumer group when the
    /// bus itself was not configured with an explicit group. Use the same name for
    /// every replica of one service deployment; use different names for independent
    /// event consumers that each need their own event copy.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The stable service/deployment identity, if one was configured.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Install the bus run behavior (used by `with_bus`).
    pub(crate) fn set_runner(&mut self, runner: ServiceRunner) {
        self.runner = Some(runner);
    }

    /// Take the installed bus run behavior (used by `run`).
    pub(crate) fn take_runner(&mut self) -> Option<ServiceRunner> {
        self.runner.take()
    }

    /// Add a typed route bundle to this service.
    pub fn routes<D>(mut self, routes: Routes<D>) -> Self
    where
        D: Send + Sync + 'static,
    {
        self.add_routes(routes);
        self
    }

    fn add_routes<D>(&mut self, routes: Routes<D>)
    where
        D: Send + Sync + 'static,
    {
        let keys = routes.registered_keys();
        for (kind, name) in &keys {
            assert!(
                !self.handles_message(*kind, name),
                "duplicate route registration for {:?} `{}`",
                kind,
                name
            );
        }

        let route_index = self.routes.len();
        for (kind, name) in keys {
            self.index
                .entry(kind)
                .or_default()
                .insert(name, route_index);
        }
        self.handler_specs.extend_from_slice(routes.handler_specs());
        self.routes.push(Box::new(routes));
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
        #[cfg(feature = "metrics")]
        let started = Instant::now();
        let result = self.dispatch_command_inner(command, input, session).await;
        #[cfg(feature = "metrics")]
        {
            let error = result.as_ref().err();
            crate::metrics::record_microsvc_dispatch(
                self.name(),
                MessageKind::Command,
                crate::telemetry::handler_message_label(command, error),
                error
                    .map(crate::telemetry::handler_error_status)
                    .unwrap_or(crate::telemetry::dispatch_status::SUCCESS),
                started.elapsed(),
            );
        }
        result
    }

    async fn dispatch_command_inner(
        &self,
        command: &str,
        input: Value,
        session: Session,
    ) -> Result<Value, HandlerError> {
        if !self.handles_message(MessageKind::Command, command) {
            let error = HandlerError::UnknownCommand(command.to_string());
            self.emit_handler_failure(
                FailureOperation::Dispatch,
                FailureMessageFields::unknown(),
                &error,
            );
            return Err(error);
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

        let result = self
            .invoke_with_dispatch_span(&message, input, session)
            .await;
        if let Err(error) = &result {
            self.emit_handler_failure(
                FailureOperation::Dispatch,
                FailureMessageFields::from_message(&message),
                error,
            );
        }
        result
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
                body: serde_json::json!({ "error": e.client_facing_message() }),
            },
        }
    }

    /// Dispatch a transport message.
    pub async fn dispatch_message(&self, message: &Message) -> Result<Value, HandlerError> {
        #[cfg(feature = "metrics")]
        let started = Instant::now();
        let result = self.dispatch_message_inner(message).await;
        #[cfg(feature = "metrics")]
        {
            let error = result.as_ref().err();
            crate::metrics::record_microsvc_dispatch(
                self.name(),
                message.kind,
                crate::telemetry::handler_message_label(message.name(), error),
                error
                    .map(crate::telemetry::handler_error_status)
                    .unwrap_or(crate::telemetry::dispatch_status::SUCCESS),
                started.elapsed(),
            );
        }
        result
    }

    async fn dispatch_message_inner(&self, message: &Message) -> Result<Value, HandlerError> {
        if !self.handles_message(message.kind, &message.name) {
            let error = HandlerError::UnknownCommand(message.name.clone());
            self.emit_handler_failure(
                FailureOperation::Dispatch,
                FailureMessageFields::unknown(),
                &error,
            );
            return Err(error);
        }

        let input = match message_to_json_input(message) {
            Ok(input) => input,
            // Binary payloads (bitcode, octet-stream) legitimately fail JSON
            // parsing: handlers for those read `ctx.message().payload` directly,
            // so a `Null` input is the intended fallback. A payload that
            // *claims* to be JSON but does not parse is a decode error — surface
            // it instead of silently nulling the input.
            Err(_) if !is_json_content_type(&message.content_type) => Value::Null,
            Err(err) => {
                self.emit_handler_failure(
                    FailureOperation::Dispatch,
                    FailureMessageFields::from_message(message),
                    &err,
                );
                return Err(err);
            }
        };
        let session = message_to_session(message);
        let result = self
            .invoke_with_dispatch_span(message, input, session)
            .await;
        if let Err(error) = &result {
            self.emit_handler_failure(
                FailureOperation::Dispatch,
                FailureMessageFields::from_message(message),
                error,
            );
        }
        result
    }

    async fn invoke_with_dispatch_span(
        &self,
        message: &Message,
        input: Value,
        session: Session,
    ) -> Result<Value, HandlerError> {
        #[cfg(feature = "otel")]
        {
            use tracing::Instrument as _;

            let span = microsvc_dispatch_span(message);
            crate::trace_context::set_span_parent_from_metadata_if_no_current_span(
                &span,
                &message.metadata,
            );
            return self.invoke(message, input, session).instrument(span).await;
        }

        #[cfg(not(feature = "otel"))]
        {
            self.invoke(message, input, session).await
        }
    }

    async fn invoke(
        &self,
        message: &Message,
        input: Value,
        session: Session,
    ) -> Result<Value, HandlerError> {
        let route_index = self
            .index
            .get(&message.kind)
            .and_then(|by_name| by_name.get(message.name.as_str()))
            .copied()
            .ok_or_else(|| HandlerError::UnknownCommand(message.name.clone()))?;
        #[cfg(feature = "otel")]
        let handler_span = microsvc_handler_span(message);
        let dispatch = self.routes[route_index].dispatch(message, input, session);

        #[cfg(feature = "otel")]
        {
            use tracing::Instrument as _;

            return dispatch.instrument(handler_span).await;
        }

        #[cfg(not(feature = "otel"))]
        {
            dispatch.await
        }
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
        self.index
            .values()
            .any(|by_name| by_name.contains_key(name))
    }

    /// Return whether this service has a handler for this message kind and name.
    pub fn handles_message(&self, kind: MessageKind, name: &str) -> bool {
        self.index
            .get(&kind)
            .is_some_and(|by_name| by_name.contains_key(name))
    }

    /// Return whether this service has an event handler for the message name.
    pub fn handles_event(&self, name: &str) -> bool {
        self.handles_message(MessageKind::Event, name)
    }

    /// Configure every route bundle that supports immediate outbox publishing.
    pub(crate) fn configure_outbox_publishers(
        &mut self,
        publisher: DynBusPublisher,
        worker_id: String,
        lease: Duration,
        max_attempts: u32,
    ) {
        let service_name = self.name.clone();
        for route in &mut self.routes {
            route.configure_outbox_publisher(
                publisher.clone(),
                worker_id.clone(),
                lease,
                max_attempts,
                service_name.clone(),
            );
        }
    }

    fn emit_handler_failure(
        &self,
        operation: FailureOperation,
        message: FailureMessageFields,
        error: &HandlerError,
    ) {
        FailureRecord::from_handler_error(FailureComponent::Microsvc, operation, error)
            .with_service(self.name())
            .with_message(message)
            .emit();
    }
}

impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Dependency builders: `Routes::new().with_repo(..)`
// =============================================================================

impl Default for Routes<()> {
    fn default() -> Self {
        Self::new()
    }
}

impl Routes<()> {
    /// Start building a typed route bundle.
    pub fn new() -> Self {
        Self::from_dependencies(())
    }

    /// Use any custom dependency value for this route bundle.
    pub fn with_dependencies<D>(self, dependencies: D) -> Routes<D>
    where
        D: Send + Sync + 'static,
    {
        self.assert_no_registrations("with_dependencies");
        Routes::from_dependencies(dependencies)
    }

    /// Use an aggregate repository as the route bundle's dependency.
    pub fn with_repo<R>(self, repo: R) -> Routes<R>
    where
        R: HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
    {
        self.assert_no_registrations("with_repo");
        Routes::from_dependencies(repo).with_outbox_configurator(configure_outbox_for::<R>)
    }

    /// Use a read-model store as the route bundle's dependency.
    pub fn with_read_model_store<S>(self, read_model_store: S) -> Routes<S>
    where
        S: HasReadModelStore + Send + Sync + 'static,
    {
        self.assert_no_registrations("with_read_model_store");
        Routes::from_dependencies(read_model_store)
    }
}

impl<R> Routes<R>
where
    R: HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    /// Add a read-model store alongside the aggregate repository, so handlers can
    /// reach both via `ctx.repo()` and `ctx.read_model_store()`. Call after
    /// `with_repo`.
    pub fn with_read_model_store<S>(
        self,
        read_model_store: S,
    ) -> Routes<RepoReadModelDependencies<R, S>>
    where
        S: HasReadModelStore + Send + Sync + 'static,
    {
        self.assert_no_registrations("with_read_model_store");
        Routes::from_dependencies(RepoReadModelDependencies::new(
            self.dependencies,
            read_model_store,
        ))
        .with_outbox_configurator(configure_outbox_for::<RepoReadModelDependencies<R, S>>)
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

/// Whether a content type declares a JSON payload (`application/json` or any
/// `+json` structured suffix), ignoring parameters like `;charset=utf-8`.
fn is_json_content_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    essence == "application/json" || essence.ends_with("+json")
}

#[cfg(feature = "otel")]
fn microsvc_dispatch_span(message: &Message) -> tracing::Span {
    crate::telemetry::microsvc_dispatch_span(message)
}

#[cfg(feature = "otel")]
fn microsvc_handler_span(message: &Message) -> tracing::Span {
    crate::telemetry::microsvc_handler_span(message)
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
    use crate::{
        sourced, AggregateBuilder, AggregateRepository, Entity, InMemoryRepository, Queueable,
        QueuedRepository,
    };
    use serde_json::json;

    #[derive(Default)]
    struct RouteComboAggregate {
        entity: Entity,
    }

    #[sourced(entity)]
    impl RouteComboAggregate {
        #[event("created")]
        fn create(&mut self) {
            self.entity.set_id("route-combo");
        }
    }

    type RouteComboRepo =
        AggregateRepository<QueuedRepository<InMemoryRepository>, RouteComboAggregate>;
    type RouteComboDeps = RepoReadModelDependencies<RouteComboRepo, InMemoryRepository>;

    fn test_routes() -> Routes<()> {
        Routes::new().with_dependencies(())
    }

    fn test_service(routes: Routes<()>) -> Service {
        Service::new().routes(routes)
    }

    #[test]
    fn named_service_preserves_identity_with_route_bundles() {
        let routes = Routes::new().with_read_model_store(crate::InMemoryRepository::new());
        let service = Service::new().named("todo-api").routes(routes);

        assert_eq!(service.name(), Some("todo-api"));
        assert_eq!(
            crate::bus::MessageRouter::consumer_group(&service),
            Some("todo-api")
        );
    }

    #[tokio::test]
    async fn service_collects_route_bundles_with_different_dependencies() {
        let service = Service::new()
            .routes(
                Routes::new()
                    .with_dependencies(String::from("orders"))
                    .command("string.dep")
                    .handle(|ctx: &Context<String>| {
                        let dep = ctx.dependencies().clone();
                        async move { Ok(json!({ "dependency": dep })) }
                    }),
            )
            .routes(
                Routes::new()
                    .with_dependencies(7_u32)
                    .event("number.dep")
                    .handle(|ctx: &Context<u32>| {
                        let dep = *ctx.dependencies();
                        async move { Ok(json!({ "dependency": dep })) }
                    }),
            );

        let command = service
            .dispatch("string.dep", json!({}), Session::new())
            .await
            .unwrap();
        let event = service
            .dispatch_message(&Message::new(
                "number.dep",
                MessageKind::Event,
                br#"{}"#.to_vec(),
            ))
            .await
            .unwrap();

        assert_eq!(command, json!({ "dependency": "orders" }));
        assert_eq!(event, json!({ "dependency": 7 }));
        assert_eq!(
            service.subscription_plan(),
            SubscriptionPlan {
                commands: vec!["string.dep".to_string()],
                events: vec!["number.dep".to_string()],
            }
        );
    }

    #[tokio::test]
    async fn service_dispatches_all_route_dependency_builder_combinations() {
        let repo_only = InMemoryRepository::new().queued().aggregate();
        let combo_repo = InMemoryRepository::new().queued().aggregate();
        let service = Service::new()
            .routes(
                Routes::new()
                    .with_dependencies(String::from("custom"))
                    .command("custom.route")
                    .handle(|ctx: &Context<String>| {
                        let dependency = ctx.dependencies().clone();
                        async move { Ok(json!({ "route": dependency })) }
                    }),
            )
            .routes(
                Routes::new()
                    .with_repo(repo_only)
                    .command("repo.route")
                    .handle(|ctx: &Context<RouteComboRepo>| {
                        let _ = ctx.repo();
                        async move { Ok(json!({ "route": "repo" })) }
                    }),
            )
            .routes(
                Routes::new()
                    .with_read_model_store(InMemoryRepository::new())
                    .event("read.route")
                    .handle(|ctx: &Context<InMemoryRepository>| {
                        let _ = ctx.read_model_store();
                        async move { Ok(json!({ "route": "read" })) }
                    }),
            )
            .routes(
                Routes::new()
                    .with_repo(combo_repo)
                    .with_read_model_store(InMemoryRepository::new())
                    .command("repo-read.route")
                    .handle(|ctx: &Context<RouteComboDeps>| {
                        let _ = ctx.repo();
                        let _ = ctx.read_model_store();
                        async move { Ok(json!({ "route": "repo-read" })) }
                    }),
            );

        let custom = service
            .dispatch("custom.route", json!({}), Session::new())
            .await
            .unwrap();
        let repo = service
            .dispatch("repo.route", json!({}), Session::new())
            .await
            .unwrap();
        let read = service
            .dispatch_message(&Message::new(
                "read.route",
                MessageKind::Event,
                br#"{}"#.to_vec(),
            ))
            .await
            .unwrap();
        let repo_read = service
            .dispatch("repo-read.route", json!({}), Session::new())
            .await
            .unwrap();

        assert_eq!(custom, json!({ "route": "custom" }));
        assert_eq!(repo, json!({ "route": "repo" }));
        assert_eq!(read, json!({ "route": "read" }));
        assert_eq!(repo_read, json!({ "route": "repo-read" }));
        assert_eq!(
            service.subscription_plan(),
            SubscriptionPlan {
                commands: vec![
                    "custom.route".to_string(),
                    "repo.route".to_string(),
                    "repo-read.route".to_string(),
                ],
                events: vec!["read.route".to_string()],
            }
        );
    }

    #[test]
    fn duplicate_route_names_within_bundle_are_rejected() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _routes = test_routes()
                .command("same")
                .handle(|_: &Context<()>| async move { Ok(json!({})) })
                .command("same")
                .handle(|_: &Context<()>| async move { Ok(json!({})) });
        }));

        assert!(result.is_err());
    }

    #[test]
    fn duplicate_route_bundle_add_is_rejected_atomically() {
        let mut service = Service::new().routes(
            test_routes()
                .command("same")
                .handle(|_: &Context<()>| async move { Ok(json!({})) }),
        );
        let conflicting = Routes::new()
            .with_dependencies(7_u32)
            .command("same")
            .handle(|_: &Context<u32>| async move { Ok(json!({})) })
            .command("new")
            .handle(|_: &Context<u32>| async move { Ok(json!({})) });

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.add_routes(conflicting);
        }));

        assert!(result.is_err());
        assert!(service.handles_message(MessageKind::Command, "same"));
        assert!(!service.handles_message(MessageKind::Command, "new"));
        assert_eq!(service.routes.len(), 1);
        assert_eq!(service.command_names(), vec!["same"]);
    }

    #[tokio::test]
    async fn dispatch_returns_handler_result() {
        let service = test_service(
            test_routes()
                .command("ping")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({ "pong": true })) }),
        );
        let result = service
            .dispatch("ping", json!({}), Session::new())
            .await
            .unwrap();
        assert_eq!(result, json!({ "pong": true }));
    }

    #[tokio::test]
    async fn unknown_command() {
        // This dispatch records the same {unnamed, unknown, unknown_command}
        // series into the process-global registry that
        // `metrics_bucket_unknown_command_under_fixed_message_label` asserts
        // an exact count on — serialize against it.
        #[cfg(feature = "metrics")]
        let _guard = crate::metrics::async_lock_for_tests().await;

        let service = test_service(
            test_routes()
                .command("ping")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({})) }),
        );
        let result = service.dispatch("unknown", json!({}), Session::new()).await;
        assert!(matches!(result, Err(HandlerError::UnknownCommand(ref s)) if s == "unknown"));
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn metrics_bucket_unknown_command_under_fixed_message_label() {
        let _guard = crate::metrics::async_lock_for_tests().await;
        crate::metrics::reset_for_tests();

        let service = test_service(
            test_routes()
                .command("ping")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({})) }),
        );

        let result = service
            .dispatch("attacker-controlled-path", json!({}), Session::new())
            .await;
        assert!(matches!(result, Err(HandlerError::UnknownCommand(_))));

        let text = crate::metrics::prometheus_text();
        assert!(
            text.contains(
                "distributed_microsvc_dispatch_total{service=\"unnamed\",message_kind=\"command\",message=\"unknown\",status=\"unknown_command\"} 1"
            ),
            "unknown commands should use a bounded message label:\n{text}"
        );
        assert!(
            !text.contains("attacker-controlled-path"),
            "unknown command input must not become a metric label:\n{text}"
        );
    }

    #[tokio::test]
    async fn handler_error_propagates() {
        let service = test_service(test_routes().command("fail").handle(
            |_ctx: &Context<()>| async move { Err(HandlerError::Rejected("nope".into())) },
        ));
        let result = service.dispatch("fail", json!({}), Session::new()).await;
        assert!(matches!(result, Err(HandlerError::Rejected(ref s)) if s == "nope"));
    }

    #[tokio::test]
    async fn decode_error_from_bad_payload() {
        #[derive(serde::Deserialize)]
        struct Input {
            _name: String,
        }

        let service = test_service(test_routes().command("typed").handle(|ctx: &Context<()>| {
            let input = ctx.input::<Input>();
            async move {
                let _input = input?;
                Ok(json!({}))
            }
        }));
        let result = service
            .dispatch("typed", json!({ "wrong": 1 }), Session::new())
            .await;
        assert!(matches!(result, Err(HandlerError::DecodeFailed(_))));
    }

    #[test]
    fn command_names_list() {
        let service = test_service(
            test_routes()
                .command("a")
                .handle(|_: &Context<()>| async move { Ok(json!({})) })
                .command("b")
                .handle(|_: &Context<()>| async move { Ok(json!({})) }),
        );
        let mut cmds = service.command_names();
        cmds.sort();
        assert_eq!(cmds, vec!["a", "b"]);
    }

    #[test]
    fn subscription_plan_separates_commands_and_events() {
        const EVENTS: &[&str] = &["checkout.started", "seat.reserved"];

        let service = test_service(
            test_routes()
                .command("checkout.start")
                .handle(|_: &Context<()>| async move { Ok(json!({})) })
                .events(EVENTS)
                .guarded(|_| true, |_: &Context<()>| async move { Ok(json!({})) }),
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
    fn event_conveniences_record_event_names() {
        const EVENTS: &[&str] = &["seat.added", "seat.reserved"];

        let service = test_service(
            test_routes()
                .event("checkout.started")
                .handle(|_: &Context<()>| async move { Ok(json!({})) })
                .events(EVENTS)
                .handle(|_: &Context<()>| async move { Ok(json!({})) }),
        );

        let mut events = service.event_names();
        events.sort();
        assert_eq!(
            events,
            vec!["checkout.started", "seat.added", "seat.reserved"]
        );
    }

    #[tokio::test]
    async fn command_and_event_handlers_can_share_a_name() {
        let service = test_service(
            test_routes()
                .command("shared")
                .handle(|ctx: &Context<()>| {
                    let kind = format!("{:?}", ctx.message().kind);
                    async move { Ok(json!({ "kind": kind })) }
                })
                .event("shared")
                .handle(|ctx: &Context<()>| {
                    let event_id = ctx.message().id().map(|s| s.to_string());
                    async move { Ok(json!({ "event_id": event_id })) }
                }),
        );
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
        let service = test_service(test_routes().event("checkout.started").handle(
            |ctx: &Context<()>| {
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
            },
        ));
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
    async fn dispatch_message_surfaces_malformed_json_as_decode_error() {
        let service = test_service(test_routes().event("checkout.started").handle(
            |_ctx: &Context<()>| async move { panic!("handler must not run on a decode error") },
        ));
        let message = Message::new(
            "checkout.started",
            MessageKind::Event,
            br#"{"checkout_id": oops"#.to_vec(),
        );

        let err = service.dispatch_message(&message).await.unwrap_err();

        match err {
            HandlerError::DecodeFailed(detail) => {
                assert!(
                    detail.contains("invalid JSON payload") && detail.contains("checkout.started"),
                    "decode error should carry the parse failure, got: {detail}"
                );
            }
            other => panic!("expected DecodeFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_message_nulls_input_for_non_json_payloads() {
        let service = test_service(test_routes().event("blob.stored").handle(
            |ctx: &Context<()>| {
                let input_is_null = ctx.raw_input().is_null();
                let payload = ctx.message().payload().to_vec();
                async move { Ok(json!({ "null_input": input_is_null, "len": payload.len() })) }
            },
        ));
        let mut message = Message::new("blob.stored", MessageKind::Event, vec![0, 159, 146, 150]);
        message.content_type = "application/octet-stream".to_string();

        let result = service.dispatch_message(&message).await.unwrap();

        assert_eq!(result, json!({ "null_input": true, "len": 4 }));
    }

    #[tokio::test]
    async fn dispatch_message_always_exposes_message_metadata() {
        let service = test_service(test_routes().event("seat.reserved").guarded(
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
        ));
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
    async fn dispatch_exposes_trace_context_from_session_metadata() {
        let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let service = test_service(test_routes().command("checkout.start").handle(
            |ctx: &Context<()>| {
                let trace_context = ctx.message().trace_context();
                async move {
                    Ok(json!({
                        "traceparent": trace_context.traceparent,
                        "tracestate": trace_context.tracestate,
                    }))
                }
            },
        ));
        let session = Session::from_map(HashMap::from([
            ("traceparent".to_string(), traceparent.to_string()),
            ("tracestate".to_string(), "vendor=value".to_string()),
        ]));

        let result = service
            .dispatch("checkout.start", json!({}), session)
            .await
            .unwrap();

        assert_eq!(
            result,
            json!({ "traceparent": traceparent, "tracestate": "vendor=value" })
        );
    }

    #[tokio::test]
    async fn guard_passes() {
        let service = test_service(test_routes().command("greet").guarded(
            |ctx| ctx.has_fields(&["name"]),
            |ctx: &Context<()>| {
                let name = ctx.raw_input()["name"].as_str().map(|s| s.to_string());
                async move { Ok(json!({ "hello": name.unwrap() })) }
            },
        ));
        let result = service
            .dispatch("greet", json!({ "name": "Pat" }), Session::new())
            .await
            .unwrap();
        assert_eq!(result, json!({ "hello": "Pat" }));
    }

    #[tokio::test]
    async fn guard_rejects() {
        let service = test_service(test_routes().command("greet").guarded(
            |ctx| ctx.has_fields(&["name"]),
            |_ctx: &Context<()>| async move {
                panic!("handler should not run");
                #[allow(unreachable_code)]
                Ok(json!({}))
            },
        ));
        let result = service
            .dispatch("greet", json!({ "wrong": 1 }), Session::new())
            .await;
        assert!(matches!(result, Err(HandlerError::GuardRejected(ref s)) if s == "greet"));
    }

    #[tokio::test]
    async fn guard_checks_session() {
        let service = test_service(test_routes().command("admin").guarded(
            |ctx| ctx.role() == Some("admin"),
            |_ctx: &Context<()>| async move { Ok(json!({ "ok": true })) },
        ));

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
        let service = test_service(
            test_routes()
                .command("ping")
                .handle(|_ctx: &Context<()>| async move { Ok(json!({ "pong": true })) }),
        );
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
        let service = test_service(
            test_routes()
                .command("reject")
                .handle(|_: &Context<()>| async move { Err(HandlerError::Rejected("no".into())) })
                .command("unauth")
                .handle(|ctx: &Context<()>| {
                    let user_id = ctx.user_id().map(|s| s.to_string());
                    async move {
                        let _ = user_id?;
                        Ok(json!({}))
                    }
                }),
        );

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
        let service = test_service(test_routes().command("whoami").handle(|ctx: &Context<()>| {
            let user_id = ctx.user_id().map(|s| s.to_string());
            async move {
                let user_id = user_id?;
                Ok(json!({ "user_id": user_id }))
            }
        }));
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
