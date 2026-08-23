//! Todo command mounts + eventual projector (SQL list dual-write).

use distributed::graphql::SurfaceProjector;
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, RepoReadModelDependencies, Routes,
};
use distributed::{AggregateBuilder, AggregateRepository, QueuedRepository};
use todo_domain::Todo;

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;

pub const MODULE_ID: &str = "todo";

type TodoRoutes<R, L, S> =
    Routes<RepoReadModelDependencies<AggregateRepository<QueuedRepository<R, L>, Todo>, S>>;

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
        .mount(todo_domain::commands::create())
        .mount(todo_domain::commands::rename())
        .mount(todo_domain::commands::complete())
        .mount(todo_domain::commands::reopen())
        .mount(todo_domain::commands::archive())
        .mount(todo_domain::commands::force_archive())
        .mount(todo_domain::commands::purge())
        .modeled_projector(todo_projector)
        .handle(handlers::project_todos::handle)
}
