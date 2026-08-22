//! Portable Blob command declarations.
//!
//! Shard is `game_id` so a later cell is `BlobGame:{game_id}`. Client preview
//! wasm (`blobSimulateMove`) stays in [`crate::wasm`].

use crate::{domain_commands, BlobGame, BlobGameState, Direction};
use distributed::graphql::{Atomic, CommandProjectionPureReduce, PreparedCommand};
use distributed::microsvc::{
    CausalCommandContext, CausalRouteDependencies, HandlerError, PortableCommand, Routes,
};
use distributed::{mutation_file, Aggregate, Mutation};
use e2e_readmodels::BlobGames;
use serde::Deserialize;

fn rejected(err: impl std::fmt::Display) -> HandlerError {
    HandlerError::Rejected(err.to_string())
}

fn principal<A>(ctx: &CausalCommandContext<'_, A>) -> Result<String, HandlerError>
where
    A: Aggregate + Send + Sync + 'static,
{
    ctx.user_id().map(str::to_string)
}

fn authenticated_user<A>(ctx: &CausalCommandContext<'_, A>) -> bool
where
    A: Aggregate + Send + Sync + 'static,
{
    ctx.session().user_id().is_some_and(|id| !id.is_empty())
}

#[allow(non_snake_case)]
fn SaveBlobGame() -> Mutation<()> {
    mutation_file!("src/mutations/save_blob_game.mutation.graphql")
}

fn sealed_row(game: &BlobGame) -> Result<BlobGames, HandlerError> {
    SaveBlobGame()
        .from_state(&BlobGameState::from(game))
        .map_err(|error| HandlerError::Other(Box::new(error)))
}

fn blob_preview() -> CommandProjectionPureReduce {
    CommandProjectionPureReduce::wasm(
        "blob.simulate_move",
        "blob/pkg/blob_wasm",
        "blobSimulateMove",
        "BlobGames",
    )
    .key_input("game_id", ["game_id"])
    .arg_input("direction", ["direction"])
    .assign([
        "map_json",
        "score",
        "player_dead",
        "current_level_completed",
        "status",
    ])
}

/// `blob.start`
pub struct Start;

pub fn start() -> Start {
    Start
}

impl<D> PortableCommand<D> for Start
where
    D: CausalRouteDependencies<Aggregate = BlobGame> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_start(routes)
    }
}

impl Start {
    pub const COMMAND: &'static str = "blob.start";

    pub fn shard(input: &BlobStartInput) -> String {
        input.game_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobStartInput {
    pub game_id: String,
}

pub async fn handle_start(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobStartInput,
) -> Result<PreparedCommand<Atomic<BlobGames>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    if repo.get(&input.game_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "game {} already exists",
            input.game_id
        )));
    }
    let mut game = repo.create();
    game.start_with_demo(&input.game_id, &owner)
        .map_err(rejected)?;
    let row = sealed_row(&*game)?;
    repo.readmodel(row).publish_events().commit(game)?.atomic()
}

fn install_start<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = BlobGame> + Send + Sync + 'static,
{
    routes
        .command_transition::<domain_commands::StartWithMap, BlobStartInput, Atomic<BlobGames>>(
            Start::COMMAND,
        )
        .field_name("blob_games_start")
        .roles(["user", "admin"].into_iter())
        .guarded(authenticated_user, handle_start)
}

/// `blob.move`
pub struct Move;

pub fn move_dir() -> Move {
    Move
}

impl<D> PortableCommand<D> for Move
where
    D: CausalRouteDependencies<Aggregate = BlobGame> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_move(routes)
    }
}

impl Move {
    pub const COMMAND: &'static str = "blob.move";

