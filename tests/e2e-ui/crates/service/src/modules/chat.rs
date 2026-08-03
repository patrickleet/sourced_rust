//! Chat + identity-ingestor module: room messages, Zitadel ingress, auth_user projector.

use chat_domain::{ChatMessage, ChatMessagePostedDomainEvent};
use distributed::graphql::{typed_command, Eventual, SurfaceProjector};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, RepoReadModelDependencies, Routes,
};
use distributed::{AggregateBuilder, AggregateRepository, Queueable, QueuedRepository};

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;
use crate::handlers::commands::chat_post;

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
    Routes::new()
        .with_repo(repo.queued_with(locks).aggregate::<ChatMessage>())
        .with_read_model_store(read_models)
        .typed_command(
            typed_command::<chat_post::ChatPostInput, Eventual<chat_post::ChatPostPayload>>(
                chat_post::COMMAND,
            )
            .field_name("chat_messages_post")
            .roles(["user", "admin"].into_iter())
            .emits(distributed::events![ChatMessagePostedDomainEvent])
            .applies(distributed::state_preview! {
                ChatMessagePostedDomainEvent => chat_domain::ChatMessageState {
                    message_id: input.message_id,
                    room_id: input.room_id,
                    author_id: trusted("x-user-id", "string"),
                    body: input.body,
                    created_at: input.created_at,
                }
            }),
        )
        .handle(chat_post::handle)
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
