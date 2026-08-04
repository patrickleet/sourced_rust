//! Todo bounded-context module: command mounts + eventual projector.

use distributed::graphql::{Eventual, SurfaceProjector};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, RepoReadModelDependencies, Routes,
};
use distributed::{
    command_input_defaults, AggregateBuilder, AggregateRepository, QueuedRepository,
};
use todo_domain::domain_commands;
use todo_domain::Todo;

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;
use crate::handlers::commands::{
    payloads, todo_archive, todo_complete, todo_create, todo_force_archive, todo_purge, todo_rename,
    todo_reopen,
};

/// Logical module id for composition inventories.
pub const MODULE_ID: &str = "todo";

type TodoRoutes<R, L, S> =
    Routes<RepoReadModelDependencies<AggregateRepository<QueuedRepository<R, L>, Todo>, S>>;

/// Mount todo commands and the todo projector.
pub fn routes<R, L, S>(
    repo: R,
    locks: L,
    read_models: S,
    todo_projector: SurfaceProjector,
) -> TodoRoutes<R, L, S>
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
    AggregateRepository<QueuedRepository<R, L>, Todo>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    Routes::for_aggregate::<R, L, Todo, S>(repo, locks, read_models)
        .command_transition::<
            domain_commands::Create,
            todo_create::TodoCreateInput,
            Eventual<todo_create::TodoCreatePayload>,
        >(todo_create::COMMAND)
        .field_name("todos_create")
        .roles(["user", "admin"].into_iter())
        .input_defaults(command_input_defaults! {
            input: todo_create::TodoCreateInput;
            default input.todo_id = uuid_v7();
        })
        .handle(todo_create::handle)
        .command_transition::<
            domain_commands::Rename,
            todo_rename::TodoRenameInput,
            Eventual<todo_rename::TodoRenamePayload>,
        >(todo_rename::COMMAND)
        .field_name("todos_rename")
        .roles(["user", "admin"].into_iter())
        .handle(todo_rename::handle)
        .command_transition::<
            domain_commands::Complete,
            todo_complete::TodoCompleteInput,
            Eventual<payloads::TodoStatusPayload>,
        >(todo_complete::COMMAND)
        .field_name("todos_complete")
        .roles(["user", "admin"].into_iter())
        .handle(todo_complete::handle)
        .command_transition::<
            domain_commands::Reopen,
            todo_reopen::TodoReopenInput,
            Eventual<todo_reopen::TodoReopenPayload>,
        >(todo_reopen::COMMAND)
        .field_name("todos_reopen")
        .roles(["user", "admin"].into_iter())
        .handle(todo_reopen::handle)
        .command_transition::<
            domain_commands::Archive,
            todo_archive::TodoArchiveInput,
            Eventual<todo_archive::TodoArchivePayload>,
        >(todo_archive::COMMAND)
        .field_name("todos_archive")
        .roles(["user", "admin"].into_iter())
        .handle(todo_archive::handle)
        .command_transition::<
            domain_commands::ForceArchive,
            todo_force_archive::TodoForceArchiveInput,
            Eventual<todo_force_archive::TodoForceArchivePayload>,
        >(todo_force_archive::COMMAND)
        .field_name("todos_force_archive")
        .roles(["admin"])
        .handle(todo_force_archive::handle)
        .command_transition::<
            domain_commands::Purge,
            todo_purge::TodoPurgeInput,
            Eventual<todo_purge::TodoPurgePayload>,
        >(todo_purge::COMMAND)
        .field_name("todos_purge")
        .roles(["user", "admin"].into_iter())
        .handle(todo_purge::handle)
        .modeled_projector(todo_projector)
        .handle(handlers::events::project_todos::handle)
}
