//! Thin command path: load/create + domain method without a handler-context body.

use crate::microsvc::error::HandlerError;

/// Load an aggregate or return NotFound. Application code stays a domain call.
pub fn require_loaded<A>(loaded: Option<A>, id: impl Into<String>) -> Result<A, HandlerError> {
    loaded.ok_or_else(|| HandlerError::NotFound(id.into()))
}

/// Apply a domain transition and map domain errors to rejected commands.
pub fn invoke_transition<A, E>(
    aggregate: &mut A,
    transition: impl FnOnce(&mut A) -> Result<(), E>,
) -> Result<(), HandlerError>
where
    E: std::fmt::Display,
{
    transition(aggregate).map_err(|error| HandlerError::Rejected(error.to_string()))
}
