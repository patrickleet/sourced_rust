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
#[cfg(feature = "graphql")]
use std::time::SystemTime;

use serde_json::Value;

use super::causal::{CausalWorkspace, CausalWorkspaceError};
use super::context::Context;
use super::dependencies::{
    CausalRouteDependencies, ConfigurableOutboxPublisher, HasOutboxStore, HasReadModelStore,
    HasRepo, RepoReadModelDependencies,
};
use super::error::HandlerError;
use super::session::Session;
use crate::aggregate::Aggregate;
use crate::bus::{
    Bus, Message, MessageKind, MessagePublisher, RunOptions, SubscriptionPlan, TransportError,
};
#[cfg(feature = "graphql")]
use crate::command_ledger::{
    AttemptFence, CanonicalInputHash, CausalCommitBatch, CommandAttempt,
    CommandContractFingerprint, CommandId, CommandLedgerError, CommandLedgerKey,
    CommandLedgerState, CommandLookup, CommandLookupScope, CommandReplay, CommandReservation,
    PrincipalPartitionId, ReservationOutcome, TerminalCommandState,
};
#[cfg(feature = "graphql")]
use crate::command_ledger::{
    CausalRepositoryIdentity, CausalTransactionalCommit, CommandLedgerStore,
};
#[cfg(feature = "graphql")]
use crate::graphql::command_contract::CommandConsistency;
use crate::graphql::command_contract::{
    CommandOutcome, TypedCommandContract, TypedServiceCommandBinding,
};
#[cfg(feature = "graphql")]
use crate::graphql::command_input::canonicalize_command_input;
#[cfg(feature = "graphql")]
use crate::graphql::identity::VerifiedPrincipal;
use crate::graphql::{GraphqlOutputType, PreparedCommand, Projected, TypedCommand};
use crate::outbox::OutboxMessage;
use crate::outbox::OutboxPublisherConfig;
use crate::outbox_worker::BusOutboxPublishHook;
use crate::read_model::{ReadModelWritePlanBuilder, RelationalReadModel};
#[cfg(feature = "graphql")]
use crate::repository::CommitBatch;

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

/// Restricted metadata and staging context for one typed causal command.
///
/// The aggregate type is fixed by the route bundle. The context exposes owned
/// checkouts and staging operations, but never the dependency value, backend,
/// repository, or a commit method; the framework retains this route's fenced
/// durable commit capability and attaches the command-attempt fence after the
/// handler returns.
///
/// This is an API capability boundary, not a Rust sandbox. Application handler
/// code is trusted: a closure can still capture an external client/repository or
/// reach a global. Such out-of-band effects are outside the causal contract and
/// may repeat if an expired attempt is reclaimed. Only work staged through this
/// context receives the at-most-once committed-effects guarantee.
pub struct CausalCommandContext<'a, A>
where
    A: Aggregate + Send + Sync + 'static,
{
    message: &'a Message,
    session: &'a Session,
    workspace: &'a CausalWorkspace<'a, A>,
}

impl<'a, A> CausalCommandContext<'a, A>
where
    A: Aggregate + Send + Sync + 'static,
{
    #[cfg(feature = "graphql")]
    fn new(
        message: &'a Message,
        session: &'a Session,
        workspace: &'a CausalWorkspace<'a, A>,
    ) -> Self {
        Self {
            message,
            session,
            workspace,
        }
    }

    pub fn command_name(&self) -> &str {
        self.message.name()
    }

    pub fn message_id(&self) -> Option<&str> {
        self.message.id()
    }

    pub fn correlation_id(&self) -> Option<&str> {
        self.message.correlation_id()
    }

    pub fn causation_id(&self) -> Option<&str> {
        self.message.causation_id()
    }

    pub fn trace_context(&self) -> crate::TraceContext {
        self.message.trace_context()
    }

    pub fn user_id(&self) -> Result<&str, HandlerError> {
        self.session
            .user_id()
            .ok_or_else(|| HandlerError::Unauthorized("missing user ID in session".into()))
    }

    pub fn role(&self) -> Option<&str> {
        self.session.role()
    }

    pub fn claim(&self, name: &str) -> Option<&str> {
        self.session.get(name)
    }

    /// Load one aggregate as an owned checkout without retaining a queue lock.
    pub async fn load(
        &self,
        id: &str,
    ) -> Result<Option<super::AggregateCheckout<A>>, HandlerError> {
        self.workspace
            .load(id)
            .await
            .map_err(workspace_handler_error)
    }

    /// Start a new aggregate checkout. The handler must assign a valid entity
    /// identity before staging it.
    pub fn create(&self) -> super::AggregateCheckout<A> {
        self.workspace.create()
    }

    /// Stage a checkout for the framework-owned atomic commit.
    pub fn stage(&self, checkout: super::AggregateCheckout<A>) -> Result<(), HandlerError> {
        self.workspace
            .stage(checkout)
            .map_err(workspace_handler_error)
    }

    /// Stage one durable outbox fact in the command transaction.
    pub fn stage_outbox(&self, message: OutboxMessage) -> Result<(), HandlerError> {
        self.workspace
            .stage_outbox(message)
            .map_err(workspace_handler_error)
    }

    /// Stage a validated relational read-model write plan.
    pub fn stage_read_models(&self, writes: ReadModelWritePlanBuilder) -> Result<(), HandlerError> {
        self.workspace
            .stage_read_models(writes)
            .map_err(workspace_handler_error)
    }

    /// Stage the exact returned model as a full-row upsert and prepare a sealed
    /// same-transaction projection result.
    pub fn projected<M>(&self, model: M) -> Result<PreparedCommand<Projected<M>>, HandlerError>
    where
        M: GraphqlOutputType + RelationalReadModel + serde::Serialize + Send + Sync + 'static,
    {
        self.workspace
            .prepare_projected(model)
            .map_err(workspace_handler_error)
    }
}

fn workspace_handler_error(error: CausalWorkspaceError) -> HandlerError {
    HandlerError::Other(Box::new(error))
}

/// A typed causal command handler. The framework binds the decoded input to
/// the same `I` used by the GraphQL declaration, and the handler may only
/// prepare a sealed consistency outcome for the durable committer.
/// Captured external side effects are unsupported because handler invocation
/// itself can repeat after lease expiry; see [`CausalCommandContext`].
pub trait PreparedCommandHandler<'a, A, I, K>: Send + Sync
where
    A: Aggregate + Send + Sync + 'static,
    K: CommandOutcome,
{
    type Future: Future<Output = Result<PreparedCommand<K>, HandlerError>> + Send + 'a;
    fn call(&self, ctx: &'a CausalCommandContext<'a, A>, input: I) -> Self::Future;
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

impl<'a, A, I, K, F, Fut> PreparedCommandHandler<'a, A, I, K> for F
where
    A: Aggregate + Send + Sync + 'static,
    I: 'a,
    K: CommandOutcome,
    F: Fn(&'a CausalCommandContext<'a, A>, I) -> Fut + Send + Sync,
    Fut: Future<Output = Result<PreparedCommand<K>, HandlerError>> + Send + 'a,
{
    type Future = Fut;

    fn call(&self, ctx: &'a CausalCommandContext<'a, A>, input: I) -> Self::Future {
        self(ctx, input)
    }
}

fn boxed_handler<D, F>(handler: F) -> Arc<HandlerFn<D>>
where
    F: for<'a> Handler<'a, D> + 'static,
{
    Arc::new(move |ctx| Box::pin(handler.call(ctx)) as HandlerFuture<'_>)
}

/// Stable transport classification for a typed causal command dispatch.
///
/// Public receipt/status envelopes map this private error set onto a stable
/// mutation edge without exposing repository details.
#[derive(Debug)]
#[cfg(feature = "graphql")]
pub(crate) enum CausalDispatchError {
    BadRequest(String),
    Forbidden,
    CommandIdReuse,
    InProgress,
    Expired,
    Rejected {
        code: &'static str,
        status: u16,
        message: String,
    },
    Handler(HandlerError),
    Internal(String),
}

#[cfg(feature = "graphql")]
impl CausalDispatchError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::Forbidden => "FORBIDDEN",
            Self::CommandIdReuse => "COMMAND_ID_REUSE",
            Self::InProgress => "COMMAND_IN_PROGRESS",
            Self::Expired => "COMMAND_EXPIRED",
            Self::Rejected { code, .. } => code,
            Self::Handler(error) => match error.status_code() {
                400 => "BAD_REQUEST",
                401 => "UNAUTHORIZED",
                403 => "FORBIDDEN",
                404 => "NOT_FOUND",
                422 => "REJECTED",
                _ => "INTERNAL",
            },
            Self::Internal(_) => "INTERNAL",
        }
    }

    pub(crate) fn status_code(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Forbidden => 403,
            Self::CommandIdReuse | Self::InProgress => 409,
            Self::Expired => 410,
            Self::Rejected { status, .. } => *status,
            Self::Handler(error) => error.status_code(),
            Self::Internal(_) => 500,
        }
    }

    pub(crate) fn client_message(&self) -> String {
        match self {
            Self::BadRequest(message) => message.clone(),
            Self::Rejected { message, .. } => message.clone(),
            Self::Forbidden => "command is not allowed".into(),
            Self::CommandIdReuse => "command ID was already used for different input".into(),
            Self::InProgress => "command is already in progress".into(),
            Self::Expired => "command ID has expired".into(),
            Self::Handler(error) => error.client_facing_message(),
            Self::Internal(_) => "internal error".into(),
        }
    }
}

#[cfg(feature = "graphql")]
impl std::fmt::Display for CausalDispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Internal(detail) => formatter.write_str(detail),
            _ => formatter.write_str(&self.client_message()),
        }
    }
}

#[cfg(feature = "graphql")]
impl std::error::Error for CausalDispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Handler(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(feature = "graphql")]
impl From<HandlerError> for CausalDispatchError {
    fn from(error: HandlerError) -> Self {
        Self::Handler(error)
    }
}

/// Error returned when attaching a GraphQL engine whose typed command
/// inventory is not exactly the executable service inventory, or whose query
/// storage cannot prove the identity required by a `Projected` command.
#[cfg(feature = "graphql")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphqlServiceBindError(pub String);

#[cfg(feature = "graphql")]
impl std::fmt::Display for GraphqlServiceBindError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(feature = "graphql")]
impl std::error::Error for GraphqlServiceBindError {}

type PreparedHandlerFuture<'a, K> =
    Pin<Box<dyn Future<Output = Result<PreparedCommand<K>, HandlerError>> + Send + 'a>>;
