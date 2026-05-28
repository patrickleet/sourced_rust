use serde::{Deserialize, Serialize};
use sourced_rust::{
    sourced, Entity, HashMapRepository, ReadModel, ReadModelWorkspaceExt,
    ReadModelWritePlanBuilder, RowKey, RowValue, SyncReadModelWritePlanCommitExt,
};

#[derive(Default)]
struct TestAggregate {
    entity: Entity,
}

#[sourced(entity)]
impl TestAggregate {
    #[event("Touched")]
    fn touch(&mut self) {
        if self.entity.id().is_empty() {
            self.entity.set_id("agg-1");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("bridge_views")]
struct BridgeView {
    #[id]
    id: String,
    value: i32,
}

#[test]
fn repo_first_read_models_session_commit_form_is_available() {
    let repo = HashMapRepository::new();
    let view = BridgeView {
        id: "view-1".into(),
        value: 42,
    };
    let mut session = ReadModelWritePlanBuilder::new();
    session.upsert(&view).unwrap();
    let mut aggregate = TestAggregate::default();
    aggregate.touch().unwrap();

    repo.read_models_sync(session)
        .commit_sync(&mut aggregate)
        .unwrap();

    let loaded = repo
        .workspace()
        .load::<BridgeView>(RowKey::new([("id", RowValue::String("view-1".into()))]))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(loaded.data, view);
}
