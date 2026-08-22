//! Portable Todo command declarations.
//!
//! Hosts call [`distributed::microsvc::Routes::mount`] with these values. The
//! declarations do not name sqlx, celld, or `QueuedRepository`.

use distributed::command_input_defaults;
use distributed::graphql::{Eventual, PreparedCommand};
use distributed::microsvc::{
    CausalCommandContext, CausalRouteDependencies, HandlerError, PortableCommand, Routes,
};
use distributed::Aggregate;
use serde::{Deserialize, Serialize};

use crate::domain_commands;
use crate::{Todo, TodoState};

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

fn admin_user<A>(ctx: &CausalCommandContext<'_, A>) -> bool
where
    A: Aggregate + Send + Sync + 'static,
{
    authenticated_user(ctx) && ctx.session().has_role("admin")
}

/// Shared complete / archive / reopen payload.
#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoStatusPayload {
    pub todo_id: String,
    pub status: String,
}

impl TodoStatusPayload {
    fn from_todo(todo: &Todo) -> Self {
        let state = TodoState::from(todo);
        Self {
            todo_id: state.todo_id,
            status: state.status,
        }
    }
}

/// `todo.create`
pub struct Create;

pub fn create() -> Create {
    Create
}

impl<D> PortableCommand<D> for Create
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_create(routes)
    }
}

impl Create {
    pub const COMMAND: &'static str = "todo.create";

    pub fn shard(input: &TodoCreateInput) -> String {
        input.todo_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCreateInput {
    pub todo_id: String,
    pub title: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoCreatePayload {
    pub todo_id: String,
    pub owner_id: String,
    pub title: String,
    pub status: String,
}

pub async fn handle_create(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoCreateInput,
) -> Result<PreparedCommand<Eventual<TodoCreatePayload>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    if repo.get(&input.todo_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "todo {} already exists",
            input.todo_id
        )));
    }
    let mut todo = repo.create();
    todo.create(&input.todo_id, &owner, &input.title)
        .map_err(rejected)?;
    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoCreatePayload {
            todo_id: state.todo_id,
            owner_id: state.owner_id,
            title: state.title,
            status: state.status,
        })
}

fn install_create<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    routes
        .command_transition::<
            domain_commands::Create,
            TodoCreateInput,
            Eventual<TodoCreatePayload>,
        >(Create::COMMAND)
        .field_name("todos_create")
        .roles(["user", "admin"].into_iter())
        .input_defaults(command_input_defaults! {
            input: TodoCreateInput;
            default input.todo_id = uuid_v7();
        })
        .guarded(authenticated_user, handle_create)
}

/// `todo.rename`
pub struct Rename;

pub fn rename() -> Rename {
    Rename
}

impl<D> PortableCommand<D> for Rename
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_rename(routes)
    }
}

impl Rename {
    pub const COMMAND: &'static str = "todo.rename";

    pub fn shard(input: &TodoRenameInput) -> String {
        input.todo_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoRenameInput {
    pub todo_id: String,
    pub title: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoRenamePayload {
    pub todo_id: String,
    pub title: String,
    pub status: String,
}

pub async fn handle_rename(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoRenameInput,
) -> Result<PreparedCommand<Eventual<TodoRenamePayload>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.rename(&owner, &input.title).map_err(rejected)?;
    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoRenamePayload {
            todo_id: state.todo_id,
            title: state.title,
            status: state.status,
        })
}

fn install_rename<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    routes
        .command_transition::<
            domain_commands::Rename,
            TodoRenameInput,
            Eventual<TodoRenamePayload>,
        >(Rename::COMMAND)
        .field_name("todos_rename")
        .roles(["user", "admin"].into_iter())
        .guarded(authenticated_user, handle_rename)
}

/// `todo.complete`
pub struct Complete;

pub fn complete() -> Complete {
    Complete
}

impl<D> PortableCommand<D> for Complete
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_complete(routes)
    }
}

impl Complete {
    pub const COMMAND: &'static str = "todo.complete";

    pub fn shard(input: &TodoCompleteInput) -> String {
        input.todo_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCompleteInput {
    pub todo_id: String,
}

fn install_complete<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    routes
        .command_transition::<
            domain_commands::Complete,
            TodoCompleteInput,
            Eventual<TodoStatusPayload>,
        >(Complete::COMMAND)
        .field_name("todos_complete")
        .roles(["user", "admin"].into_iter())
        .load_by(|input: &TodoCompleteInput| Complete::shard(input))
        .invoke(|todo, _input, owner| todo.complete(owner))
        .eventual(|todo| TodoStatusPayload::from_todo(&**todo))
}

/// `todo.reopen`
pub struct Reopen;

pub fn reopen() -> Reopen {
    Reopen
}

impl<D> PortableCommand<D> for Reopen
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_reopen(routes)
    }
}

impl Reopen {
    pub const COMMAND: &'static str = "todo.reopen";

    pub fn shard(input: &TodoReopenInput) -> String {
        input.todo_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoReopenInput {
    pub todo_id: String,
}

pub type TodoReopenPayload = TodoStatusPayload;

pub async fn handle_reopen(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoReopenInput,
) -> Result<PreparedCommand<Eventual<TodoReopenPayload>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.reopen(&owner).map_err(rejected)?;
    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoReopenPayload {
            todo_id: state.todo_id,
            status: state.status,
        })
}

fn install_reopen<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    routes
        .command_transition::<
            domain_commands::Reopen,
            TodoReopenInput,
            Eventual<TodoReopenPayload>,
        >(Reopen::COMMAND)
        .field_name("todos_reopen")
        .roles(["user", "admin"].into_iter())
        .guarded(authenticated_user, handle_reopen)
}

