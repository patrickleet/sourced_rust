//! Context passed to command handlers.
//!
//! Carries the parsed input, session variables, and a reference to the
//! repository. Handlers access everything they need through the context.

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::error::HandlerError;
use super::session::Session;

/// The context passed to every command handler.
///
/// Generic over `R` (the repository type) so handlers can access
/// whatever repository implementation the service is configured with.
///
/// ## Example
///
/// ```ignore
/// pub fn handle<R: GetAggregate + CommitAggregate>(
///     ctx: &Context<R>,
/// ) -> Result<Value, HandlerError> {
///     let user_id = ctx.user_id()?;
///     let input = ctx.input::<CreateOrderInput>()?;
///     // ...
/// }
/// ```
pub struct Context<'a, R> {
    /// The command name being handled.
    command_name: String,
    /// Raw JSON input from the request.
    input: Value,
    /// Session variables (user ID, role, etc.).
    session: Session,
    /// Reference to the repository.
    repo: &'a R,
}

impl<'a, R> Context<'a, R> {
    /// Create a new context.
    pub(crate) fn new(command_name: String, input: Value, session: Session, repo: &'a R) -> Self {
        Self {
            command_name,
            input,
            session,
            repo,
        }
    }

    /// Deserialize the input payload into a typed struct.
    pub fn input<T: DeserializeOwned>(&self) -> Result<T, HandlerError> {
        serde_json::from_value(self.input.clone()).map_err(|e| HandlerError::DecodeFailed(e.to_string()))
    }

    /// Get the raw JSON input.
    pub fn raw_input(&self) -> &Value {
        &self.input
    }

    /// Get the command name.
    pub fn command_name(&self) -> &str {
        &self.command_name
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

    /// Get a reference to the repository.
    pub fn repo(&self) -> &R {
        self.repo
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
