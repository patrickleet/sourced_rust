//! Event handler that applies the shared Chat projection program.
//!
//! Public authoring contract is SAVE_CHAT_MESSAGE mutation IR.

use distributed::microsvc::{CausalProjectorContext, HandlerError, ModeledProjection};
use e2e_projections::{save_chat_message_program, CHAT_MESSAGES};

pub async fn handle(
    context: CausalProjectorContext,
    projection: ModeledProjection,
) -> Result<(), HandlerError> {
    let _mutation_authoring = save_chat_message_program().id().map_err(|error| {
        HandlerError::Other(Box::new(std::io::Error::other(error.to_string())))
    })?;
    projection.apply(CHAT_MESSAGES, &context).await
}
