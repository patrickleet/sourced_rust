use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::aggregate::Aggregate;
use crate::bus::Message;
use crate::graphql::command_contract::CommandOutcome;
use crate::graphql::{GraphqlOutputType, PreparedCommand, Projected};
use crate::microsvc::causal::{CausalWorkspace, CausalWorkspaceError};
use crate::microsvc::context::Context;
use crate::microsvc::error::HandlerError;
use crate::microsvc::session::Session;
use crate::outbox::OutboxMessage;
use crate::read_model::{ReadModelWritePlanBuilder, RelationalReadModel};

pub(super) type GuardFn<D> = dyn Fn(&Context<D>) -> bool + Send + Sync;
pub(super) type HandlerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, HandlerError>> + Send + 'a>>;
pub(super) type ProjectorBootstrapFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), HandlerError>> + Send + 'a>>;
pub(super) type HandlerFn<D> =
    dyn for<'a> Fn(&'a Context<'a, D>) -> HandlerFuture<'a> + Send + Sync;

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
    pub(super) fn new(
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
    ) -> Result<Option<crate::microsvc::AggregateCheckout<A>>, HandlerError> {
        self.workspace
            .load(id)
            .await
            .map_err(workspace_handler_error)
    }

    /// Start a new aggregate checkout. The handler must assign a valid entity
    /// identity before staging it.
    pub fn create(&self) -> crate::microsvc::AggregateCheckout<A> {
        self.workspace.create()
    }

    /// Stage a checkout for the framework-owned atomic commit.
    pub fn stage(
        &self,
        checkout: crate::microsvc::AggregateCheckout<A>,
    ) -> Result<(), HandlerError> {
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

pub(super) fn workspace_handler_error(error: CausalWorkspaceError) -> HandlerError {
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

pub(super) fn boxed_handler<D, F>(handler: F) -> Arc<HandlerFn<D>>
where
    F: for<'a> Handler<'a, D> + 'static,
{
    Arc::new(move |ctx| Box::pin(handler.call(ctx)) as HandlerFuture<'_>)
}

pub(super) type PreparedHandlerFuture<'a, K> =
    Pin<Box<dyn Future<Output = Result<PreparedCommand<K>, HandlerError>> + Send + 'a>>;
pub(super) type PreparedHandlerFn<A, I, K> = dyn for<'a> Fn(&'a CausalCommandContext<'a, A>, I) -> PreparedHandlerFuture<'a, K>
    + Send
    + Sync;
pub(super) type CausalGuardFn<A> =
    dyn for<'a> Fn(&CausalCommandContext<'a, A>) -> bool + Send + Sync;

pub(super) fn boxed_prepared_handler<A, I, K, F>(handler: F) -> Arc<PreparedHandlerFn<A, I, K>>
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

pub(super) fn boxed_causal_guard<A, G>(guard: G) -> Arc<CausalGuardFn<A>>
where
    A: Aggregate + Send + Sync + 'static,
    G: for<'a> Fn(&CausalCommandContext<'a, A>) -> bool + Send + Sync + 'static,
{
    Arc::new(guard)
}
