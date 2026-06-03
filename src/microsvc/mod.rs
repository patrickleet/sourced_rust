//! microsvc — Convention-based microservice command handler framework.
//!
//! Build microservices by registering command and event handlers on a `Service`.
//! Each handler receives a `Context<D>` with access to the input payload,
//! session variables, and the service dependencies.
//!
//! ## Quick Start
//!
//! ```ignore
//! use std::sync::Arc;
//! use distributed::{microsvc, HashMapRepository};
//! use serde_json::json;
//!
//! let service = Arc::new(
//!     microsvc::Service::with_repo(HashMapRepository::new())
//!         .command("order.create")
//!         .handle(|ctx| {
//!             let input = ctx.input::<CreateOrderInput>()?;
//!             Ok(json!({ "id": input.id }))
//!         })
//! );
//!
//! // Direct dispatch
//! let result = service.dispatch("order.create", json!({ "id": "o1" }), microsvc::Session::new());
//!
//! // HTTP transport (requires "http" feature)
//! // microsvc::serve(service, "0.0.0.0:3000").await?;
//! ```
//!
//! ## Handler Convention
//!
//! Each handler file follows this convention:
//!
//! ```ignore
//! // src/handlers/order_create.rs
//!
//! pub const COMMAND: &str = "order.create";
//!
//! pub fn guard<D>(ctx: &microsvc::Context<D>) -> bool {
//!     ctx.has_fields(&["id", "product_id"])
//! }
//!
//! pub fn handle<D>(ctx: &microsvc::Context<D>) -> Result<Value, microsvc::HandlerError>
//! where
//!     D: microsvc::HasRepo,
//!     D::Repo: CommitAggregate,
//! {
//!     let input = ctx.input::<CreateOrderInput>()?;
//!     let mut order = Order::default();
//!     order.create(input.id);
//!     ctx.repo().commit_aggregate(&mut order)?;
//!     Ok(json!({ "id": order.entity().id() }))
//! }
//! ```

mod context;
mod dependencies;
mod error;
mod message_router;
mod runtime;
mod service;
mod session;

pub use crate::bus::{Message, MessageKind, PayloadDecodeError, SubscriptionPlan};
pub use context::Context;
pub use dependencies::{
    HasOutboxStore, HasReadModelStore, HasRepo, ReadModelStoreDependencies, RepoDependencies,
    RepoReadModelDependencies,
};
pub use error::HandlerError;
pub use runtime::{Microservice, DEFAULT_MAX_PUBLISH_ATTEMPTS, DEFAULT_PUBLISH_LEASE};
pub use service::{
    CommandRequest, CommandResponse, DeliveryKind, HandlerBuilder, HandlerNames, HandlerSpec,
    Service,
};
pub use session::Session;

// HTTP transport (requires "http" feature)
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "http")]
pub use http::{router, serve};

// Knative / CloudEvents HTTP ingress (Service-coupled; the bus keeps only the
// produce/manifest helpers). Requires the "http" feature.
#[cfg(feature = "http")]
mod knative_ingress;
#[cfg(feature = "http")]
pub use knative_ingress::cloud_events_router;

// gRPC transport (requires "grpc" feature)
#[cfg(feature = "grpc")]
pub mod grpc;
#[cfg(feature = "grpc")]
pub use grpc::{grpc_server, serve_grpc, GrpcServeError};

/// Register handler modules with a service using the convention pattern.
///
/// Each handler entry must be prefixed with `command`, `event`, or `events`.
///
/// Command handler modules must export:
/// - `COMMAND: &str` — the command name
/// - `guard(ctx) -> bool` — input validation
/// - `handle(ctx) -> Result<Value, HandlerError>` — the handler
///
/// Event handler modules must export:
/// - `EVENT: &str` or `EVENTS: &[&str]` — event names
/// - `guard(ctx) -> bool` — input validation
/// - `handle(ctx) -> Result<Value, HandlerError>` — the handler
///
/// # Example
/// ```ignore
/// let service = distributed::register_handlers!(
///     microsvc::Service::with_repo(HashMapRepository::new()),
///     command handlers::counter_create,
///     command handlers::counter_increment,
///     event handlers::counter_rebuilt,
///     events handlers::counter_projection,
/// );
/// ```
#[macro_export]
macro_rules! register_handlers {
    ($service:expr $(,)?) => {
        $service
    };
    ($service:expr, $($rest:tt)+) => {
        $crate::__register_handlers!($service, $($rest)+)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __register_handlers {
    ($service:expr, command $($seg:ident)::+ $(, $($rest:tt)*)?) => {
        $crate::__register_handlers_continue!(
            $service.command($($seg)::+::COMMAND).guarded(
                $($seg)::+::guard,
                $($seg)::+::handle,
            )
            $(, $($rest)*)?
        )
    };
    ($service:expr, event $($seg:ident)::+ $(, $($rest:tt)*)?) => {
        $crate::__register_handlers_continue!(
            $service.event($($seg)::+::EVENT).guarded(
                $($seg)::+::guard,
                $($seg)::+::handle,
            )
            $(, $($rest)*)?
        )
    };
    ($service:expr, events $($seg:ident)::+ $(, $($rest:tt)*)?) => {
        $crate::__register_handlers_continue!(
            $service.events($($seg)::+::EVENTS).guarded(
                $($seg)::+::guard,
                $($seg)::+::handle,
            )
            $(, $($rest)*)?
        )
    };
    ($service:expr, $($seg:ident)::+ $(, $($rest:tt)*)?) => {
        compile_error!(
            "register_handlers! entries must be prefixed with `command`, `event`, or `events`"
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __register_handlers_continue {
    ($service:expr) => {
        $service
    };
    ($service:expr,) => {
        $service
    };
    ($service:expr, $($rest:tt)+) => {
        $crate::__register_handlers!($service, $($rest)+)
    };
}
