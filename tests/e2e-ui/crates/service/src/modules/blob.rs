//! Blob game module: Atomic command mounts (direct projection seal).

use blob_domain::{
    BlobGame, BlobLevelStartedDomainEvent, BlobMovedDomainEvent, BlobStartedDomainEvent,
};
use distributed::graphql::{typed_command, Atomic, SurfaceDirectProjection};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, RepoReadModelDependencies, Routes,
};
use distributed::{AggregateBuilder, AggregateRepository, Queueable, QueuedRepository};
use e2e_readmodels::BlobGames;

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers::commands::{blob_move, blob_start, blob_start_level};

/// Logical module id for composition inventories.
pub const MODULE_ID: &str = "blob";

type BlobRoutes<R, L, S> =
    Routes<RepoReadModelDependencies<AggregateRepository<QueuedRepository<R, L>, BlobGame>, S>>;

/// Mount blob Atomic commands.
pub fn routes<R, L, S>(
    repo: R,
    locks: L,
    read_models: S,
    _blob_direct: SurfaceDirectProjection,
) -> BlobRoutes<R, L, S>
where
    R: EventStore,
    L: Locks,
    S: ReadStore,
    QueuedRepository<R, L>: Clone
        + AggregateBuilder
        + HasOutboxStore
        + distributed::TransactionalCommit
        + Send
        + Sync
        + 'static,
    AggregateRepository<QueuedRepository<R, L>, BlobGame>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    let _ = _blob_direct;
    Routes::new()
        .with_repo(repo.queued_with(locks).aggregate::<BlobGame>())
        .with_read_model_store(read_models)
        .typed_command(
            typed_command::<blob_start::BlobStartInput, Atomic<BlobGames>>(blob_start::COMMAND)
                .field_name("blob_games_start")
                .roles(["user", "admin"].into_iter())
                .emits(distributed::events![BlobStartedDomainEvent])
                .applies(distributed::state_preview! {
                    BlobStartedDomainEvent => blob_domain::BlobGameState {
                        game_id: input.game_id,
                        owner_id: trusted("x-user-id", "string"),
                        score: 0,
                        player_dead: unknown,
                        current_level: 1,
                        current_level_completed: unknown,
                        map_json: "[]",
                        status: "active",
                    }
                }),
        )
        .handle(blob_start::handle)
        .typed_command(
            typed_command::<blob_move::BlobMoveInput, Atomic<BlobGames>>(blob_move::COMMAND)
                .field_name("blob_games_move")
                .roles(["user", "admin"].into_iter())
                .emits(distributed::events![BlobMovedDomainEvent])
                .applies(distributed::state_preview! {
                    BlobMovedDomainEvent => blob_domain::BlobGameState {
                        game_id: input.game_id,
                        owner_id: trusted("x-user-id", "string"),
                        score: input.score,
                        player_dead: input.player_dead,
                        current_level: input.current_level,
                        current_level_completed: input.current_level_completed,
                        map_json: input.map_json,
                        status: input.status,
                    }
                }),
        )
        .handle(blob_move::handle)
        .typed_command(
            typed_command::<blob_start_level::BlobStartLevelInput, Atomic<BlobGames>>(
                blob_start_level::COMMAND,
            )
            .field_name("blob_games_start_level")
            .roles(["user", "admin"].into_iter())
            .emits(distributed::events![BlobLevelStartedDomainEvent])
            .applies(distributed::state_preview! {
                BlobLevelStartedDomainEvent => blob_domain::BlobGameState {
                    game_id: input.game_id,
                    owner_id: trusted("x-user-id", "string"),
                    score: unknown,
                    player_dead: unknown,
                    current_level: unknown,
                    current_level_completed: unknown,
                    map_json: "[]",
                    status: "active",
                }
            }),
        )
        .handle(blob_start_level::handle)
}
