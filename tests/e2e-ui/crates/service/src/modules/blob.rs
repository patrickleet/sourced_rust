//! Blob game module: Atomic command mounts (direct projection seal).

use blob_domain::domain_commands;
use blob_domain::BlobGame;
use distributed::graphql::{
    Atomic, CommandProjectionPureReduce, SurfaceDirectProjection,
};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, RepoReadModelDependencies, Routes,
};
use distributed::{AggregateBuilder, AggregateRepository, QueuedRepository};
use e2e_readmodels::BlobGames;

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers::commands::{blob_move, blob_start, blob_start_level};
use crate::handlers::util::causal_has_user;

/// Logical module id for composition inventories.
pub const MODULE_ID: &str = "blob";

type BlobRoutes<R, L, S> =
    Routes<RepoReadModelDependencies<AggregateRepository<QueuedRepository<R, L>, BlobGame>, S>>;

/// Mount blob Atomic commands.
///
/// Emit sets come from domain transitions that directly capture events:
/// - start → [`domain_commands::StartWithMap`] (`blob.started`; demo start uses this path)
/// - move → [`domain_commands::MoveDir`] (`blob.moved`)
/// - start_level → [`domain_commands::StartLevel`] (`blob.level_started`)
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
    Routes::for_aggregate::<R, L, BlobGame, S>(repo, locks, read_models)
        .command_transition::<
            domain_commands::StartWithMap,
            blob_start::BlobStartInput,
            Atomic<BlobGames>,
        >(blob_start::COMMAND)
        .field_name("blob_games_start")
        .roles(["user", "admin"].into_iter())
        .guarded(causal_has_user, blob_start::handle)
        .command_transition::<
            domain_commands::MoveDir,
            blob_move::BlobMoveInput,
            Atomic<BlobGames>,
        >(blob_move::COMMAND)
        .field_name("blob_games_move")
        .roles(["user", "admin"].into_iter())
        // Domain pure: blob_domain::core::simulate_move — client via WASM ($lib/blob/simulate-move).
        .preview_reduce_known_record(
            CommandProjectionPureReduce::new(
                "blob.simulate_move",
                "blob/simulate-move",
                "simulateMove",
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
            ]),
        )
        .guarded(causal_has_user, blob_move::handle)
        .command_transition::<
            domain_commands::StartLevel,
            blob_start_level::BlobStartLevelInput,
            Atomic<BlobGames>,
        >(blob_start_level::COMMAND)
        .field_name("blob_games_start_level")
        .roles(["user", "admin"].into_iter())
        .guarded(causal_has_user, blob_start_level::handle)
}
