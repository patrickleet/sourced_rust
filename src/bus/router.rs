//! The consume seam: a minimal router the [`run_source`](super::run_source) loop
//! and the [`BusConsumer`](super::BusConsumer) adapters depend on instead of the
//! concrete `microsvc::Service`.
//!
//! Implemented by `microsvc::Service<D>` (the rich, dependency-carrying registry)
//! and — once it lands — the dependency-free `Handlers` builder. Splitting the
//! consume path behind this trait is what lets the bus become a standalone module
//! that does not name `Service`. See `specs/bus-module-decomposition`.

use std::future::Future;

use super::TransportError;
use super::{Message, MessageKind, SubscriptionPlan};

/// What the receive loop and the consumer adapters need from a message consumer.
///
/// Three responsibilities, each used by a different part of the consume path:
///
/// - [`handles`](MessageRouter::handles) — the runner's ack-and-ignore vs dispatch
///   predicate;
/// - [`subscription_plan`](MessageRouter::subscription_plan) — the command/event
///   names the adapters use to build their broker topology *before* the run loop;
/// - [`dispatch`](MessageRouter::dispatch) — route and run one delivered message,
///   returning an already-classified [`TransportError`] so the bus-core runner
///   never sees a `microsvc::HandlerError`.
///
/// Not dyn-compatible (the RPITIT `dispatch`), exactly like [`Bus`](super::Bus) /
/// [`BusConsumer`](super::BusConsumer): consume it as `Arc<R>` or a generic
/// `<R: MessageRouter>`, never `Arc<dyn MessageRouter>`.
pub trait MessageRouter: Send + Sync {
    /// Stable identity for this consumer, used by broker adapters as the default
    /// durable consumer group when the bus itself was not configured with one.
    fn consumer_group(&self) -> Option<&str> {
        None
    }

    /// Whether this router has a handler for `(kind, name)`. The runner acks and
    /// ignores a delivered message it does not handle rather than dead-lettering it.
    fn handles(&self, kind: MessageKind, name: &str) -> bool;

    /// The command and event names a transport should subscribe to, derived from
    /// the router's registered handlers.
    fn subscription_plan(&self) -> SubscriptionPlan;

    /// Route and run one delivered message. `Ok(())` means the handler (and its
    /// effects) succeeded; the error is already classified retryable/permanent.
    fn dispatch(
        &self,
        message: &Message,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;
}