type PreparedHandlerFn<A, I, K> = dyn for<'a> Fn(&'a CausalCommandContext<'a, A>, I) -> PreparedHandlerFuture<'a, K>
    + Send
    + Sync;
type CausalGuardFn<A> = dyn for<'a> Fn(&CausalCommandContext<'a, A>) -> bool + Send + Sync;

fn boxed_prepared_handler<A, I, K, F>(handler: F) -> Arc<PreparedHandlerFn<A, I, K>>
where
    A: Aggregate + Send + Sync + 'static,
    I: serde::de::DeserializeOwned + Send + 'static,
    K: CommandOutcome,
    F: for<'a> PreparedCommandHandler<'a, A, I, K> + 'static,
{
    Arc::new(move |context, input| {
        Box::pin(handler.call(context, input)) as PreparedHandlerFuture<'_, K>
    })
}

fn boxed_causal_guard<A, G>(guard: G) -> Arc<CausalGuardFn<A>>
where
    A: Aggregate + Send + Sync + 'static,
    G: for<'a> Fn(&CausalCommandContext<'a, A>) -> bool + Send + Sync + 'static,
{
    Arc::new(guard)
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

enum RegisteredHandler<D> {
    Legacy {
        guard: Option<Arc<GuardFn<D>>>,
        handle: Arc<HandlerFn<D>>,
    },
    Causal(Box<dyn ErasedCausalHandler<D>>),
}

#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "graphql"), allow(dead_code))]
struct CausalCommandPolicy {
    attempt_lease: Duration,
    replay_retention: Duration,
}

impl Default for CausalCommandPolicy {
    fn default() -> Self {
        Self {
            attempt_lease: Duration::from_secs(30),
            replay_retention: Duration::from_secs(30 * 24 * 60 * 60),
        }
    }
}

#[cfg(feature = "graphql")]
type CausalHandlerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, CausalDispatchError>> + Send + 'a>>;

trait ErasedCausalHandler<D>: Send + Sync {
    fn contract(&self) -> &TypedCommandContract;

    #[cfg(feature = "graphql")]
    fn storage_identity(&self, dependencies: &D) -> crate::command_ledger::CausalStorageIdentity;

    #[cfg(feature = "graphql")]
    #[allow(clippy::too_many_arguments)]
    fn dispatch<'a>(
        &'a self,
        dependencies: &'a D,
        service_id: &'a str,
        command_id: &'a str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        policy: CausalCommandPolicy,
    ) -> CausalHandlerFuture<'a>;

    #[cfg(feature = "graphql")]
    #[allow(dead_code)]
    fn lookup<'a>(
        &'a self,
        dependencies: &'a D,
        service_id: &'a str,
        command_id: &'a str,
        session: &'a Session,
        principal: VerifiedPrincipal,
    ) -> Pin<Box<dyn Future<Output = Result<CommandLookup, CausalDispatchError>> + Send + 'a>>;
}

struct RegisteredCausalHandler<A, I, K>
where
    A: Aggregate + Send + Sync + 'static,
    K: CommandOutcome,
{
    contract: TypedCommandContract,
    #[cfg_attr(not(feature = "graphql"), allow(dead_code))]
    guard: Option<Arc<CausalGuardFn<A>>>,
    #[cfg_attr(not(feature = "graphql"), allow(dead_code))]
    handle: Arc<PreparedHandlerFn<A, I, K>>,
    _types: std::marker::PhantomData<fn(A, I) -> K>,
}

impl<A, I, K> RegisteredCausalHandler<A, I, K>
where
    A: Aggregate + Send + Sync + 'static,
    K: CommandOutcome,
{
    fn new(
        contract: TypedCommandContract,
        guard: Option<Arc<CausalGuardFn<A>>>,
        handle: Arc<PreparedHandlerFn<A, I, K>>,
    ) -> Self {
        Self {
            contract,
            guard,
            handle,
            _types: std::marker::PhantomData,
        }
    }
}

type OutboxConfigurator<D> = fn(&mut D, DynBusPublisher, String, Duration, u32, Option<String>);

trait ErasedRoutes: Send + Sync {
    fn handler_specs(&self) -> &[HandlerSpec];

    fn typed_command_contracts(&self) -> Vec<&TypedCommandContract>;

    fn dispatch<'a>(
        &'a self,
        message: &'a Message,
        input: Value,
        session: Session,
    ) -> HandlerFuture<'a>;

    #[cfg(feature = "graphql")]
    #[allow(clippy::too_many_arguments)]
    fn dispatch_causal<'a>(
        &'a self,
        command: &'a str,
        service_id: &'a str,
        command_id: &'a str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        policy: CausalCommandPolicy,
    ) -> CausalHandlerFuture<'a>;

    #[cfg(feature = "graphql")]
    #[allow(dead_code)]
    fn lookup_causal<'a>(
        &'a self,
        command: &'a str,
        service_id: &'a str,
        command_id: &'a str,
        session: &'a Session,
        principal: VerifiedPrincipal,
    ) -> Pin<Box<dyn Future<Output = Result<CommandLookup, CausalDispatchError>> + Send + 'a>>;

    #[cfg(feature = "graphql")]
    fn projected_storage_identities(&self) -> Vec<crate::command_ledger::CausalStorageIdentity>;

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

