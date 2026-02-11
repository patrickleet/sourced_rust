//! microsvc — Convention-based microservice command handler framework.
//!
//! Build microservices by registering command handlers on a `Service`.
//! Each handler receives a `Context<R>` with access to the input payload,
//! session variables, and the repository.
//!
//! ## Quick Start
//!
//! ```ignore
//! use std::sync::Arc;
//! use sourced_rust::{microsvc, HashMapRepository};
//! use serde_json::json;
//!
//! let service = Arc::new(
//!     microsvc::Service::new(HashMapRepository::new())
//!         .command("order.create", |ctx| {
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
//! pub fn guard<R>(ctx: &microsvc::Context<R>) -> bool {
//!     ctx.has_fields(&["id", "product_id"])
//! }
//!
//! pub fn handle<R: GetAggregate + CommitAggregate>(
//!     ctx: &microsvc::Context<R>,
//! ) -> Result<Value, microsvc::HandlerError> {
//!     let input = ctx.input::<CreateOrderInput>()?;
//!     let mut order = Order::default();
//!     order.create(input.id);
//!     ctx.repo().commit_aggregate(&mut order)?;
//!     Ok(json!({ "id": order.entity().id() }))
//! }
//! ```

mod context;
mod error;
mod service;
mod session;

pub use context::Context;
pub use error::HandlerError;
pub use service::{CommandRequest, CommandResponse, Service};
pub use session::Session;

// Bus transports (requires "bus" feature)
#[cfg(feature = "bus")]
pub use service::{listen, subscribe, TransportHandle, TransportStats};

// HTTP transport (requires "http" feature)
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "http")]
pub use http::{router, serve};

/// Register handler modules with a service using the convention pattern.
///
/// Each handler module must export:
/// - `COMMAND: &str` — the command name
/// - `guard(ctx) -> bool` — input validation
/// - `handle(ctx) -> Result<Value, HandlerError>` — the handler
///
/// # Example
/// ```ignore
/// let service = sourced_rust::register_handlers!(
///     microsvc::Service::new(HashMapRepository::new()),
///     handlers::counter_create,
///     handlers::counter_increment,
/// );
/// ```
#[macro_export]
macro_rules! register_handlers {
    ($service:expr, $( $($seg:ident)::+ ),+ $(,)?) => {
        $service
        $(
            .command_guarded(
                $($seg)::+::COMMAND,
                $($seg)::+::guard,
                $($seg)::+::handle,
            )
        )+
    };
}
