//! Context passed to handlers.
//!
//! Carries the message, parsed JSON payload when available, session variables,
//! and a reference to the service dependencies. Handlers access everything they
//! need through the context.

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::dependencies::{HasReadModelStore, HasRepo};
use super::error::HandlerError;
use super::transport::Message;
use super::session::Session;

/// The context passed to every handler.
///
/// Generic over `D` (the service dependency type) so handlers can access the
/// repository, read-model store, or custom dependencies the service is
/// configured with.
///
/// ## Example
///
/// ```ignore
/// pub fn handle<D: HasRepo>(
///     ctx: &Context<D>,
/// ) -> Result<Value, HandlerError> {
///     let user_id = ctx.user_id()?;
///     let input = ctx.input::<CreateOrderInput>()?;
///     let repo = ctx.repo();
/// }
/// ```
pub struct Context<'a, D> {
    /// Message being handled.
    message: Message,
    /// Raw JSON payload input, when the payload is JSON.
    input: Value,
    /// Session variables (user ID, role, etc.).
    session: Session,
    /// Reference to the service dependencies.
    dependencies: &'a D,
}

impl<'a, D> Context<'a, D> {
    /// Create a new context.
    pub(crate) fn new(
        message: Message,
        input: Value,
        session: Session,
        dependencies: &'a D,
    ) -> Self {
        Self {
            message,
            input,
            session,
            dependencies,
        }
    }

    /// Deserialize the input payload into a typed struct.
    pub fn input<T: DeserializeOwned>(&self) -> Result<T, HandlerError> {
        self.message.payload_json().map_err(HandlerError::from)
    }

    /// Get the raw JSON input.
    pub fn raw_input(&self) -> &Value {
        &self.input
    }

    /// Get the command name.
    pub fn command_name(&self) -> &str {
        self.message.name()
    }

    /// Get the message name.
    pub fn message_name(&self) -> &str {
        self.message.name()
    }

    /// Get the full message, including id, raw payload bytes, and metadata.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Get the session.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Get the user ID from the session. Returns `Unauthorized` if not present.
    pub fn user_id(&self) -> Result<&str, HandlerError> {
        self.session
            .user_id()
            .ok_or_else(|| HandlerError::Unauthorized("missing user ID in session".into()))
    }

    /// Get the user role from the session.
    pub fn role(&self) -> Option<&str> {
        self.session.role()
    }

    /// Get a reference to the service dependencies.
    pub fn dependencies(&self) -> &D {
        self.dependencies
    }

    /// Get the aggregate repository for handlers whose dependencies expose one.
    pub fn repo(&self) -> &D::Repo
    where
        D: HasRepo,
    {
        self.dependencies.repo()
    }

    /// Get the read-model store for handlers whose dependencies expose one.
    pub fn read_model_store(&self) -> &D::ReadModelStore
    where
        D: HasReadModelStore,
    {
        self.dependencies.read_model_store()
    }

    /// Check if the raw input contains a field.
    pub fn has_field(&self, field: &str) -> bool {
        self.input.get(field).is_some()
    }

    /// Check if the raw input contains all specified fields.
    pub fn has_fields(&self, fields: &[&str]) -> bool {
        fields.iter().all(|f| self.has_field(f))
    }
}