/// `todo.archive`
pub struct Archive;

pub fn archive() -> Archive {
    Archive
}

impl<D> PortableCommand<D> for Archive
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_archive(routes)
    }
}

impl Archive {
    pub const COMMAND: &'static str = "todo.archive";

    pub fn shard(input: &TodoArchiveInput) -> String {
        input.todo_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoArchiveInput {
    pub todo_id: String,
}

pub type TodoArchivePayload = TodoStatusPayload;

pub async fn handle_archive(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoArchiveInput,
) -> Result<PreparedCommand<Eventual<TodoArchivePayload>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.archive(&owner).map_err(rejected)?;
    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoArchivePayload {
            todo_id: state.todo_id,
            status: state.status,
        })
}

fn install_archive<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    routes
        .command_transition::<
            domain_commands::Archive,
            TodoArchiveInput,
            Eventual<TodoArchivePayload>,
        >(Archive::COMMAND)
        .field_name("todos_archive")
        .roles(["user", "admin"].into_iter())
        .guarded(authenticated_user, handle_archive)
}

/// `todo.force_archive`
pub struct ForceArchive;

pub fn force_archive() -> ForceArchive {
    ForceArchive
}

impl<D> PortableCommand<D> for ForceArchive
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_force_archive(routes)
    }
}

impl ForceArchive {
    pub const COMMAND: &'static str = "todo.force_archive";

    pub fn shard(input: &TodoForceArchiveInput) -> String {
        input.todo_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoForceArchiveInput {
    pub todo_id: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoForceArchivePayload {
    pub todo_id: String,
    pub owner_id: String,
    pub status: String,
    pub archived_by: String,
}

pub async fn handle_force_archive(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoForceArchiveInput,
) -> Result<PreparedCommand<Eventual<TodoForceArchivePayload>>, HandlerError> {
    let admin = principal(ctx)?;
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.force_archive().map_err(rejected)?;
    let state = TodoState::from(&*todo);
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoForceArchivePayload {
            todo_id: state.todo_id,
            owner_id: state.owner_id,
            status: state.status,
            archived_by: admin,
        })
}

fn install_force_archive<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    routes
        .command_transition::<
            domain_commands::ForceArchive,
            TodoForceArchiveInput,
            Eventual<TodoForceArchivePayload>,
        >(ForceArchive::COMMAND)
        .field_name("todos_force_archive")
        .roles(["admin"])
        .guarded(admin_user, handle_force_archive)
}

/// `todo.purge`
pub struct Purge;

pub fn purge() -> Purge {
    Purge
}

impl<D> PortableCommand<D> for Purge
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_purge(routes)
    }
}

impl Purge {
    pub const COMMAND: &'static str = "todo.purge";

    pub fn shard(input: &TodoPurgeInput) -> String {
        input.todo_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoPurgeInput {
    pub todo_id: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct TodoPurgePayload {
    pub todo_id: String,
    pub purged: bool,
}

pub async fn handle_purge(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoPurgeInput,
) -> Result<PreparedCommand<Eventual<TodoPurgePayload>>, HandlerError> {
    let owner = principal(ctx)?;
    let repo = ctx.repo();
    let mut todo = repo
        .get(&input.todo_id)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.purge(&owner).map_err(rejected)?;
    repo.publish_events()
        .commit(todo)?
        .eventual(TodoPurgePayload {
            todo_id: input.todo_id,
            purged: true,
        })
}

fn install_purge<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = Todo> + Send + Sync + 'static,
{
    routes
        .command_transition::<domain_commands::Purge, TodoPurgeInput, Eventual<TodoPurgePayload>>(
            Purge::COMMAND,
        )
        .field_name("todos_purge")
        .roles(["user", "admin"].into_iter())
        .guarded(authenticated_user, handle_purge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use distributed::{AggregateBuilder, InMemoryRepository};

    fn mounted_specs() -> Vec<String> {
        let repository = InMemoryRepository::new();
        let routes = Routes::new()
            .with_repo(repository.aggregate::<Todo>())
            .mount(create())
            .mount(rename())
            .mount(complete())
            .mount(reopen())
            .mount(archive())
            .mount(force_archive())
            .mount(purge());
        routes
            .command_specs()
            .expect("todo command declarations compile")
            .into_iter()
            .map(|spec| spec.id)
            .collect()
    }

    #[test]
    fn complete_shard_is_todo_id() {
        let input = TodoCompleteInput {
            todo_id: "todo-1".into(),
        };
        assert_eq!(Complete::shard(&input), "todo-1");
    }

    #[test]
    fn create_handle_is_the_escape_hatch() {
        assert_eq!(Create::COMMAND, "todo.create");
        let _ = handle_create;
    }

    #[test]
    fn domain_declarations_mount_without_sqlx_or_celld() {
        let ids = mounted_specs();
        for command in [
            "todo.create",
            "todo.rename",
            "todo.complete",
            "todo.reopen",
            "todo.archive",
            "todo.force_archive",
            "todo.purge",
        ] {
            assert!(ids.iter().any(|id| id == command), "missing {command}");
        }
    }

    #[test]
    fn complete_is_thin_shard_invoke_eventual() {
        let ids = mounted_specs();
        assert!(ids.iter().any(|id| id == "todo.complete"));
        let complete_spec = Routes::new()
            .with_repo(InMemoryRepository::new().aggregate::<Todo>())
            .mount(complete())
            .command_specs()
            .expect("complete spec")
            .into_iter()
            .find(|spec| spec.id == "todo.complete")
            .expect("todo.complete");
        assert_eq!(complete_spec.field_name, "todos_complete");
    }
}