/// Builder returned by [`Routes::typed_command`].
///
/// Unlike a legacy JSON route, the declaration and executable handler share
/// one route object, the same input and committed-outcome types, and the route
/// bundle's single aggregate repository.
pub struct TypedRouteBuilder<D, I, K: CommandOutcome> {
    routes: Routes<D>,
    route_name: &'static str,
    contract: TypedCommandContract,
    _types: std::marker::PhantomData<fn(I) -> K>,
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

impl<D, I, K> TypedRouteBuilder<D, I, K>
where
    D: CausalRouteDependencies + Send + Sync + 'static,
    D::Aggregate: Aggregate + Send + Sync + 'static,
    I: serde::de::DeserializeOwned + Send + 'static,
    K: CommandOutcome,
{
    /// Register a typed causal command handler without a guard.
    pub fn handle<F>(self, handler: F) -> Routes<D>
    where
        F: for<'a> PreparedCommandHandler<'a, D::Aggregate, I, K> + 'static,
    {
        self.routes.register_typed_handler(
            self.route_name,
            self.contract,
            None,
            boxed_prepared_handler(handler),
        )
    }

    /// Register a typed causal command handler with a synchronous guard.
    pub fn guarded<G, F>(self, guard: G, handler: F) -> Routes<D>
    where
        G: for<'a> Fn(&CausalCommandContext<'a, D::Aggregate>) -> bool + Send + Sync + 'static,
        F: for<'a> PreparedCommandHandler<'a, D::Aggregate, I, K> + 'static,
    {
        let guard = boxed_causal_guard(guard);
        self.routes.register_typed_handler(
            self.route_name,
            self.contract,
            Some(guard),
            boxed_prepared_handler(handler),
        )
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

    /// Register a typed command declaration and its executable handler as one
    /// inventory entry.
    pub fn typed_command<I, K>(self, command: TypedCommand<I, K>) -> TypedRouteBuilder<D, I, K>
    where
        I: serde::de::DeserializeOwned + Send + 'static,
        K: CommandOutcome,
    {
        let (route_name, contract) = command.into_parts();
        TypedRouteBuilder {
            routes: self,
            route_name,
            contract,
            _types: std::marker::PhantomData,
        }
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
                RegisteredHandler::Legacy {
                    guard: guard.clone(),
                    handle: handle.clone(),
                },
            );
        }
        self.handler_specs.push(spec);
        self
    }

    fn register_typed_handler<I, K>(
        mut self,
        route_name: &'static str,
        contract: TypedCommandContract,
        guard: Option<Arc<CausalGuardFn<D::Aggregate>>>,
        handle: Arc<PreparedHandlerFn<D::Aggregate, I, K>>,
    ) -> Self
    where
        D: CausalRouteDependencies,
        D::Aggregate: Aggregate + Send + Sync + 'static,
        I: serde::de::DeserializeOwned + Send + 'static,
        K: CommandOutcome,
    {
        assert_eq!(
            route_name, contract.name,
            "typed command route and contract ids must match"
        );
        let by_name = self.handlers.entry(MessageKind::Command).or_default();
        assert!(
            !by_name.contains_key(route_name),
            "duplicate route registration for {:?} `{}`",
            MessageKind::Command,
            route_name,
        );
        by_name.insert(
            route_name.to_string(),
            RegisteredHandler::Causal(Box::new(
                RegisteredCausalHandler::<D::Aggregate, I, K>::new(contract, guard, handle),
            )),
        );
        self.handler_specs.push(HandlerSpec::command(route_name));
        self
    }

    fn typed_contracts(&self) -> Vec<&TypedCommandContract> {
        self.handlers
            .values()
            .flat_map(HashMap::values)
            .filter_map(|handler| match handler {
                RegisteredHandler::Causal(handler) => Some(handler.contract()),
                RegisteredHandler::Legacy { .. } => None,
            })
            .collect()
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
            match handler {
                RegisteredHandler::Legacy { guard, handle } => (guard.clone(), handle.clone()),
                RegisteredHandler::Causal(_) => {
                    return Err(HandlerError::Unauthorized(
                        "typed causal commands require a verified GraphQL bearer envelope".into(),
                    ));
                }
            }
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

impl<D, A, I, K> ErasedCausalHandler<D> for RegisteredCausalHandler<A, I, K>
where
    D: CausalRouteDependencies<Aggregate = A> + Send + Sync + 'static,
    A: Aggregate + Send + Sync + 'static,
    I: serde::de::DeserializeOwned + Send + 'static,
    K: CommandOutcome,
{
    fn contract(&self) -> &TypedCommandContract {
        &self.contract
    }

    #[cfg(feature = "graphql")]
    fn storage_identity(&self, dependencies: &D) -> crate::command_ledger::CausalStorageIdentity {
        dependencies
            .__causal_aggregate_repository()
            .repo()
            .causal_storage_identity()
    }

    #[cfg(feature = "graphql")]
    fn dispatch<'a>(
        &'a self,
        dependencies: &'a D,
        service_id: &'a str,
        command_id: &'a str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        policy: CausalCommandPolicy,
    ) -> CausalHandlerFuture<'a> {
        Box::pin(async move {
            ensure_causal_grant(&self.contract, &session)?;

            let canonical = canonicalize_command_input(&self.contract.input, input)
                .map_err(|error| CausalDispatchError::BadRequest(error.to_string()))?;
            let typed = canonical
                .decode::<I>()
                .map_err(|error| CausalDispatchError::BadRequest(error.to_string()))?;
            let (input, wire, input_digest) = typed.into_parts();
            let projection_obligations = self
                .contract
                .resolve_projection_obligations(&wire)
                .map_err(|error| CausalDispatchError::Internal(error.to_string()))?;

            let command_id = CommandId::parse(command_id)
                .map_err(|error| CausalDispatchError::BadRequest(error.to_string()))?;
            let partition = PrincipalPartitionId::new(principal.partition_for_service(service_id))
                .map_err(internal_ledger_error)?;
            let key = CommandLedgerKey::new(service_id, partition, command_id)
                .map_err(internal_ledger_error)?;
            let reservation = CommandReservation::new(
                key,
                self.contract.name.clone(),
                CommandContractFingerprint::new(self.contract.fingerprint_bytes()),
                CanonicalInputHash::new(input_digest),
                policy.attempt_lease,
                policy.replay_retention,
            )
            .map_err(internal_ledger_error)?;

            let aggregate_repository = dependencies.__causal_aggregate_repository();
            let repository = aggregate_repository.repo();
            let attempt = match repository
                .reserve_command(reservation)
                .await
                .map_err(internal_ledger_error)?
            {
                ReservationOutcome::Acquired(attempt) => attempt,
                ReservationOutcome::InProgress { .. } => {
                    return Err(CausalDispatchError::InProgress)
                }
                ReservationOutcome::Replay(replay) => return replay_result(replay),
                ReservationOutcome::Conflict => return Err(CausalDispatchError::CommandIdReuse),
                ReservationOutcome::Expired => return Err(CausalDispatchError::Expired),
            };

            let payload = serde_json::to_vec(&wire).map_err(|error| {
                CausalDispatchError::Internal(format!(
                    "canonical command input could not be encoded: {error}"
                ))
            })?;
            let mut metadata = session
                .variables()
                .iter()
                .filter(|(name, _)| !name.eq_ignore_ascii_case(crate::trace_context::CAUSATION_ID))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<Vec<_>>();
            metadata.push((
                crate::trace_context::CAUSATION_ID.to_string(),
                attempt.causation_id().as_str().to_string(),
            ));
            let message = Message {
                id: Some(attempt.key().command_id().to_string()),
                name: self.contract.name.clone(),
                kind: MessageKind::Command,
                payload,
                content_type: "application/json".into(),
                metadata,
            };

            let workspace = CausalWorkspace::new(aggregate_repository);
            let context = CausalCommandContext::new(&message, &session, &workspace);
            if self.guard.as_ref().is_some_and(|guard| !guard(&context)) {
                return commit_causal_rejection(
                    repository,
                    attempt,
                    policy.replay_retention,
                    "REJECTED",
                    422,
                    format!("guard rejected command: {}", self.contract.name),
                )
                .await;
            }

            let prepared = match (self.handle)(&context, input).await {
                Ok(prepared) => prepared,
                Err(error) if error.status_code() < 500 => {
                    let code = causal_handler_error_code(&error);
                    let status = error.status_code();
                    let message = error.client_facing_message();
                    return commit_causal_rejection(
                        repository,
                        attempt,
                        policy.replay_retention,
                        code,
                        status,
                        message,
                    )
                    .await;
                }
                Err(error) => {
                    return abandon_causal_attempt(repository, attempt, error.to_string()).await;
                }
            };

            let mut parts = match workspace.into_parts() {
                Ok(parts) => parts,
                Err(error) => {
                    return abandon_causal_attempt(repository, attempt, error.to_string()).await
                }
            };
            if let Err(error) = parts.validate_prepared(&self.contract, &prepared) {
                return abandon_causal_attempt(repository, attempt, error.to_string()).await;
            }

            let terminal_state = match self.contract.consistency {
                CommandConsistency::Accepted if self.contract.confirmations.is_empty() => {
                    TerminalCommandState::Accepted
                }
                CommandConsistency::Accepted | CommandConsistency::Fact => {
                    TerminalCommandState::AcceptedPendingProjection
                }
                CommandConsistency::Projected => TerminalCommandState::Projected,
            };
            let replay_payload = prepared.serialized_payload().clone();
            let publisher = aggregate_repository.outbox_publisher();
            let mut batch = match parts.prepare_commit_batch() {
                Ok(batch) => batch,
                Err(error) => {
                    return abandon_causal_attempt(
                        repository,
                        attempt,
                        format!("causal commit batch preparation failed: {error}"),
                    )
                    .await
                }
            };

            // Match the ordinary aggregate commit path: when Service::with_bus
            // installed an immediate publisher, make each fresh outbox row
            // InFlight inside the same fenced transaction and publish it only
            // after that transaction succeeds. A crash or publish failure leaves
            // the durable lease for a separately operated polling worker to
            // recover.
            let mut claimed = Vec::new();
            if let Some(config) = publisher {
                let now = SystemTime::now();
                let mut claim_error = None;
                for message in &mut batch.outbox_messages {
                    // The post-commit hook receives clones of this staged
                    // batch. Stamp before cloning so the broker copy and the
                    // persisted row carry the same authoritative causation.
                    message.overwrite_causation_id(attempt.causation_id().as_str());
                    if let Err(error) = message.claim_at(&config.worker_id, config.lease, now) {
                        claim_error = Some(error.to_string());
                        break;
                    }
                    claimed.push(message.clone());
                }
                if let Some(error) = claim_error {
                    drop(batch);
                    return abandon_causal_attempt(
                        repository,
                        attempt,
                        format!("causal outbox claim failed before commit: {error}"),
                    )
                    .await;
                }
            }

            let fence = attempt.fence();
            let completion = attempt
                .complete_with_obligations(
                    terminal_state,
                    replay_payload.clone(),
                    projection_obligations,
                    policy.replay_retention,
                )
                .map_err(internal_ledger_error)?;
            let causal_batch = CausalCommitBatch::new(batch, completion);
            match repository.commit_causal_batch(causal_batch).await {
                Ok(()) => {
                    parts.mark_snapshot_versions_committed();
                    if let Some(config) = publisher {
                        let _ = config.hook.publish_claimed(claimed).await;
                    }
                    let (_committed, serialized) = prepared.finalize_after_commit();
                    Ok(serialized)
                }
                Err(error) => {
                    recover_causal_commit_error(repository, fence, error.to_string()).await
                }
            }
        })
    }

    #[cfg(feature = "graphql")]
    fn lookup<'a>(
        &'a self,
        dependencies: &'a D,
        service_id: &'a str,
        command_id: &'a str,
        session: &'a Session,
        principal: VerifiedPrincipal,
    ) -> Pin<Box<dyn Future<Output = Result<CommandLookup, CausalDispatchError>> + Send + 'a>> {
        Box::pin(async move {
            ensure_causal_grant(&self.contract, session)?;
            let command_id = CommandId::parse(command_id)
                .map_err(|error| CausalDispatchError::BadRequest(error.to_string()))?;
            let partition = PrincipalPartitionId::new(principal.partition_for_service(service_id))
                .map_err(internal_ledger_error)?;
            let key = CommandLedgerKey::new(service_id, partition, command_id)
                .map_err(internal_ledger_error)?;
            dependencies
                .__causal_aggregate_repository()
                .repo()
                .lookup_command(&key, CommandLookupScope::CommandName(&self.contract.name))
                .await
                .map_err(internal_ledger_error)
        })
    }
}

#[cfg(feature = "graphql")]
fn ensure_causal_grant(
    contract: &TypedCommandContract,
    session: &Session,
) -> Result<(), CausalDispatchError> {
    if contract.roles.is_empty()
        || session
            .role()
            .is_some_and(|role| contract.roles.iter().any(|allowed| allowed == role))
    {
        Ok(())
    } else {
        Err(CausalDispatchError::Forbidden)
    }
}

#[cfg(feature = "graphql")]
fn causal_handler_error_code(error: &HandlerError) -> &'static str {
    match error.status_code() {
        400 => "BAD_REQUEST",
        401 => "UNAUTHORIZED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        422 => "REJECTED",
        _ => "REJECTED",
    }
}

#[cfg(feature = "graphql")]
fn internal_ledger_error(error: CommandLedgerError) -> CausalDispatchError {
    CausalDispatchError::Internal(error.to_string())
}

#[cfg(feature = "graphql")]
fn replay_result(replay: CommandReplay) -> Result<Value, CausalDispatchError> {
    match replay.state {
        CommandLedgerState::Accepted
        | CommandLedgerState::AcceptedPendingProjection
        | CommandLedgerState::Projected => Ok(replay.outcome),
        CommandLedgerState::Rejected => replay_rejection(replay.outcome),
        CommandLedgerState::ProjectionFailed => Err(CausalDispatchError::Internal(
            "stored command projection failed".into(),
        )),
        CommandLedgerState::InProgress
        | CommandLedgerState::RetryableUnknown
        | CommandLedgerState::Expired => Err(CausalDispatchError::Internal(
            "stored replay has a non-terminal state".into(),
        )),
    }
}

#[cfg(feature = "graphql")]
fn replay_rejection(outcome: Value) -> Result<Value, CausalDispatchError> {
    let error = outcome
        .get("error")
        .and_then(Value::as_object)
        .ok_or_else(|| CausalDispatchError::Internal("stored rejection is malformed".into()))?;
    let code = match error.get("code").and_then(Value::as_str) {
        Some("BAD_REQUEST") => "BAD_REQUEST",
        Some("UNAUTHORIZED") => "UNAUTHORIZED",
        Some("FORBIDDEN") => "FORBIDDEN",
        Some("NOT_FOUND") => "NOT_FOUND",
        Some("REJECTED") => "REJECTED",
        _ => {
            return Err(CausalDispatchError::Internal(
                "stored rejection code is invalid".into(),
            ))
        }
    };
    let status = error
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
        .filter(|status| (400..500).contains(status))
        .ok_or_else(|| {
            CausalDispatchError::Internal("stored rejection status is invalid".into())
        })?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| CausalDispatchError::Internal("stored rejection message is invalid".into()))?
        .to_string();
    Err(CausalDispatchError::Rejected {
        code,
        status,
        message,
    })
}

