//! Chat + identity-ingestor module: room messages, Zitadel ingress, auth_user projector.

use chat_domain::ChatMessage;
use distributed::graphql::SurfaceProjector;
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, RepoReadModelDependencies, Routes,
};
use distributed::{AggregateBuilder, AggregateRepository, QueuedRepository};

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;

/// Logical module id for composition inventories.
pub const MODULE_ID: &str = "chat";

type ChatRoutes<R, L, S> =
    Routes<RepoReadModelDependencies<AggregateRepository<QueuedRepository<R, L>, ChatMessage>, S>>;

/// Mount chat commands, Zitadel extension commands, and chat/auth projectors.
pub fn routes<R, L, S>(
    repo: R,
    locks: L,
    read_models: S,
    chat_projector: SurfaceProjector,
) -> ChatRoutes<R, L, S>
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
    AggregateRepository<QueuedRepository<R, L>, ChatMessage>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    Routes::for_aggregate::<R, L, ChatMessage, S>(repo, locks, read_models)
        .mount(chat_domain::commands::post())
        // Zitadel Action ingress + on-demand scrape remain non-GraphQL
        // integration commands (explicit extension mounts).
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
        .modeled_projector(chat_projector)
        .handle(handlers::events::project_chat_messages::handle)
        .events(handlers::events::project_auth_user::EVENTS)
        .guarded(
            handlers::events::project_auth_user::guard,
            handlers::events::project_auth_user::handle,
        )
}
