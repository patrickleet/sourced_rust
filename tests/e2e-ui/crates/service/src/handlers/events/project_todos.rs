//! Event handler that applies the Todo projector.
//!
//! `TODOS` is mutation-backed: program/resolve factories rewrite
//! SAVE_TODO / DELETE_TODO (see `e2e_projections::todos`).

use distributed::microsvc::{CausalProjectorContext, HandlerError, ModeledProjection};
use e2e_projections::TODOS;

pub async fn handle(
    context: CausalProjectorContext,
    projection: ModeledProjection,
) -> Result<(), HandlerError> {
    // Real path: ModeledProjection plan was resolved via mutation-backed
    // TODOS factories (SAVE_TODO / DELETE_TODO rewrite).
    projection.apply(TODOS, &context).await
}
