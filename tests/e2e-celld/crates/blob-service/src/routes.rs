//! Blob game module: Atomic command mounts (direct projection seal).

use blob_domain::BlobGame;
use distributed::graphql::SurfaceDirectProjection;
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, RepoReadModelDependencies, Routes,
};
use distributed::{AggregateBuilder, AggregateRepository, QueuedRepository};

use crate::bounds::{EventStore, Locks, ReadStore};

/// Logical module id for composition inventories.
pub const MODULE_ID: &str = "blob";

type BlobRoutes<R, L, S> =
    Routes<RepoReadModelDependencies<AggregateRepository<QueuedRepository<R, L>, BlobGame>, S>>;

/// Mount blob Atomic commands from blob-domain.
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
        .mount(blob_domain::commands::start())
        .mount(blob_domain::commands::move_dir())
        .mount(blob_domain::commands::start_level())
}
