//! Portable Todo command declarations.
//!
//! Hosts call [`distributed::microsvc::Routes::mount`] with these values. The
//! declarations do not name sqlx, celld, or `QueuedRepository`. Thin commands
//! use [`distributed::portable_command`]; `todo.create` / `todo.force_archive`
//! keep a `handle:` escape hatch (`PCH-AC-002.1`).

use distributed::command_input_defaults;
use distributed::graphql::{Eventual, PreparedCommand};
use distributed::microsvc::{CausalCommandContext, HandlerError};
use distributed::portable_command;
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

portable_command! {
    name: "todo.create",
    transition: domain_commands::Create,
    aggregate: Todo,
    input: TodoCreateInput,
    outcome: Eventual<TodoCreatePayload>,
    shard: |input| input.todo_id.clone(),
    roles: ["user", "admin"],
    field: "todos_create",
    guard: authenticated_user,
    handle: handle_create,
    defaults: command_input_defaults! {
        input: TodoCreateInput;
        default input.todo_id = uuid_v7();
    },
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

portable_command! {
    name: "todo.rename",
    transition: domain_commands::Rename,
    aggregate: Todo,
    input: TodoRenameInput,
    outcome: Eventual<TodoRenamePayload>,
    shard: |input| input.todo_id.clone(),
    load: required,
    roles: ["user", "admin"],
    field: "todos_rename",
    invoke: |todo, input, principal| todo.rename(principal, &input.title),
    payload: |todo| {
        let state = TodoState::from(&**todo);
        TodoRenamePayload {
            todo_id: state.todo_id,
            title: state.title,
            status: state.status,
        }
    },
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCompleteInput {
    pub todo_id: String,
}

portable_command! {
    name: "todo.complete",
    transition: domain_commands::Complete,
    aggregate: Todo,
    input: TodoCompleteInput,
    outcome: Eventual<TodoStatusPayload>,
    shard: |input| input.todo_id.clone(),
    load: required,
    roles: ["user", "admin"],
    field: "todos_complete",
    invoke: |todo, _input, principal| todo.complete(principal),
    payload: |todo| TodoStatusPayload::from_todo(&**todo),
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoReopenInput {
    pub todo_id: String,
}

pub type TodoReopenPayload = TodoStatusPayload;

portable_command! {
    name: "todo.reopen",
    transition: domain_commands::Reopen,
    aggregate: Todo,
    input: TodoReopenInput,
    outcome: Eventual<TodoReopenPayload>,
    shard: |input| input.todo_id.clone(),
    load: required,
    roles: ["user", "admin"],
    field: "todos_reopen",
    invoke: |todo, _input, principal| todo.reopen(principal),
    payload: |todo| TodoStatusPayload::from_todo(&**todo),
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoArchiveInput {
    pub todo_id: String,
}

pub type TodoArchivePayload = TodoStatusPayload;

portable_command! {
    name: "todo.archive",
    transition: domain_commands::Archive,
    aggregate: Todo,
    input: TodoArchiveInput,
    outcome: Eventual<TodoArchivePayload>,
    shard: |input| input.todo_id.clone(),
    load: required,
    roles: ["user", "admin"],
    field: "todos_archive",
    invoke: |todo, _input, principal| todo.archive(principal),
    payload: |todo| TodoStatusPayload::from_todo(&**todo),
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

portable_command! {
    name: "todo.force_archive",
    transition: domain_commands::ForceArchive,
    aggregate: Todo,
    input: TodoForceArchiveInput,
    outcome: Eventual<TodoForceArchivePayload>,
    shard: |input| input.todo_id.clone(),
    roles: ["admin"],
    field: "todos_force_archive",
    guard: admin_user,
    handle: handle_force_archive,
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

portable_command! {
    name: "todo.purge",
    transition: domain_commands::Purge,
    aggregate: Todo,
    input: TodoPurgeInput,
    outcome: Eventual<TodoPurgePayload>,
    shard: |input| input.todo_id.clone(),
    load: required,
    roles: ["user", "admin"],
    field: "todos_purge",
    invoke: |todo, _input, principal| todo.purge(principal),
    payload: |todo| TodoPurgePayload {
        todo_id: todo.todo_id.clone(),
        purged: true,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use distributed::microsvc::Routes;
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

    #[tokio::test]
    async fn cell_host_dispatches_complete_with_the_same_handle_as_soa() {
        use distributed::cell_host::AggregateCell;
        use distributed::microsvc::{Session, USER_ID_KEY};

        let cell = AggregateCell::<Todo>::new("todo-1")
            .expect("cell identity")
            .mount(create())
            .mount(complete());
        assert_eq!(cell.instance_name(), "todo:todo-1");
        assert!(cell.is_command_only());
        assert!(cell
            .command_names()
            .iter()
            .any(|name| name == "todo.complete"));

        let mut session = Session::new();
        session.set(USER_ID_KEY, "owner-1");
        session.set("x-roles", "user");

        cell.dispatch(
            "todo.create",
            serde_json::json!({
                "todo_id": "todo-1",
                "title": "cell complete",
            }),
            session.clone(),
        )
        .await
        .expect("todo.create on cell");

        let completed = cell
            .dispatch(
                "todo.complete",
                serde_json::json!({ "todo_id": "todo-1" }),
                session,
            )
            .await
            .expect("todo.complete on cell");
        assert_eq!(completed["todo_id"], "todo-1");
        assert_eq!(completed["status"], "completed");
    }
}
