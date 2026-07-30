//! Event handler that applies the shared Chat projection program.

use distributed::microsvc::{CausalProjectorContext, HandlerError, ModeledProjection};
use e2e_projections::CHAT_MESSAGES;

pub async fn handle(
    context: CausalProjectorContext,
    projection: ModeledProjection,
) -> Result<(), HandlerError> {
    projection.apply(CHAT_MESSAGES, &context).await
}