    pub fn shard(input: &BlobMoveInput) -> String {
        input.game_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobMoveInput {
    pub game_id: String,
    pub direction: String,
}

pub async fn handle_move(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobMoveInput,
) -> Result<PreparedCommand<Atomic<BlobGames>>, HandlerError> {
    let owner = principal(ctx)?;
    let dir = Direction::parse(&input.direction).ok_or_else(|| {
        HandlerError::Rejected(format!(
            "invalid direction `{}` (use up|down|left|right)",
            input.direction
        ))
    })?;
    let repo = ctx.repo();
    let mut game = repo
        .get(&input.game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.game_id.clone()))?;
    game.move_dir(&owner, dir).map_err(rejected)?;
    let row = sealed_row(&*game)?;
    repo.readmodel(row).publish_events().commit(game)?.atomic()
}

fn install_move<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = BlobGame> + Send + Sync + 'static,
{
    routes
        .command_transition::<domain_commands::MoveDir, BlobMoveInput, Atomic<BlobGames>>(
            Move::COMMAND,
        )
        .field_name("blob_games_move")
        .roles(["user", "admin"].into_iter())
        .preview_reduce_known_record(blob_preview())
        .guarded(authenticated_user, handle_move)
}

/// `blob.start_level`
pub struct StartLevel;

pub fn start_level() -> StartLevel {
    StartLevel
}

impl<D> PortableCommand<D> for StartLevel
where
    D: CausalRouteDependencies<Aggregate = BlobGame> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_start_level(routes)
    }
}

impl StartLevel {
    pub const COMMAND: &'static str = "blob.start_level";

    pub fn shard(input: &BlobStartLevelInput) -> String {
        input.game_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct BlobStartLevelInput {
    pub game_id: String,
}

pub async fn handle_start_level(
    ctx: &CausalCommandContext<'_, BlobGame>,
    input: BlobStartLevelInput,
) -> Result<PreparedCommand<Atomic<BlobGames>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    let mut game = repo
        .get(&input.game_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.game_id.clone()))?;
    game.start_next_generated_level(&owner).map_err(rejected)?;
    let row = sealed_row(&*game)?;
    repo.readmodel(row).publish_events().commit(game)?.atomic()
}

fn install_start_level<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = BlobGame> + Send + Sync + 'static,
{
    routes
        .command_transition::<domain_commands::StartLevel, BlobStartLevelInput, Atomic<BlobGames>>(
            StartLevel::COMMAND,
        )
        .field_name("blob_games_start_level")
        .roles(["user", "admin"].into_iter())
        .guarded(authenticated_user, handle_start_level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use distributed::{Aggregate, AggregateBuilder, InMemoryRepository};
    use std::path::Path;

    #[test]
    fn blob_shards_are_game_id() {
        let start = BlobStartInput {
            game_id: "g1".into(),
        };
        let mv = BlobMoveInput {
            game_id: "g1".into(),
            direction: "up".into(),
        };
        let level = BlobStartLevelInput {
            game_id: "g1".into(),
        };
        assert_eq!(Start::shard(&start), "g1");
        assert_eq!(Move::shard(&mv), "g1");
        assert_eq!(StartLevel::shard(&level), "g1");
    }

    #[test]
    fn blob_cell_is_parent_game_shard() {
        use distributed::cell_host::instance_name;
        let mv = BlobMoveInput {
            game_id: "g1".into(),
            direction: "up".into(),
        };
        let shard = Move::shard(&mv);
        assert_eq!(
            instance_name::<BlobGame>(&shard),
            format!("{}:{}", BlobGame::aggregate_type(), shard)
        );
        assert_eq!(
            instance_name::<BlobGame>(&shard),
            "blob:g1",
            "cell host addresses BlobGame as (aggregate_type, game_id)"
        );
    }

    #[test]
    fn atomic_blob_games_commands_mount_without_sqlx_or_celld() {
        let repository = InMemoryRepository::new();
        let specs = Routes::new()
            .with_repo(repository.aggregate::<BlobGame>())
            .mount(start())
            .mount(move_dir())
            .mount(start_level())
            .command_specs()
            .expect("blob command declarations compile");
        for command in ["blob.start", "blob.move", "blob.start_level"] {
            let spec = specs
                .iter()
                .find(|spec| spec.id == command)
                .unwrap_or_else(|| panic!("missing {command}"));
            let model = spec.projected_model.as_deref().unwrap_or("");
            assert!(
                model == "BlobGames" || model == "blob_games",
                "{command} should be Atomic<BlobGames>, got {model:?}"
            );
        }
    }

    #[test]
    fn client_preview_wasm_stays_in_blob_domain_wasm_module() {
        assert!(Path::new("src/wasm.rs").exists());
        let src = include_str!("wasm.rs");
        assert!(src.contains("blobSimulateMove"));
        assert!(src.contains("blob_simulate_move"));
    }
}
