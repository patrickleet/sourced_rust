use super::{instance_name, AggregateCell, CellNamespace, CellStreamStore};
use crate::aggregate::{Aggregate, AggregateRepository};
use crate::entity::Entity;
use crate::graphql::{typed_command, PreparedCommand, Succeeded};
use crate::microsvc::service::{CausalCommandContext, PortableCommand, Routes};
use crate::microsvc::session::{Session, USER_ID_KEY};
use crate::microsvc::HandlerError;
use crate::repository::{RepositoryError, TransactionalCommit};
use crate::sourced;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::causal::{CausalWorkspace, CausalWorkspaceError};

#[derive(Clone, Default)]
struct CellItem {
    entity: Entity,
    title: String,
    done: bool,
}

#[sourced(entity, aggregate_type = "CellItem")]
impl CellItem {
    #[event("cell_item.created", version = 1)]
    fn create(&mut self, id: String, title: String) {
        self.entity.set_id(id);
        self.title = title;
        self.done = false;
    }

    #[event("cell_item.completed", version = 1)]
    fn complete(&mut self) {
        self.done = true;
    }
}

#[derive(Debug, Deserialize, crate::GraphqlInput)]
struct CreateInput {
    id: String,
    title: String,
}

#[derive(Debug, Serialize, crate::GraphqlOutput)]
struct CreatePayload {
    id: String,
}

#[derive(Debug, Deserialize, crate::GraphqlInput)]
struct CompleteInput {
    id: String,
}

#[derive(Debug, Serialize, crate::GraphqlOutput)]
struct CompletePayload {
    id: String,
    done: bool,
}

struct Create;

impl Create {
    const COMMAND: &'static str = "cell_item.create";
}

impl<D> PortableCommand<D> for Create
where
    D: crate::microsvc::CausalRouteDependencies<Aggregate = CellItem> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        routes
            .typed_command(typed_command::<CreateInput, Succeeded<CreatePayload>>(
                Self::COMMAND,
            ))
            .guarded(
                |ctx: &CausalCommandContext<'_, CellItem>| ctx.session().user_id().is_some(),
                handle_create,
            )
    }
}

struct Complete;

impl Complete {
    const COMMAND: &'static str = "cell_item.complete";

    fn shard(input: &CompleteInput) -> String {
        input.id.clone()
    }
}

impl<D> PortableCommand<D> for Complete
where
    D: crate::microsvc::CausalRouteDependencies<Aggregate = CellItem> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        routes
            .typed_command(typed_command::<CompleteInput, Succeeded<CompletePayload>>(
                Self::COMMAND,
            ))
            .load_by(|input: &CompleteInput| Complete::shard(input))
            .invoke(|item, _input, _owner| item.complete())
            .succeeded(|item| CompletePayload {
                id: item.entity().id().to_string(),
                done: item.done,
            })
    }
}

async fn handle_create(
    ctx: &CausalCommandContext<'_, CellItem>,
    input: CreateInput,
) -> Result<PreparedCommand<Succeeded<CreatePayload>>, HandlerError> {
    let repo = ctx.repo();
    if repo.get(&input.id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "cell item {} already exists",
            input.id
        )));
    }
    let mut item = repo.create();
    item.create(input.id.clone(), input.title)
        .map_err(|error| HandlerError::Rejected(error.to_string()))?;
    repo.commit(item)?.succeeded(CreatePayload { id: input.id })
}

fn owner_session() -> Session {
    let mut session = Session::new();
    session.set(USER_ID_KEY, "user-1");
    session
}

fn fn_send_sync<T: Send + Sync>(_: &T) {}

#[tokio::test]
async fn workspace_adapter_loads_and_commits_one_stream_without_sqlx() {
    let store = CellStreamStore::new("CellItem", "item-1").expect("identity");
    let repository = AggregateRepository::<_, CellItem>::new(store.clone());
    let workspace = CausalWorkspace::new(&repository);

    let mut item = workspace.create();
    item.create("item-1".into(), "write".into()).unwrap();
    workspace.stage(item).unwrap();

    let mut parts = workspace.into_parts().unwrap();
    parts.prepare_domain_publications("causation-1").unwrap();
    let batch = parts.prepare_commit_batch().unwrap();
    TransactionalCommit::commit_batch(&store, batch)
        .await
        .unwrap();

    let repository = AggregateRepository::<_, CellItem>::new(store.clone());
    let workspace = CausalWorkspace::new(&repository);
    let loaded = workspace.load("item-1").await.unwrap().unwrap();
    assert_eq!(loaded.entity().id(), "item-1");
    assert_eq!(loaded.title, "write");

    match workspace.load("item-2").await {
        Err(CausalWorkspaceError::Repository(RepositoryError::Model(message))) => {
            assert!(
                message.contains("cannot access stream"),
                "unexpected message: {message}"
            );
        }
        other => panic!(
            "expected shard fence, got {}",
            match other {
                Ok(_) => "Ok(checkout)".to_string(),
                Err(error) => error.to_string(),
            }
        ),
    }
}

