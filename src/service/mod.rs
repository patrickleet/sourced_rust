//! Domain Service — command handler registration and dispatch.
//!
//! This module provides `DomainService<R>`, a registry of named command
//! handlers backed by a shared repository. Commands are dispatched by
//! message type, either directly via `handle()` or from a background
//! thread listening on a bus queue via `DomainServiceThread`.
//!
//! ## Quick Start
//!
//! ```ignore
//! use sourced_rust::{DomainService, HashMapRepository};
//! use sourced_rust::bus::Message;
//!
//! let repo = HashMapRepository::new();
//! let service = DomainService::new(repo)
//!     .command("order.create", |repo, msg| {
//!         // load/create aggregate, apply domain logic, commit
//!         Ok(())
//!     });
//!
//! let msg = Message::with_string_payload("cmd-1", "order.create", "{}");
//! service.handle(&msg).unwrap();
//! ```

mod domain_service;
mod error;
#[cfg(feature = "http")]
mod http;
mod thread;

pub use domain_service::DomainService;
pub use error::HandlerError;
#[cfg(feature = "http")]
pub use http::{CommandRequest, CommandResponse};
pub use thread::{DomainServiceThread, ServiceStats};
