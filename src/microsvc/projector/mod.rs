//! Typed, capability-restricted causal projector routes.
//!
//! A projector handler receives typed input and a [`CausalProjectorContext`].
//! It can load/stage projected rows, but it never receives the dependency
//! bundle, repository, trusted transport cursor, or commit/failure methods.
//! The framework authenticates ordering through the receive adapter, bootstraps
//! the complete compiled topology, and owns the atomic protocol commit.

mod context;
mod errors;
mod handle;
mod registration;
mod runtime;

pub use context::{CausalProjectorContext, LoadedProjection};
pub use handle::{ProjectionRepairHandle, ProjectionRepairHandleParseError};
pub use registration::CausalProjectorRouteBuilder;

pub(super) use errors::projection_error_is_retryable;
pub(super) use runtime::{
    ErasedProjectorHandler, ProjectorRegistration, ProjectorRepairFuture,
    ProjectorRepairLookupFuture,
};

#[cfg(test)]
mod tests;