#[tokio::test]
async fn cell_rejects_commit_of_a_foreign_stream() {
    let store = CellStreamStore::new("CellItem", "item-1").expect("identity");
    let repository = AggregateRepository::<_, CellItem>::new(store.clone());
    let workspace = CausalWorkspace::new(&repository);
    let mut item = workspace.create();
    item.create("item-2".into(), "other".into()).unwrap();
    workspace.stage(item).unwrap();
    let mut parts = workspace.into_parts().unwrap();
    parts.prepare_domain_publications("causation-1").unwrap();
    let batch = parts.prepare_commit_batch().unwrap();
    let error = TransactionalCommit::commit_batch(&store, batch)
        .await
        .unwrap_err();
    assert!(
        matches!(error, RepositoryError::Model(message) if message.contains("cannot access stream"))
    );
}

#[tokio::test]
async fn cell_dispatches_complete_with_the_same_portable_command_as_soa() {
    let cell = AggregateCell::<CellItem>::new("item-1")
        .unwrap()
        .mount(Create)
        .mount(Complete);
    assert_eq!(cell.instance_name(), "CellItem:item-1");
    assert_eq!(instance_name::<CellItem>("item-1"), "CellItem:item-1");
    let names = cell.command_names();
    assert!(names.iter().any(|name| name == "cell_item.create"));
    assert!(names.iter().any(|name| name == "cell_item.complete"));
    assert!(cell.is_command_only());
    fn_send_sync(&cell);

    let created = cell
        .dispatch(
            "cell_item.create",
            json!({ "id": "item-1", "title": "ship" }),
            owner_session(),
        )
        .await
        .expect("create");
    assert_eq!(created["id"], "item-1");

    let completed = cell
        .dispatch(
            "cell_item.complete",
            json!({ "id": "item-1" }),
            owner_session(),
        )
        .await
        .expect("complete");
    assert_eq!(completed["id"], "item-1");
    assert_eq!(completed["done"], true);
}

#[tokio::test]
async fn cell_complete_rejects_a_different_shard_id() {
    let cell = AggregateCell::<CellItem>::new("item-1")
        .unwrap()
        .mount(Create)
        .mount(Complete);
    cell.dispatch(
        "cell_item.create",
        json!({ "id": "item-1", "title": "ship" }),
        owner_session(),
    )
    .await
    .unwrap();

    let error = cell
        .dispatch(
            "cell_item.complete",
            json!({ "id": "item-2" }),
            owner_session(),
        )
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("cannot access stream") || message.contains("not found"),
        "unexpected error: {message}"
    );
}

#[tokio::test]
async fn namespace_get_by_name_addresses_type_and_shard() {
    let mut namespace = CellNamespace::<CellItem>::new();
    namespace
        .get_or_create("item-7", |cell| cell.mount(Create).mount(Complete))
        .unwrap();
    let cell = namespace
        .get_by_name("CellItem:item-7")
        .expect("named cell");
    assert_eq!(cell.shard_id(), "item-7");
    assert!(namespace.get_by_name("CellItem:missing").is_none());
}

#[test]
fn cargo_features_keep_sqlite_and_do_not_add_celld() {
    let manifest = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    assert!(
        manifest
            .lines()
            .any(|line| line.trim_start().starts_with("sqlite =")),
        "sqlite feature must remain next to postgres"
    );
    assert!(
        manifest
            .lines()
            .any(|line| line.trim_start().starts_with("postgres =")),
        "postgres feature must remain next to sqlite"
    );
    let features = manifest
        .split("[features]")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("features table");
    assert!(
        !features
            .lines()
            .any(|line| line.trim_start().starts_with("celld")),
        "PCH-DEC-005: do not add a celld Cargo feature beside sqlite/postgres"
    );
}
