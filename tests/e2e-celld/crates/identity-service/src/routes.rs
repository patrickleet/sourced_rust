//! Zitadel ingress/scrape commands + AuthUsers projector.

use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, RepoReadModelDependencies, Routes,
};
use distributed::{AggregateBuilder, AggregateRepository, QueuedRepository};

use crate::aggregate::Identity;
use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;

pub const MODULE_ID: &str = "identity";

type IdentityRoutes<R, L, S> =
    Routes<RepoReadModelDependencies<AggregateRepository<QueuedRepository<R, L>, Identity>, S>>;

pub fn routes<R, L, S>(repo: R, locks: L, read_models: S) -> IdentityRoutes<R, L, S>
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
    AggregateRepository<QueuedRepository<R, L>, Identity>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    Routes::for_aggregate::<R, L, Identity, S>(repo, locks, read_models)
        .command(handlers::ingestors::zitadel::COMMAND)
        .guarded(
            handlers::ingestors::zitadel::guard,
            handlers::ingestors::zitadel::handle,
        )
        .command(handlers::ingestors::zitadel_scrape::COMMAND)
        .guarded(
            handlers::ingestors::zitadel_scrape::guard,
            handlers::ingestors::zitadel_scrape::handle,
        )
        .events(handlers::events::project_auth_user::EVENTS)
        .guarded(
            handlers::events::project_auth_user::guard,
            handlers::events::project_auth_user::handle,
        )
}