#[cfg(feature = "graphql")]
async fn commit_causal_rejection<R>(
    repository: &R,
    attempt: CommandAttempt,
    retention: Duration,
    code: &'static str,
    status: u16,
    message: String,
) -> Result<Value, CausalDispatchError>
where
    R: CommandLedgerStore + CausalTransactionalCommit + Send + Sync,
{
    let outcome = serde_json::json!({
        "error": {
            "code": code,
            "status": status,
            "message": message,
        }
    });
    let fence = attempt.fence();
    let completion = attempt
        .complete(TerminalCommandState::Rejected, outcome, retention)
        .map_err(internal_ledger_error)?;
    match repository
        .commit_causal_batch(CausalCommitBatch::new(CommitBatch::empty(), completion))
        .await
    {
        Ok(()) => Err(CausalDispatchError::Rejected {
            code,
            status,
            message,
        }),
        Err(error) => recover_causal_commit_error(repository, fence, error.to_string()).await,
    }
}

#[cfg(feature = "graphql")]
async fn abandon_causal_attempt<R>(
    repository: &R,
    attempt: CommandAttempt,
    detail: String,
) -> Result<Value, CausalDispatchError>
where
    R: CommandLedgerStore + Send + Sync,
{
    let fence = attempt.fence();
    match repository.mark_retryable_unknown(fence.clone()).await {
        Ok(()) => Err(CausalDispatchError::Internal(detail)),
        Err(CommandLedgerError::AttemptFenced { .. }) => {
            resolve_ambiguous_lookup(repository, fence, detail).await
        }
        Err(error) => Err(CausalDispatchError::Internal(format!(
            "{detail}; failed to mark command retryable: {error}"
        ))),
    }
}

#[cfg(feature = "graphql")]
async fn recover_causal_commit_error<R>(
    repository: &R,
    fence: AttemptFence,
    detail: String,
) -> Result<Value, CausalDispatchError>
where
    R: CommandLedgerStore + Send + Sync,
{
    resolve_ambiguous_lookup(repository, fence, detail).await
}

#[cfg(feature = "graphql")]
async fn resolve_ambiguous_lookup<R>(
    repository: &R,
    fence: AttemptFence,
    detail: String,
) -> Result<Value, CausalDispatchError>
where
    R: CommandLedgerStore + Send + Sync,
{
    match repository
        .lookup_command(fence.key(), CommandLookupScope::Attempt(&fence))
        .await
    {
        Ok(CommandLookup::Replay(replay)) => replay_result(replay),
        Ok(CommandLookup::Expired) => Err(CausalDispatchError::Expired),
        Ok(CommandLookup::RetryableUnknown { .. }) => Err(CausalDispatchError::Internal(detail)),
        Ok(CommandLookup::InProgress { .. }) => {
            match repository.mark_retryable_unknown(fence).await {
                Ok(()) => Err(CausalDispatchError::Internal(detail)),
                Err(CommandLedgerError::AttemptFenced { .. }) => {
                    Err(CausalDispatchError::InProgress)
                }
                Err(error) => Err(CausalDispatchError::Internal(format!(
                    "{detail}; command recovery failed: {error}"
                ))),
            }
        }
        Ok(CommandLookup::Unknown) => Err(CausalDispatchError::Internal(format!(
            "{detail}; command ledger row disappeared"
        ))),
        Err(error) => Err(CausalDispatchError::Internal(format!(
            "{detail}; command outcome lookup failed: {error}"
        ))),
    }
}

impl<D> ErasedRoutes for Routes<D>
where
    D: Send + Sync + 'static,
{
    fn handler_specs(&self) -> &[HandlerSpec] {
        &self.handler_specs
    }

    fn typed_command_contracts(&self) -> Vec<&TypedCommandContract> {
        self.typed_contracts()
    }

    fn dispatch<'a>(
        &'a self,
        message: &'a Message,
        input: Value,
        session: Session,
    ) -> HandlerFuture<'a> {
        Box::pin(self.invoke(message, input, session))
    }

    #[cfg(feature = "graphql")]
    fn dispatch_causal<'a>(
        &'a self,
        command: &'a str,
        service_id: &'a str,
        command_id: &'a str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        policy: CausalCommandPolicy,
    ) -> CausalHandlerFuture<'a> {
        let handler = self
            .handlers
            .get(&MessageKind::Command)
            .and_then(|handlers| handlers.get(command));
        match handler {
            Some(RegisteredHandler::Causal(handler)) => handler.dispatch(
                &self.dependencies,
                service_id,
                command_id,
                input,
                session,
                principal,
                policy,
            ),
            Some(RegisteredHandler::Legacy { .. }) | None => Box::pin(async move {
                Err(CausalDispatchError::BadRequest(format!(
                    "`{command}` is not a typed causal command"
                )))
            }),
        }
    }

    #[cfg(feature = "graphql")]
    fn lookup_causal<'a>(
        &'a self,
        command: &'a str,
        service_id: &'a str,
        command_id: &'a str,
        session: &'a Session,
        principal: VerifiedPrincipal,
    ) -> Pin<Box<dyn Future<Output = Result<CommandLookup, CausalDispatchError>> + Send + 'a>> {
        let handler = self
            .handlers
            .get(&MessageKind::Command)
            .and_then(|handlers| handlers.get(command));
        match handler {
            Some(RegisteredHandler::Causal(handler)) => handler.lookup(
                &self.dependencies,
                service_id,
                command_id,
                session,
                principal,
            ),
            Some(RegisteredHandler::Legacy { .. }) | None => Box::pin(async move {
                Err(CausalDispatchError::BadRequest(format!(
                    "`{command}` is not a typed causal command"
                )))
            }),
        }
    }

    #[cfg(feature = "graphql")]
    fn projected_storage_identities(&self) -> Vec<crate::command_ledger::CausalStorageIdentity> {
        self.handlers
            .values()
            .flat_map(|handlers| handlers.values())
            .filter_map(|handler| match handler {
                RegisteredHandler::Causal(handler)
                    if handler.contract().consistency == CommandConsistency::Projected =>
                {
                    Some(handler.storage_identity(&self.dependencies))
                }
                RegisteredHandler::Causal(_) | RegisteredHandler::Legacy { .. } => None,
            })
            .collect()
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
    causal_command_policy: CausalCommandPolicy,
    runner: Option<ServiceRunner>,
    /// When false, HTTP does not mount `POST /{command}` (GraphQL / health only).
    /// Commands remain dispatchable via GraphQL mutations and in-process `dispatch`.
    http_command_routes: bool,
    #[cfg(feature = "graphql")]
    graphql: Option<std::sync::Arc<crate::graphql::GraphqlEngine>>,
}

impl Service {
    /// Start building a deployment-level service.
    pub fn new() -> Self {
        Self {
            name: None,
            routes: Vec::new(),
            index: HashMap::new(),
            handler_specs: Vec::new(),
            causal_command_policy: CausalCommandPolicy::default(),
            runner: None,
            http_command_routes: true,
            #[cfg(feature = "graphql")]
            graphql: None,
        }
    }

    /// Disable `POST /{command}` HTTP routes.
    ///
    /// Use when the public surface is GraphQL-only (command mutations + queries).
    /// Handlers stay registered for GraphQL dispatch and bus consumers.
    pub fn without_http_command_routes(mut self) -> Self {
        self.http_command_routes = false;
        self
    }

    /// Whether the HTTP router mounts per-command `POST /{name}` routes.
    pub fn http_command_routes_enabled(&self) -> bool {
        self.http_command_routes
    }

    /// Configure the durable command attempt lease and replay retention.
    ///
    /// The defaults are 30 seconds and 30 days. Retention must remain longer
    /// than the attempt lease; deployments must also keep it beyond the retry
    /// and resume window advertised to their generated clients.
    pub fn causal_command_timing(
        mut self,
        attempt_lease: Duration,
        replay_retention: Duration,
    ) -> Self {
        assert!(
            !attempt_lease.is_zero(),
            "causal command attempt lease must be positive"
        );
        assert!(
            replay_retention > attempt_lease,
            "causal command replay retention must exceed the attempt lease"
        );
        self.causal_command_policy = CausalCommandPolicy {
            attempt_lease,
            replay_retention,
        };
        self
    }

    /// Attach a GraphQL query engine served at `POST /graphql`.
    ///
    /// Panics when [`Self::try_with_graphql`] rejects the attachment. New code
    /// that registers typed commands should prefer the fallible form.
    #[cfg(feature = "graphql")]
    pub fn with_graphql(self, engine: crate::graphql::GraphqlEngine) -> Self {
        self.try_with_graphql(engine)
            .unwrap_or_else(|error| panic!("cannot enable GraphQL: {error}"))
    }

    /// Validate and attach a GraphQL engine.
    ///
    /// Typed commands are compared by service ID, a canonical structural
    /// fingerprint, and exact Rust input/output `TypeId`s. A validated engine
    /// may attach and serve reads and durable typed mutations. `Projected`
    /// commands additionally require the engine and command repository to
    /// carry the same opaque causal-storage identity.
    #[cfg(feature = "graphql")]
    pub fn try_with_graphql(
        mut self,
        engine: crate::graphql::GraphqlEngine,
    ) -> Result<Self, GraphqlServiceBindError> {
        self.validate_graphql_engine(&engine)?;
        self.graphql = Some(std::sync::Arc::new(engine));
        Ok(self)
    }

