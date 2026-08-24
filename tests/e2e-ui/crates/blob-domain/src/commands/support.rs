use distributed::microsvc::{CausalCommandContext, HandlerError};
use distributed::{mutation_file, Aggregate, Mutation};
use e2e_readmodels::BlobGames;

use crate::{BlobGame, BlobGameState};

pub(super) fn rejected(err: impl std::fmt::Display) -> HandlerError {
    HandlerError::Rejected(err.to_string())
}

pub(super) fn principal<A>(ctx: &CausalCommandContext<'_, A>) -> Result<String, HandlerError>
where
    A: Aggregate + Send + Sync + 'static,
{
    ctx.user_id().map(str::to_string)
}

pub(super) fn authenticated_user<A>(ctx: &CausalCommandContext<'_, A>) -> bool
where
    A: Aggregate + Send + Sync + 'static,
{
    ctx.session().user_id().is_some_and(|id| !id.is_empty())
}

#[allow(non_snake_case)]
fn SaveBlobGame() -> Mutation<()> {
    mutation_file!("src/mutations/save_blob_game.mutation.graphql")
}

pub(super) fn sealed_row(game: &BlobGame) -> Result<BlobGames, HandlerError> {
    SaveBlobGame()
        .from_state(&BlobGameState::from(game))
        .map_err(|error| HandlerError::Other(Box::new(error)))
}
