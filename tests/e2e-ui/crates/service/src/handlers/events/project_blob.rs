//! Causally project `blob.*` facts → `blob_games`.

use blob_domain::BlobGameFact;
use distributed::microsvc::{CausalProjectorContext, HandlerError};
use e2e_readmodels::map_blob_fact;

pub const EVENTS: &[&str] = &[
    "blob.started",
    "blob.initialized",
    "blob.level_started",
    "blob.moved",
];

pub async fn handle(ctx: CausalProjectorContext, fact: BlobGameFact) -> Result<(), HandlerError> {
    let row = map_blob_fact(&fact);
    ctx.project(&row).await?;
    Ok(())
}
