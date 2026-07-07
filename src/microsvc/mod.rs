//! microsvc — Convention-based microservice command handler framework.
//!
//! Build microservices by registering command and event handlers on typed
//! `Routes<D>` bundles, then adding those bundles to a deployment-level
//! `Service`. Each handler receives a `Context<D>` with access to the input
//! payload, session variables, and its route dependencies.
//!
//! ## Quick Start
//!
//! Dispatch is **async** — `dispatch`, `handle`, and the commit path all return
//! futures and are awaited.
//!
//! ```ignore
//! use std::sync::Arc;
//! use distributed::{microsvc, InMemoryRepository};
//! use serde_json::json;
//!
//! let routes = microsvc::Routes::new()
//!     .with_repo(InMemoryRepository::new().queued().aggregate::<Order>())
//!     .command("order.create")
//!     .handle(|ctx| {
//!         let input = ctx.input::<CreateOrderInput>();
//!         async move { Ok(json!({ "id": input?.id })) }
//!     });
//! let service = Arc::new(microsvc::Service::new().routes(routes));
//!
//! // Direct dispatch (async)
//! let result = service
//!     .dispatch("order.create", json!({ "id": "o1" }), microsvc::Session::new())
//!     .await?;
//!
//! // HTTP transport (requires "http" feature)
//! // microsvc::serve(service, "0.0.0.0:3000").await?;
//! ```
//!
//! ## Handler Convention
//!
//! Each handler file follows this convention. `handle` is **async**:
//!
//! ```ignore
//! // src/handlers/order_create.rs
//!
//! pub const COMMAND: &str = "order.create";
//!
//! pub fn guard(ctx: &microsvc::Context<Repo>) -> bool {
//!     ctx.has_fields(&["id", "product_id"])
//! }
//!
//! pub async fn handle(ctx: &microsvc::Context<'_, Repo>) -> Result<Value, microsvc::HandlerError> {
//!     let input = ctx.input::<CreateOrderInput>()?;
//!     let mut order = Order::default();
//!     order.create(input.id)?;
//!     ctx.repo().commit(&mut order).await?;
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
    ConfigurableOutboxPublisher, HasOutboxStore, HasReadModelStore, HasRepo,
    ReadModelStoreDependencies, RepoDependencies, RepoReadModelDependencies,
};
pub use error::HandlerError;
pub use runtime::{DEFAULT_MAX_PUBLISH_ATTEMPTS, DEFAULT_PUBLISH_LEASE};
pub use service::{
    CommandRequest, CommandResponse, DeliveryKind, HandlerNames, HandlerSpec, RouteBuilder, Routes,
    Service,
};
pub use session::Session;

/// Maximum accepted HTTP request body size for the microsvc ingresses, in bytes
/// (1 MiB).
///
/// Pins axum's implicit 2 MiB default to an explicit, smaller ceiling shared by
/// the command [`router`] and the CloudEvents [`cloud_events_router`]: both
/// buffer the whole body into memory, so an unbounded body is a
/// memory-amplification vector. Raise it deliberately if a deployment needs
/// larger payloads.
#[cfg(feature = "http")]
pub const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;

// HTTP transport (requires "http" feature)
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "http")]
pub use http::{router, router_with_diagnostics, serve};

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

/// Register handler modules with a route bundle using the convention pattern.
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
/// let routes = distributed::routes!(
///     microsvc::Routes::new().with_repo(repo),
///     command handlers::counter_create,
///     command handlers::counter_increment,
///     event handlers::counter_rebuilt,
///     events handlers::counter_projection,
/// );
/// let service = microsvc::Service::new().routes(routes);
/// ```
#[macro_export]
macro_rules! routes {
    ($routes:expr $(,)?) => {
        $routes
    };
    ($routes:expr, $($rest:tt)+) => {
        $crate::__routes!($routes, $($rest)+)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __routes {
    ($routes:expr, command $($seg:ident)::+ $(, $($rest:tt)*)?) => {
        $crate::__routes_continue!(
            $routes.command($($seg)::+::COMMAND).guarded(
                $($seg)::+::guard,
                $($seg)::+::handle,
            )
            $(, $($rest)*)?
        )
    };
    ($routes:expr, event $($seg:ident)::+ $(, $($rest:tt)*)?) => {
        $crate::__routes_continue!(
            $routes.event($($seg)::+::EVENT).guarded(
                $($seg)::+::guard,
                $($seg)::+::handle,
            )
            $(, $($rest)*)?
        )
    };
    ($routes:expr, events $($seg:ident)::+ $(, $($rest:tt)*)?) => {
        $crate::__routes_continue!(
            $routes.events($($seg)::+::EVENTS).guarded(
                $($seg)::+::guard,
                $($seg)::+::handle,
            )
            $(, $($rest)*)?
        )
    };
    ($routes:expr, $($seg:ident)::+ $(, $($rest:tt)*)?) => {
        compile_error!(
            "routes! entries must be prefixed with `command`, `event`, or `events`"
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __routes_continue {
    ($routes:expr) => {
        $routes
    };
    ($routes:expr,) => {
        $routes
    };
    ($routes:expr, $($rest:tt)+) => {
        $crate::__routes!($routes, $($rest)+)
    };
}
