//! Domain service for registering and dispatching command handlers.
//!
//! A `DomainService<R>` holds a repository and a map of command handlers.
//! Each handler is a closure that receives a reference to the repository
//! and a message, and returns `Result<(), HandlerError>`.
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::{DomainService, HashMapRepository};
//! use sourced_rust::bus::Message;
//!
//! let repo = HashMapRepository::new();
//! let service = DomainService::new(repo)
//!     .command("order.create", |repo, msg| {
//!         // decode payload, load/create aggregate, commit
//!         Ok(())
//!     })
//!     .command("order.cancel", |repo, msg| {
//!         Ok(())
//!     });
//!
//! let msg = Message::with_string_payload("cmd-1", "order.create", r#"{"id":"1"}"#);
//! service.handle(&msg).unwrap();
//! ```

use std::collections::HashMap;

use crate::bus::Message;
use super::error::HandlerError;

#[cfg(feature = "http")]
use super::http::{CommandRequest, CommandResponse};

/// A domain service that maps command names to handler closures.
///
/// The type parameter `R` is the repository type. Handlers receive `&R`
/// so they can load and commit aggregates.
pub struct DomainService<R> {
    repo: R,
    handlers: HashMap<String, Box<dyn Fn(&R, &Message) -> Result<(), HandlerError> + Send + Sync>>,
}

impl<R: Send + Sync + 'static> DomainService<R> {
    /// Create a new domain service with the given repository.
    pub fn new(repo: R) -> Self {
        Self {
            repo,
            handlers: HashMap::new(),
        }
    }

    /// Register a command handler for the given command name.
    ///
    /// Uses builder pattern — returns `self` for chaining.
    pub fn command<F>(mut self, name: &str, handler: F) -> Self
    where
        F: Fn(&R, &Message) -> Result<(), HandlerError> + Send + Sync + 'static,
    {
        self.handlers.insert(name.to_string(), Box::new(handler));
        self
    }

    /// Dispatch a single message to its registered handler.
    ///
    /// The message's `event_type` field is used as the command name to
    /// look up the handler.
    pub fn handle(&self, message: &Message) -> Result<(), HandlerError> {
        let handler = self
            .handlers
            .get(&message.event_type)
            .ok_or_else(|| HandlerError::UnknownCommand(message.event_type.clone()))?;
        handler(&self.repo, message)
    }

    /// List registered command names.
    pub fn commands(&self) -> Vec<&str> {
        self.handlers.keys().map(|s| s.as_str()).collect()
    }

    /// Get a reference to the underlying repository.
    pub fn repo(&self) -> &R {
        &self.repo
    }

    /// Dispatch an HTTP-style `CommandRequest`, returning a `CommandResponse`.
    ///
    /// Generates an ID if the request omits one, JSON-serializes the payload
    /// into a `Message`, calls `handle()`, and maps the result to a status code.
    #[cfg(feature = "http")]
    pub fn dispatch(&self, req: &CommandRequest) -> CommandResponse {
        let id = req
            .id
            .clone()
            .unwrap_or_else(|| {
                use std::time::{SystemTime, UNIX_EPOCH};
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                format!("cmd-{}", nanos)
            });

        let payload = match serde_json::to_vec(&req.payload) {
            Ok(bytes) => bytes,
            Err(e) => return HandlerError::DecodeFailed(e.to_string()).into(),
        };

        let msg = Message::new(id, &req.command, payload);

        match self.handle(&msg) {
            Ok(()) => CommandResponse::ok(),
            Err(e) => e.into(),
        }
    }
}
