//! Apply the Todos portable handlers for matching domain events.

use distributed::microsvc::{CausalProjectorContext, HandlerError, ModeledProjection};
use e2e_projections::TODOS;

pub async fn handle(
    context: CausalProjectorContext,
    projection: ModeledProjection,
) -> Result<(), HandlerError> {
    projection.apply(TODOS, &context).await
}