    #[cfg(feature = "graphql")]
    pub(crate) fn validate_graphql_engine(
        &self,
        engine: &crate::graphql::GraphqlEngine,
    ) -> Result<(), GraphqlServiceBindError> {
        let service_id = self.name().ok_or_else(|| {
            GraphqlServiceBindError(
                "GraphQL attachment requires a stable Service::named identity".into(),
            )
        })?;
        let engine_service_id = engine.service_id().ok_or_else(|| {
            GraphqlServiceBindError(
                "GraphQL attachment requires an engine with a validated service ID".into(),
            )
        })?;
        if service_id != engine_service_id {
            return Err(GraphqlServiceBindError(format!(
                "service ID mismatch: executable service `{service_id}` vs GraphQL engine `{engine_service_id}`"
            )));
        }
        if self.handles_message(crate::bus::MessageKind::Command, "graphql") {
            return Err(GraphqlServiceBindError(
                "a command named `graphql` is already registered".into(),
            ));
        }

        let typed_commands = self.typed_command_contracts();
        match (typed_commands.is_empty(), engine.typed_command_binding()) {
            (true, None) => {}
            (_, Some(engine_binding)) => {
                let service_binding = self
                    .typed_command_binding()
                    .map_err(GraphqlServiceBindError)?;
                if service_binding.service_id != engine_binding.service_id {
                    return Err(GraphqlServiceBindError(format!(
                        "service ID mismatch: executable service `{}` vs GraphQL engine `{}`",
                        service_binding.service_id, engine_binding.service_id
                    )));
                }
                if service_binding.structural_fingerprint != engine_binding.structural_fingerprint {
                    return Err(GraphqlServiceBindError(format!(
                        "typed command structural fingerprint mismatch: executable `{}` vs GraphQL `{}`",
                        service_binding.structural_fingerprint,
                        engine_binding.structural_fingerprint
                    )));
                }
                if service_binding.types != engine_binding.types {
                    return Err(GraphqlServiceBindError(
                        "typed command Rust input/output TypeId mismatch".into(),
                    ));
                }
            }
            (false, None) => {
                return Err(GraphqlServiceBindError(
                    "GraphQL engine was not derived from this service's typed command inventory"
                        .into(),
                ));
            }
        }

        let projected_identities = self
            .routes
            .iter()
            .flat_map(|routes| routes.projected_storage_identities())
            .collect::<Vec<_>>();
        if !projected_identities.is_empty() {
            let engine_identity = engine.causal_storage_identity().ok_or_else(|| {
                GraphqlServiceBindError(
                    "Projected commands require a GraphQL pool derived from the same repository handle"
                        .into(),
                )
            })?;
            if projected_identities
                .iter()
                .any(|identity| *identity != engine_identity)
            {
                return Err(GraphqlServiceBindError(
                    "Projected command repository and GraphQL query pool storage identities differ"
                        .into(),
                ));
            }
        }

        Ok(())
    }

