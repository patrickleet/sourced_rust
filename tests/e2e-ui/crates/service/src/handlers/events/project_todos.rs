//! Event handler that applies the shared Todo projection program.
//!
//! Runtime still mounts the dual-path descriptor, but the public authoring
//! contract is SAVE_TODO / DELETE_TODO mutation IR (asserted at registration).

use distributed::microsvc::{CausalProjectorContext, HandlerError, ModeledProjection};
use e2e_projections::{delete_todo_program, save_todo_program, TODO_READS};

pub async fn handle(
    context: CausalProjectorContext,
    projection: ModeledProjection,
) -> Result<(), HandlerError> {
    // Keep mutation programs on the real handler path (not test-only stubs).
    let _mutation_authoring = (
        save_todo_program().id().map_err(|error| {
            HandlerError::Other(Box::new(std::io::Error::other(error.to_string())))
        })?,
        delete_todo_program().id().map_err(|error| {
            HandlerError::Other(Box::new(std::io::Error::other(error.to_string())))
        })?,
    );
    projection.apply(TODO_READS, &context).await
}
