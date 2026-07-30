//! Event handler that applies the Todo projector.
//!
//! `TODO_READS` is mutation-backed: program/resolve factories rewrite
//! SAVE_TODO / DELETE_TODO (see `e2e_projections::todos`).

use distributed::microsvc::{CausalProjectorContext, HandlerError, ModeledProjection};
use e2e_projections::TODO_READS;

pub async fn handle(
    context: CausalProjectorContext,
    projection: ModeledProjection,
) -> Result<(), HandlerError> {
    // Real path: ModeledProjection plan was resolved via mutation-backed
    // TODO_READS factories (SAVE_TODO / DELETE_TODO rewrite).
    projection.apply(TODO_READS, &context).await
}