    /// The attached GraphQL engine, if any.
    #[cfg(feature = "graphql")]
    pub fn graphql_engine(&self) -> Option<std::sync::Arc<crate::graphql::GraphqlEngine>> {
        self.graphql.clone()
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
        let name = name.into();
        assert!(!name.trim().is_empty(), "service name must not be empty");
        if let Some(existing) = self.name.as_deref() {
            assert_eq!(
                existing, name,
                "service identity was already configured and cannot be changed"
            );
        }
        #[cfg(feature = "graphql")]
        if let Some(engine) = &self.graphql {
            assert_eq!(
                engine.service_id(),
                Some(name.as_str()),
                "attached GraphQL engine identity does not match renamed service"
            );
        }
        self.name = Some(name);
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
        let typed_commands = routes
            .typed_contracts()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        #[cfg(feature = "graphql")]
        assert!(
            self.graphql.is_none() || typed_commands.is_empty(),
            "cannot add typed command routes after attaching a GraphQL engine"
        );
        for (kind, name) in &keys {
            assert!(
                !self.handles_message(*kind, name),
                "duplicate route registration for {:?} `{}`",
                kind,
                name
            );
            #[cfg(feature = "graphql")]
            assert!(
                !(self.graphql.is_some()
                    && *kind == crate::bus::MessageKind::Command
                    && name == "graphql"),
                "cannot register command `graphql` while GraphQL is enabled on this service"
            );
        }
        let existing_commands = self.typed_command_contracts();
        for contract in &typed_commands {
            assert!(
                !existing_commands
                    .iter()
                    .any(|registered| registered.name == contract.name),
                "duplicate typed command declaration for `{}`",
                contract.name
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

    pub(crate) fn typed_command_contracts(&self) -> Vec<TypedCommandContract> {
        self.routes
            .iter()
            .flat_map(|routes| routes.typed_command_contracts())
            .cloned()
            .collect()
    }

    pub(crate) fn typed_command_binding(&self) -> Result<TypedServiceCommandBinding, String> {
        let service_id = self
            .name()
            .ok_or_else(|| "typed command inventory requires Service::named".to_string())?;
        TypedServiceCommandBinding::from_contracts(service_id, &self.typed_command_contracts())
    }

    /// Execute one authenticated typed causal route through its durable ledger
    /// and framework-owned staged commit boundary.
    #[cfg(feature = "graphql")]
    pub(crate) async fn dispatch_causal(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
    ) -> Result<Value, CausalDispatchError> {
        let service_id = self.name().ok_or_else(|| {
            CausalDispatchError::Internal(
                "typed causal dispatch requires Service::named identity".into(),
            )
        })?;
        let route_index = self
            .index
            .get(&MessageKind::Command)
            .and_then(|commands| commands.get(command))
            .copied()
            .ok_or_else(|| CausalDispatchError::BadRequest("unknown typed command".into()))?;
        self.routes[route_index]
            .dispatch_causal(
                command,
                service_id,
                command_id,
                input,
                session,
                principal,
                self.causal_command_policy,
            )
            .await
    }

    /// Private lookup seam used by replay recovery and the authorized status
    /// envelope. The route rechecks the current role grant before deriving the
    /// bearer-scoped ledger key.
    #[cfg(feature = "graphql")]
    #[allow(dead_code)]
    pub(crate) async fn lookup_causal_command(
        &self,
        command: &str,
        command_id: &str,
        session: &Session,
        principal: VerifiedPrincipal,
    ) -> Result<CommandLookup, CausalDispatchError> {
        let service_id = self.name().ok_or_else(|| {
            CausalDispatchError::Internal(
                "typed causal lookup requires Service::named identity".into(),
            )
        })?;
        let route_index = self
            .index
            .get(&MessageKind::Command)
            .and_then(|commands| commands.get(command))
            .copied()
            .ok_or_else(|| CausalDispatchError::BadRequest("unknown typed command".into()))?;
        self.routes[route_index]
            .lookup_causal(command, service_id, command_id, session, principal)
            .await
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

        self.invoke_with_dispatch_span(&message, input, session)
            .await
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
            return Err(HandlerError::UnknownCommand(message.name.clone()));
        }

        let input = match message_to_json_input(message) {
            Ok(input) => input,
            // Binary payloads (bitcode, octet-stream) legitimately fail JSON
            // parsing: handlers for those read `ctx.message().payload` directly,
            // so a `Null` input is the intended fallback. A payload that
            // *claims* to be JSON but does not parse is a decode error — surface
            // it instead of silently nulling the input.
            Err(_) if !is_json_content_type(&message.content_type) => Value::Null,
            Err(err) => return Err(err),
        };
        let session = message_to_session(message);
        self.invoke_with_dispatch_span(message, input, session)
            .await
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
    #[cfg(feature = "graphql")]
    use crate::command_ledger::CausalGetStream;
    #[cfg(feature = "graphql")]
    use crate::graphql::SurfaceProjector;
    use crate::graphql::{
        typed_command, Accepted, GraphqlInputType, GraphqlOutputType, GraphqlTypeDef,
        GraphqlTypeField,
    };
    use crate::{
        sourced, AggregateBuilder, AggregateRepository, Entity, InMemoryRepository, Queueable,
        QueuedRepository,
    };
    #[cfg(feature = "graphql")]
    use crate::{GetStream, OutboxStore};
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    #[cfg(feature = "graphql")]
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[cfg(feature = "graphql")]
    use std::sync::{Arc, Mutex};

    #[derive(Deserialize)]
    struct TypedInput {
        id: String,
    }

    #[derive(Serialize)]
    struct TypedOutput {
        id: String,
    }

    fn one_string_field(name: &str, field: &str) -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            name,
            vec![GraphqlTypeField {
                name: field.into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        )
    }

    impl GraphqlInputType for TypedInput {
        fn graphql_type() -> GraphqlTypeDef {
            one_string_field("TypedInput", "id").with_type_id(std::any::TypeId::of::<Self>())
        }
    }

    impl GraphqlOutputType for TypedOutput {
        fn graphql_type() -> GraphqlTypeDef {
            one_string_field("TypedOutput", "id").with_type_id(std::any::TypeId::of::<Self>())
        }
    }

    #[cfg(feature = "graphql")]
    #[derive(Deserialize)]
    struct CausalTestInput {
        id: String,
        label: String,
    }

    #[cfg(feature = "graphql")]
    impl GraphqlInputType for CausalTestInput {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "CausalTestInput",
                vec![
                    GraphqlTypeField {
                        name: "id".into(),
                        type_name: "String".into(),
                        nullable: false,
                        list: false,
                        item_nullable: false,
                        nested: None,
                    },
                    GraphqlTypeField {
                        name: "label".into(),
                        type_name: "String".into(),
                        nullable: false,
                        list: false,
                        item_nullable: false,
                        nested: None,
                    },
                ],
            )
            .with_type_id(std::any::TypeId::of::<Self>())
        }
    }

    #[cfg(feature = "graphql")]
    #[derive(Clone, Deserialize, crate::GraphqlInput)]
    struct CausalProjectionInput {
        #[serde(rename = "todoId")]
        id: String,
        #[serde(rename = "tenantPartition")]
        partition: String,
    }

    #[cfg(feature = "graphql")]
    #[derive(Clone, Serialize, Deserialize, crate::ReadModel)]
    #[readmodel(
        table = "causal_projection_obligation_views",
        primary_key = ["id"]
    )]
    struct CausalProjectionObligationView {
        id: String,
    }

    static TYPED_HANDLER_INVOKED: AtomicBool = AtomicBool::new(false);
    static TYPED_GUARD_INVOKED: AtomicBool = AtomicBool::new(false);

    async fn typed_handler(
        _context: &CausalCommandContext<'_, RouteComboAggregate>,
        input: TypedInput,
    ) -> Result<PreparedCommand<Accepted<TypedOutput>>, HandlerError> {
        TYPED_HANDLER_INVOKED.store(true, Ordering::SeqCst);
        Ok(PreparedCommand::prepare(TypedOutput { id: input.id }).unwrap())
    }

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

    #[cfg(feature = "graphql")]
    #[derive(Default)]
    struct CausalDispatcherAggregate {
        entity: Entity,
    }

    #[cfg(feature = "graphql")]
    impl CausalDispatcherAggregate {
        fn record(&mut self, id: String) -> crate::SourcedResult {
            self.entity.set_id(id);
            self.entity.digest_empty("causal.recorded")
        }
    }

    #[cfg(feature = "graphql")]
    impl Aggregate for CausalDispatcherAggregate {
        type ReplayError = std::convert::Infallible;

        fn aggregate_type() -> &'static str {
            "service-causal-dispatcher-test"
        }

        fn entity(&self) -> &Entity {
            &self.entity
        }

        fn entity_mut(&mut self) -> &mut Entity {
            &mut self.entity
        }

        fn replay_event(&mut self, _event: &crate::EventRecord) -> Result<(), Self::ReplayError> {
            Ok(())
        }
    }

    #[cfg(feature = "graphql")]
    fn causal_test_principal() -> VerifiedPrincipal {
        VerifiedPrincipal::test_oidc(
            "https://issuer.example/",
            "causal-test-subject",
            &["distributed-tests"],
        )
    }

    #[cfg(feature = "graphql")]
    fn causal_test_command_id() -> String {
        uuid::Uuid::now_v7().hyphenated().to_string()
    }

    #[cfg(feature = "graphql")]
    fn causal_test_input(id: &str, label: &str) -> Value {
        json!({ "id": id, "label": label })
    }

    #[cfg(feature = "graphql")]
    fn session_with_role(role: &str) -> Session {
        let mut session = Session::new();
        session.set(crate::microsvc::ROLE_KEY, role);
        session
    }

    #[cfg(feature = "graphql")]
    #[derive(Clone, Copy)]
    enum InjectedCommitBehavior {
        CommitThenErrorOnce,
        ErrorBeforeCommitOnce,
        Delegate,
    }

    #[cfg(feature = "graphql")]
    #[derive(Clone)]
    struct AmbiguousCommitRepository {
        inner: InMemoryRepository,
        behavior: Arc<Mutex<InjectedCommitBehavior>>,
    }

    #[cfg(feature = "graphql")]
    impl AmbiguousCommitRepository {
        fn new(inner: InMemoryRepository, behavior: InjectedCommitBehavior) -> Self {
            Self {
                inner,
                behavior: Arc::new(Mutex::new(behavior)),
            }
        }

        fn injected_error() -> CommandLedgerError {
            CommandLedgerError::Storage(crate::RepositoryError::retryable_storage(
                "injected ambiguous causal commit",
                std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "injected transport acknowledgement loss",
                ),
            ))
        }
    }

    #[cfg(feature = "graphql")]
    impl CausalGetStream for AmbiguousCommitRepository {
        fn get_causal_stream<'a>(
            &'a self,
            identity: &'a crate::StreamIdentity,
        ) -> impl Future<Output = Result<Option<Entity>, crate::RepositoryError>> + Send + 'a
        {
            CausalGetStream::get_causal_stream(&self.inner, identity)
        }
    }

    #[cfg(feature = "graphql")]
    impl CausalRepositoryIdentity for AmbiguousCommitRepository {
        fn causal_storage_identity(&self) -> crate::command_ledger::CausalStorageIdentity {
            CausalRepositoryIdentity::causal_storage_identity(&self.inner)
        }
    }

    #[cfg(feature = "graphql")]
    impl CommandLedgerStore for AmbiguousCommitRepository {
        fn reserve_command(
            &self,
            reservation: CommandReservation,
        ) -> impl Future<Output = Result<ReservationOutcome, CommandLedgerError>> + Send + '_
        {
            CommandLedgerStore::reserve_command(&self.inner, reservation)
        }

        fn lookup_command<'a>(
            &'a self,
            key: &'a CommandLedgerKey,
            scope: CommandLookupScope<'a>,
        ) -> impl Future<Output = Result<CommandLookup, CommandLedgerError>> + Send + 'a {
            CommandLedgerStore::lookup_command(&self.inner, key, scope)
        }

        fn mark_retryable_unknown(
            &self,
            attempt: AttemptFence,
        ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + '_ {
            CommandLedgerStore::mark_retryable_unknown(&self.inner, attempt)
        }

        fn compact_expired_commands(
            &self,
            limit: usize,
        ) -> impl Future<Output = Result<u64, CommandLedgerError>> + Send + '_ {
            CommandLedgerStore::compact_expired_commands(&self.inner, limit)
        }
    }

    #[cfg(feature = "graphql")]
    impl CausalTransactionalCommit for AmbiguousCommitRepository {
        fn commit_causal_batch<'a>(
            &'a self,
            batch: CausalCommitBatch<'a>,
        ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + 'a {
            async move {
                let behavior = {
                    let mut behavior = self.behavior.lock().map_err(|_| {
                        CommandLedgerError::Storage(crate::RepositoryError::LockPoisoned(
                            "injected causal commit behavior",
                        ))
                    })?;
                    std::mem::replace(&mut *behavior, InjectedCommitBehavior::Delegate)
                };
                match behavior {
                    InjectedCommitBehavior::CommitThenErrorOnce => {
                        CausalTransactionalCommit::commit_causal_batch(&self.inner, batch).await?;
                        Err(Self::injected_error())
                    }
                    InjectedCommitBehavior::ErrorBeforeCommitOnce => Err(Self::injected_error()),
                    InjectedCommitBehavior::Delegate => {
                        CausalTransactionalCommit::commit_causal_batch(&self.inner, batch).await
                    }
                }
            }
        }
    }

    #[cfg(feature = "graphql")]
    impl HasOutboxStore for AmbiguousCommitRepository {
        type OutboxStore = crate::InMemoryOutboxStore;

        fn outbox_store(&self) -> Self::OutboxStore {
            self.inner.outbox_store()
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
    async fn typed_direct_dispatch_fails_before_invoking_handler() {
        TYPED_HANDLER_INVOKED.store(false, Ordering::SeqCst);
        let service = Service::new().named("todos").routes(
            Routes::new()
                .with_repo(
                    InMemoryRepository::new()
                        .queued()
                        .aggregate::<RouteComboAggregate>(),
                )
                .typed_command(typed_command::<TypedInput, Accepted<TypedOutput>>(
                    "todo.create",
                ))
                .handle(typed_handler),
        );

        let error = service
            .dispatch("todo.create", json!({ "id": "todo-1" }), Session::new())
            .await
            .expect_err("typed causal commands must reject direct dispatch");

        assert!(error.to_string().contains("verified GraphQL bearer"));
        assert!(!TYPED_HANDLER_INVOKED.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn typed_direct_dispatch_fails_before_invoking_guard_or_handler() {
        TYPED_GUARD_INVOKED.store(false, Ordering::SeqCst);
        TYPED_HANDLER_INVOKED.store(false, Ordering::SeqCst);
        let service = Service::new().named("todos").routes(
            Routes::new()
                .with_repo(
                    InMemoryRepository::new()
                        .queued()
                        .aggregate::<RouteComboAggregate>(),
                )
                .typed_command(typed_command::<TypedInput, Accepted<TypedOutput>>(
                    "todo.guarded_create",
                ))
                .guarded(
                    |_| {
                        TYPED_GUARD_INVOKED.store(true, Ordering::SeqCst);
                        true
                    },
                    typed_handler,
                ),
        );

        let error = service
            .dispatch(
                "todo.guarded_create",
                json!({ "id": "todo-1" }),
                Session::new(),
            )
            .await
            .expect_err("typed causal commands must reject before application guards");

        assert!(error.to_string().contains("verified GraphQL bearer"));
        assert!(!TYPED_GUARD_INVOKED.load(Ordering::SeqCst));
        assert!(!TYPED_HANDLER_INVOKED.load(Ordering::SeqCst));
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_dispatch_replays_canonical_equivalent_input_without_reinvoking_handler() {
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let route_handler_calls = Arc::clone(&handler_calls);
        let repository = InMemoryRepository::new();
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.clone().aggregate::<CausalDispatcherAggregate>())
                .typed_command(typed_command::<CausalTestInput, Accepted<TypedOutput>>(
                    "causal.replay",
                ))
                .handle(
                    move |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                          input: CausalTestInput| {
                        let calls = Arc::clone(&route_handler_calls);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            let _label = input.label;
                            Ok(
                                PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                    id: input.id,
                                })
                                .unwrap(),
                            )
                        }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();
        let first_input = serde_json::from_str(r#"{"id":"todo-1","label":"same"}"#).unwrap();
        let equivalent_input = serde_json::from_str(r#"{"label":"same","id":"todo-1"}"#).unwrap();

        let first = service
            .dispatch_causal(
                "causal.replay",
                &command_id,
                first_input,
                Session::new(),
                principal.clone(),
            )
            .await
            .unwrap();
        let replay = service
            .dispatch_causal(
                "causal.replay",
                &command_id,
                equivalent_input,
                Session::new(),
                principal,
            )
            .await
            .unwrap();

        assert_eq!(first, json!({ "id": "todo-1" }));
        assert_eq!(replay, first);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_dispatch_rejects_same_command_id_with_different_input() {
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let route_handler_calls = Arc::clone(&handler_calls);
        let repository = InMemoryRepository::new();
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.clone().aggregate::<CausalDispatcherAggregate>())
                .typed_command(typed_command::<CausalTestInput, Accepted<TypedOutput>>(
                    "causal.reuse",
                ))
                .handle(
                    move |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                          input: CausalTestInput| {
                        let calls = Arc::clone(&route_handler_calls);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            let _label = input.label;
                            Ok(
                                PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                    id: input.id,
                                })
                                .unwrap(),
                            )
                        }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();

        service
            .dispatch_causal(
                "causal.reuse",
                &command_id,
                causal_test_input("todo-1", "first"),
                Session::new(),
                principal.clone(),
            )
            .await
            .unwrap();
        let error = service
            .dispatch_causal(
                "causal.reuse",
                &command_id,
                causal_test_input("todo-1", "changed"),
                Session::new(),
                principal,
            )
            .await
            .expect_err("different canonical input must conflict");

        assert_eq!(error.code(), "COMMAND_ID_REUSE");
        assert_eq!(error.status_code(), 409);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_guard_rejection_is_replayed_without_guard_or_handler_callback() {
        let guard_calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let route_guard_calls = Arc::clone(&guard_calls);
        let route_handler_calls = Arc::clone(&handler_calls);
        let repository = InMemoryRepository::new();
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.clone().aggregate::<CausalDispatcherAggregate>())
                .typed_command(typed_command::<CausalTestInput, Accepted<TypedOutput>>(
                    "causal.guard_rejection",
                ))
                .guarded(
                    move |_| {
                        route_guard_calls.fetch_add(1, Ordering::SeqCst);
                        false
                    },
                    move |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                          input: CausalTestInput| {
                        let calls = Arc::clone(&route_handler_calls);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(
                                PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                    id: input.id,
                                })
                                .unwrap(),
                            )
                        }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();

        let first = service
            .dispatch_causal(
                "causal.guard_rejection",
                &command_id,
                causal_test_input("todo-1", "same"),
                Session::new(),
                principal.clone(),
            )
            .await
            .expect_err("guard should reject first attempt");
        let replay = service
            .dispatch_causal(
                "causal.guard_rejection",
                &command_id,
                causal_test_input("todo-1", "same"),
                Session::new(),
                principal,
            )
            .await
            .expect_err("guard rejection should replay");

        assert_eq!(first.code(), "REJECTED");
        assert_eq!(first.status_code(), 422);
        assert_eq!(replay.code(), first.code());
        assert_eq!(replay.status_code(), first.status_code());
        assert_eq!(replay.client_message(), first.client_message());
        assert_eq!(guard_calls.load(Ordering::SeqCst), 1);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_handler_rejection_is_replayed_without_reinvoking_handler() {
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let route_handler_calls = Arc::clone(&handler_calls);
        let repository = InMemoryRepository::new();
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.clone().aggregate::<CausalDispatcherAggregate>())
                .typed_command(typed_command::<CausalTestInput, Accepted<TypedOutput>>(
                    "causal.handler_rejection",
                ))
                .handle(
                    move |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                          _input: CausalTestInput| {
                        let calls = Arc::clone(&route_handler_calls);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Err::<PreparedCommand<Accepted<TypedOutput>>, HandlerError>(
                                HandlerError::Rejected("deterministic refusal".into()),
                            )
                        }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();

        let first = service
            .dispatch_causal(
                "causal.handler_rejection",
                &command_id,
                causal_test_input("todo-1", "same"),
                Session::new(),
                principal.clone(),
            )
            .await
            .expect_err("handler should reject first attempt");
        let replay = service
            .dispatch_causal(
                "causal.handler_rejection",
                &command_id,
                causal_test_input("todo-1", "same"),
                Session::new(),
                principal,
            )
            .await
            .expect_err("handler rejection should replay");

        assert_eq!(first.code(), "REJECTED");
        assert_eq!(first.status_code(), 422);
        assert_eq!(first.client_message(), "rejected: deterministic refusal");
        assert_eq!(replay.code(), first.code());
        assert_eq!(replay.status_code(), first.status_code());
        assert_eq!(replay.client_message(), first.client_message());
        assert_eq!(handler_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_dispatch_checks_current_role_before_reservation_guard_and_handler() {
        let guard_calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let route_guard_calls = Arc::clone(&guard_calls);
        let route_handler_calls = Arc::clone(&handler_calls);
        let repository = InMemoryRepository::new();
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.clone().aggregate::<CausalDispatcherAggregate>())
                .typed_command(
                    typed_command::<CausalTestInput, Accepted<TypedOutput>>("causal.role_guarded")
                        .roles(["admin"]),
                )
                .guarded(
                    move |_| {
                        route_guard_calls.fetch_add(1, Ordering::SeqCst);
                        true
                    },
                    move |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                          input: CausalTestInput| {
                        let calls = Arc::clone(&route_handler_calls);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(
                                PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                    id: input.id,
                                })
                                .unwrap(),
                            )
                        }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();

        let denied_before_reservation = service
            .dispatch_causal(
                "causal.role_guarded",
                &command_id,
                causal_test_input("todo-1", "same"),
                session_with_role("user"),
                principal.clone(),
            )
            .await
            .expect_err("current role must be denied before reservation");
        assert_eq!(denied_before_reservation.code(), "FORBIDDEN");
        assert_eq!(guard_calls.load(Ordering::SeqCst), 0);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 0);

        let accepted = service
            .dispatch_causal(
                "causal.role_guarded",
                &command_id,
                causal_test_input("todo-1", "same"),
                session_with_role("admin"),
                principal.clone(),
            )
            .await
            .expect("denied dispatch must not have reserved the command ID");
        assert_eq!(accepted, json!({ "id": "todo-1" }));

        let denied_before_replay = service
            .dispatch_causal(
                "causal.role_guarded",
                &command_id,
                causal_test_input("todo-1", "same"),
                session_with_role("user"),
                principal,
            )
            .await
            .expect_err("current role must be rechecked before replay");
        assert_eq!(denied_before_replay.code(), "FORBIDDEN");
        assert_eq!(guard_calls.load(Ordering::SeqCst), 1);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 1);

        let denied_lookup = service
            .lookup_causal_command(
                "causal.role_guarded",
                &command_id,
                &session_with_role("user"),
                causal_test_principal(),
            )
            .await
            .expect_err("current role must also be rechecked before status lookup");
        assert_eq!(denied_lookup.code(), "FORBIDDEN");
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_status_lookup_does_not_disclose_another_routes_command() {
        let repository = InMemoryRepository::new();
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.aggregate::<CausalDispatcherAggregate>())
                .typed_command(
                    typed_command::<CausalTestInput, Accepted<TypedOutput>>("causal.admin_only")
                        .roles(["admin"]),
                )
                .handle(
                    |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                     input: CausalTestInput| async move {
                        Ok(
                            PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                id: input.id,
                            })
                            .unwrap(),
                        )
                    },
                )
                .typed_command(
                    typed_command::<CausalTestInput, Accepted<TypedOutput>>("causal.user_allowed")
                        .roles(["user"]),
                )
                .handle(
                    |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                     input: CausalTestInput| async move {
                        Ok(
                            PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                id: input.id,
                            })
                            .unwrap(),
                        )
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();

        service
            .dispatch_causal(
                "causal.admin_only",
                &command_id,
                causal_test_input("todo-secret", "classified"),
                session_with_role("admin"),
                principal.clone(),
            )
            .await
            .expect("admin should be able to commit the protected command");

        let denied = service
            .lookup_causal_command(
                "causal.admin_only",
                &command_id,
                &session_with_role("user"),
                principal.clone(),
            )
            .await
            .expect_err("the current role must not retain access to the protected route");
        assert_eq!(denied.code(), "FORBIDDEN");

        let cross_route = service
            .lookup_causal_command(
                "causal.user_allowed",
                &command_id,
                &session_with_role("user"),
                principal,
            )
            .await
            .expect("the allowed route should produce a non-disclosing status result");
        assert_eq!(cross_route, CommandLookup::Unknown);
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_dispatch_overwrites_event_and_outbox_causation_with_ledger_identity() {
        let observed_causation = Arc::new(Mutex::new(None::<String>));
        let route_observed_causation = Arc::clone(&observed_causation);
        let projector_causation = Arc::new(Mutex::new(None::<String>));
        let route_projector_causation = Arc::clone(&projector_causation);
        let repository = InMemoryRepository::new();
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.clone().aggregate::<CausalDispatcherAggregate>())
                .typed_command(typed_command::<CausalTestInput, Accepted<TypedOutput>>(
                    "causal.persist",
                ))
                .handle(
                    move |context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                          input: CausalTestInput| {
                        let observed = Arc::clone(&route_observed_causation);
                        let result = (|| {
                            let causation = context
                                .causation_id()
                                .expect("reserved command causation")
                                .to_string();
                            *observed.lock().unwrap() = Some(causation);

                            let mut checkout = context.create();
                            checkout
                                .entity_mut()
                                .set_causation_id("handler-supplied-event-causation");
                            checkout
                                .record(input.id.clone())
                                .map_err(|error| HandlerError::Other(Box::new(error)))?;
                            context.stage(checkout)?;

                            let mut outbox = OutboxMessage::create(
                                format!("{}:fact", input.id),
                                "causal.recorded",
                                input.label.as_bytes().to_vec(),
                            )
                            .map_err(|error| HandlerError::Other(Box::new(error)))?;
                            outbox.set_causation_id("handler-supplied-outbox-causation");
                            context.stage_outbox(outbox)?;

                            Ok(
                                PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                    id: input.id,
                                })
                                .unwrap(),
                            )
                        })();
                        async move { result }
                    },
                )
                .event("causal.recorded")
                .handle(
                    move |context: &Context<
                        AggregateRepository<InMemoryRepository, CausalDispatcherAggregate>,
                    >| {
                        let causation = context.message().causation_id().map(str::to_string);
                        let observed = Arc::clone(&route_projector_causation);
                        async move {
                            *observed.lock().unwrap() = causation;
                            Ok(json!({ "projected": true }))
                        }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let mut session = Session::new();
        session.set(
            crate::trace_context::CAUSATION_ID,
            "caller-supplied-causation",
        );
        session.set(crate::trace_context::CORRELATION_ID, "caller-correlation");
        session.set(
            crate::trace_context::TRACEPARENT,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        );
        session.set(crate::trace_context::TRACESTATE, "vendor=value");

        let result = service
            .dispatch_causal(
                "causal.persist",
                &command_id,
                causal_test_input("todo-causal", "payload"),
                session,
                causal_test_principal(),
            )
            .await
            .unwrap();
        assert_eq!(result, json!({ "id": "todo-causal" }));

        let causation = observed_causation
            .lock()
            .unwrap()
            .clone()
            .expect("handler observed causation");
        let parsed_causation = uuid::Uuid::parse_str(&causation).unwrap();
        assert_eq!(parsed_causation.get_version_num(), 7);
        assert_ne!(causation, command_id);
        assert_ne!(causation, "caller-supplied-causation");
        assert_ne!(causation, "handler-supplied-event-causation");
        assert_ne!(causation, "handler-supplied-outbox-causation");

        let identity =
            crate::StreamIdentity::new(CausalDispatcherAggregate::aggregate_type(), "todo-causal")
                .unwrap();
        let stored = repository
            .get_stream(&identity)
            .await
            .unwrap()
            .expect("causal aggregate stream");
        assert_eq!(stored.events().len(), 1);
        assert_eq!(stored.events()[0].causation_id(), Some(causation.as_str()));

        let outbox_store = repository.outbox_store();
        let pending = outbox_store.pending(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].causation_id(), Some(causation.as_str()));

        let projector_input = Message::from(pending[0].clone());
        service.dispatch_message(&projector_input).await.unwrap();
        assert_eq!(
            projector_causation.lock().unwrap().as_deref(),
            Some(causation.as_str())
        );
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_dispatch_uses_the_configured_immediate_outbox_publisher() {
        let repository = InMemoryRepository::new();
        let observed_broker_metadata = Arc::new(Mutex::new(None::<[String; 4]>));
        let route_observed_broker_metadata = Arc::clone(&observed_broker_metadata);
        let service = Service::new()
            .named("causal-tests")
            .routes(
                Routes::new()
                    .with_repo(repository.clone().aggregate::<CausalDispatcherAggregate>())
                    .typed_command(typed_command::<CausalTestInput, Accepted<TypedOutput>>(
                        "causal.publish_immediately",
                    ))
                    .handle(
                        |context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                         input: CausalTestInput| {
                            let result =
                                (|| {
                                    let mut checkout = context.create();
                                    checkout
                                        .record(input.id.clone())
                                        .map_err(|error| HandlerError::Other(Box::new(error)))?;
                                    context.stage(checkout)?;
                                    context.stage_outbox(
                                        OutboxMessage::create(
                                            format!("{}:immediate-fact", input.id),
                                            "causal.immediate_fact",
                                            input.label.as_bytes().to_vec(),
                                        )
                                        .map_err(|error| HandlerError::Other(Box::new(error)))?,
                                    )?;
                                    Ok(PreparedCommand::<Accepted<TypedOutput>>::prepare(
                                        TypedOutput { id: input.id },
                                    )
                                    .unwrap())
                                })();
                            async move { result }
                        },
                    )
                    .event("causal.immediate_fact")
                    .handle(
                        move |context: &Context<
                            AggregateRepository<InMemoryRepository, CausalDispatcherAggregate>,
                        >| {
                            let message = context.message();
                            let metadata = [
                                message.causation_id().unwrap_or_default().to_string(),
                                message
                                    .metadata("x-sourced-source-aggregate-type")
                                    .unwrap_or_default()
                                    .to_string(),
                                message
                                    .metadata("x-sourced-source-aggregate-id")
                                    .unwrap_or_default()
                                    .to_string(),
                                message
                                    .metadata("x-sourced-source-sequence")
                                    .unwrap_or_default()
                                    .to_string(),
                            ];
                            let observed = Arc::clone(&route_observed_broker_metadata);
                            async move {
                                *observed.lock().unwrap() = Some(metadata);
                                Ok(json!({ "projected": true }))
                            }
                        },
                    ),
            )
            .with_bus(crate::bus::InMemoryBus::new());

        service
            .dispatch_causal(
                "causal.publish_immediately",
                &causal_test_command_id(),
                causal_test_input("todo-immediate", "payload"),
                Session::new(),
                causal_test_principal(),
            )
            .await
            .expect("causal dispatch should commit before immediate publication");

        let outbox = repository.outbox_store();
        assert!(outbox.pending(usize::MAX).await.unwrap().is_empty());
        let published = outbox
            .messages_by_status(crate::outbox::OutboxMessageStatus::Published, usize::MAX)
            .await
            .unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].id(), "todo-immediate:immediate-fact");
        assert_eq!(published[0].event_type, "causal.immediate_fact");
        let causation = published[0]
            .causation_id()
            .expect("persisted outbox row should retain ledger causation")
            .to_string();
        assert_eq!(
            published[0].source_aggregate_type.as_deref(),
            Some(CausalDispatcherAggregate::aggregate_type())
        );
        assert_eq!(
            published[0].source_aggregate_id.as_deref(),
            Some("todo-immediate")
        );
        assert_eq!(published[0].source_sequence, Some(1));

        service
            .run(RunOptions::idempotent())
            .await
            .expect("attached bus should deliver the immediately published fact");
        assert_eq!(
            observed_broker_metadata.lock().unwrap().as_ref(),
            Some(&[
                causation,
                CausalDispatcherAggregate::aggregate_type().to_string(),
                "todo-immediate".to_string(),
                "1".to_string(),
            ]),
            "the post-commit clone must carry authoritative causation and aggregate source metadata",
        );
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_dispatch_replay_contains_resolved_projection_obligation() {
        let repository = InMemoryRepository::new();
        let projector = SurfaceProjector::new("project_causal_obligation")
            .facts(["causal.obligation_fact"])
            .models(["CausalProjectionObligationView"]);
        let confirmations = crate::command_confirmations! {
            input: CausalProjectionInput;
            confirm projector -> CausalProjectionObligationView {
                key { id: input.id },
                partition: input.partition
            };
        };
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.clone().aggregate::<CausalDispatcherAggregate>())
                .typed_command(
                    typed_command::<CausalProjectionInput, Accepted<TypedOutput>>(
                        "causal.projection_obligation",
                    )
                    .confirmations(confirmations),
                )
                .handle(
                    |context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                     input: CausalProjectionInput| {
                        let result = (|| {
                            context.stage_outbox(
                                OutboxMessage::create(
                                    format!("{}:obligation", input.id),
                                    "causal.obligation_fact",
                                    input.partition.as_bytes().to_vec(),
                                )
                                .map_err(|error| HandlerError::Other(Box::new(error)))?,
                            )?;
                            Ok(
                                PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                    id: input.id,
                                })
                                .unwrap(),
                            )
                        })();
                        async move { result }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();

        let result = service
            .dispatch_causal(
                "causal.projection_obligation",
                &command_id,
                json!({
                    "todoId": "todo-obligation",
                    "tenantPartition": "tenant-a"
                }),
                Session::new(),
                principal.clone(),
            )
            .await
            .expect("matching outbox fact should make the causal dispatch commit");
        assert_eq!(result, json!({ "id": "todo-obligation" }));

        let lookup = service
            .lookup_causal_command(
                "causal.projection_obligation",
                &command_id,
                &Session::new(),
                principal,
            )
            .await
            .expect("same principal should be able to recover its command");
        let CommandLookup::Replay(replay) = lookup else {
            panic!("completed command should be replayable");
        };
        assert_eq!(replay.state, CommandLedgerState::AcceptedPendingProjection);
        assert_eq!(replay.projection_obligations.len(), 1);

        let obligation = &replay.projection_obligations[0];
        assert_eq!(obligation.projector, "project_causal_obligation");
        assert_eq!(obligation.model, "CausalProjectionObligationView");
        assert_eq!(obligation.key.fields.len(), 1);
        assert_eq!(obligation.key.fields[0].field, "id");
        assert_eq!(obligation.key.fields[0].value, json!("todo-obligation"));
        assert_eq!(obligation.partition, Some(json!("tenant-a")));

        let pending = repository.outbox_store().pending(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_type, "causal.obligation_fact");
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_dispatch_recovers_committed_replay_after_commit_acknowledgement_loss() {
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let route_handler_calls = Arc::clone(&handler_calls);
        let repository = AmbiguousCommitRepository::new(
            InMemoryRepository::new(),
            InjectedCommitBehavior::CommitThenErrorOnce,
        );
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.aggregate::<CausalDispatcherAggregate>())
                .typed_command(typed_command::<CausalTestInput, Accepted<TypedOutput>>(
                    "causal.ambiguous_committed",
                ))
                .handle(
                    move |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                          input: CausalTestInput| {
                        let calls = Arc::clone(&route_handler_calls);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(
                                PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                    id: input.id,
                                })
                                .unwrap(),
                            )
                        }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();
        let input = causal_test_input("todo-ambiguous", "same");

        let recovered = service
            .dispatch_causal(
                "causal.ambiguous_committed",
                &command_id,
                input.clone(),
                Session::new(),
                principal.clone(),
            )
            .await
            .expect("lookup should recover the committed outcome");
        assert_eq!(recovered, json!({ "id": "todo-ambiguous" }));
        assert!(matches!(
            service
                .lookup_causal_command(
                    "causal.ambiguous_committed",
                    &command_id,
                    &Session::new(),
                    principal.clone(),
                )
                .await
                .unwrap(),
            CommandLookup::Replay(_)
        ));

        let replay = service
            .dispatch_causal(
                "causal.ambiguous_committed",
                &command_id,
                input,
                Session::new(),
                principal,
            )
            .await
            .unwrap();
        assert_eq!(replay, recovered);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "graphql")]
    #[tokio::test]
    async fn causal_dispatch_reclaims_retryable_attempt_after_precommit_failure() {
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let route_handler_calls = Arc::clone(&handler_calls);
        let repository = AmbiguousCommitRepository::new(
            InMemoryRepository::new(),
            InjectedCommitBehavior::ErrorBeforeCommitOnce,
        );
        let service = Service::new().named("causal-tests").routes(
            Routes::new()
                .with_repo(repository.aggregate::<CausalDispatcherAggregate>())
                .typed_command(typed_command::<CausalTestInput, Accepted<TypedOutput>>(
                    "causal.ambiguous_retry",
                ))
                .handle(
                    move |_context: &CausalCommandContext<'_, CausalDispatcherAggregate>,
                          input: CausalTestInput| {
                        let calls = Arc::clone(&route_handler_calls);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(
                                PreparedCommand::<Accepted<TypedOutput>>::prepare(TypedOutput {
                                    id: input.id,
                                })
                                .unwrap(),
                            )
                        }
                    },
                ),
        );
        let command_id = causal_test_command_id();
        let principal = causal_test_principal();
        let input = causal_test_input("todo-retry", "same");

        let first = service
            .dispatch_causal(
                "causal.ambiguous_retry",
                &command_id,
                input.clone(),
                Session::new(),
                principal.clone(),
            )
            .await
            .expect_err("pre-commit failure should remain unknown to the caller");
        assert_eq!(first.code(), "INTERNAL");
        assert_eq!(handler_calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            service
                .lookup_causal_command(
                    "causal.ambiguous_retry",
                    &command_id,
                    &Session::new(),
                    principal.clone(),
                )
                .await
                .unwrap(),
            CommandLookup::RetryableUnknown { .. }
        ));

        let retried = service
            .dispatch_causal(
                "causal.ambiguous_retry",
                &command_id,
                input.clone(),
                Session::new(),
                principal.clone(),
            )
            .await
            .expect("same-ID retry should reclaim and commit");
        assert_eq!(retried, json!({ "id": "todo-retry" }));
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);

        let replay = service
            .dispatch_causal(
                "causal.ambiguous_retry",
                &command_id,
                input,
                Session::new(),
                principal,
            )
            .await
            .unwrap();
        assert_eq!(replay, retried);
        assert_eq!(handler_calls.load(Ordering::SeqCst), 2);
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
            metadata: vec![("X-User-Id".to_string(), "user-1".to_string())],
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
        session.set(crate::microsvc::ROLE_KEY, "admin");
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
        vars.insert(
            crate::microsvc::USER_ID_KEY.to_string(),
            "user-99".to_string(),
        );
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
/// Generic command envelope used by in-process dispatch and adapters that
/// already decoded a gateway payload. Example shape:
/// ```json
/// {
///   "command": "order.create",
///   "input": { "product_id": "SKU-1" },
///   "session_variables": { "x-user-id": "user-42" }
/// }
/// ```
///
/// `session_variables` keys are deployment convention (see [`Session`]). A
/// query-layer action (Hasura, custom BFF, …) can map its native claims into
/// these variables before calling `dispatch_request`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandRequest {
    /// Command name (URL path, action name, or explicit field).
    pub command: String,
    /// JSON input payload.
    pub input: Value,
    /// Opaque session variables (identity claims, roles, tenant, etc.).
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
