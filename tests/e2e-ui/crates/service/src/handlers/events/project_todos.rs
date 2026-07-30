//! Event handler that applies the shared Todo projection program.

use distributed::microsvc::{CausalProjectorContext, HandlerError, ModeledProjection};
use e2e_projections::TODO_READS;

pub async fn handle(
    context: CausalProjectorContext,
    projection: ModeledProjection,
) -> Result<(), HandlerError> {
    projection.apply(TODO_READS, &context).await
}
