//! CommitBuilder - chain read models, sessions, outbox, and aggregates into one transactional batch.
//!
//! ## Example
//!
//! ```ignore
//! // Document read model.
//! repo
//!     .readmodel(&game_view)
//!     .outbox(message)
//!     .commit(&mut game)?;
//!
//! // Relational/session read models.
//! let mut read_models = sourced_rust::ReadModelSession::new();
//! read_models.save(&player)?;
//! read_models.save_related(&player, "weapons", &weapon)?;
//!
//! repo
//!     .read_models(read_models)
//!     .commit(&mut game)?;
//!
//! // Ordering is semantic staging only.
//! repo
//!     .outbox(message)
//!     .read_models(read_models)
//!     .commit(&mut game)?;
//!
//! repo
//!     .aggregate(&mut game)
//!     .read_models(read_models)
//!     .outbox(message)
//!     .commit()?;
//! ```

use crate::aggregate::Aggregate;
use crate::entity::Entity;
use crate::outbox::OutboxMessage;
use crate::read_model::{ReadModel, ReadModelError, ReadModelSession, ReadModelWritePlan};
use crate::repository::{CommitBatch, RepositoryError, TransactionalCommit};

/// Builder for chaining multiple items into a single transactional commit batch.
pub struct CommitBuilder<'a, R> {
    repo: &'a R,
    entities: Vec<Entity>,
    read_model_plans: Vec<ReadModelWritePlan>,
    error: Option<RepositoryError>,
}

impl<'a, R> CommitBuilder<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self {
            repo,
            entities: vec![],
            read_model_plans: vec![],
            error: None,
        }
    }

    /// Add a read model to the commit.
    ///
    /// Serialization errors are returned by the final `commit*` call so the
    /// fluent builder API stays chainable without panicking.
    pub fn readmodel<M: ReadModel>(mut self, model: &M) -> Self {
        if self.error.is_some() {
            return self;
        }

        match document_plan(model) {
            Ok(plan) => self.read_model_plans.push(plan),
            Err(err) => self.error = Some(err),
        }
        self
    }

    /// Add a read-model session to the commit.
    pub fn read_models(mut self, session: ReadModelSession) -> Self {
        if self.error.is_some() {
            return self;
        }

        match session.into_write_plan() {
            Ok(plan) => self.read_model_plans.push(plan),
            Err(err) => self.error = Some(err.into()),
        }
        self
    }

    /// Add an outbox message to the commit (takes ownership).
    pub fn outbox(mut self, msg: OutboxMessage) -> Self {
        self.entities.push(msg.into_entity());
        self
    }

    /// Stage an aggregate and switch to a no-argument staged commit builder.
    pub fn aggregate<A: Aggregate>(self, aggregate: &'a mut A) -> StagedCommitBuilder<'a, R> {
        let mut builder = StagedCommitBuilder::from_builder(self);
        builder.staged_entities.push(aggregate.entity_mut());
        builder
    }

    /// Commit all items plus the primary aggregate.
    pub fn commit<A: Aggregate>(mut self, aggregate: &mut A) -> Result<(), RepositoryError>
    where
        R: TransactionalCommit,
    {
        self.check_staged()?;

        let mut entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
        entity_refs.push(aggregate.entity_mut());
        self.repo.commit_batch(CommitBatch {
            entities: entity_refs,
            read_model_plans: self.read_model_plans,
            snapshots: Vec::new(),
        })
    }

    /// Commit multiple entities in one batch (along with any queued read models and outbox).
    ///
    /// Use `entity_mut()` on each aggregate to get the entity references:
    /// ```ignore
    /// repo.readmodel(&view)
    ///     .commit_many(&mut [player.entity_mut(), monster.entity_mut()])?;
    /// ```
    pub fn commit_many(mut self, entities: &mut [&mut Entity]) -> Result<(), RepositoryError>
    where
        R: TransactionalCommit,
    {
        self.check_staged()?;

        let mut entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
        for e in entities.iter_mut() {
            entity_refs.push(&mut **e);
        }
        self.repo.commit_batch(CommitBatch {
            entities: entity_refs,
            read_model_plans: self.read_model_plans,
            snapshots: Vec::new(),
        })
    }

    /// Commit without a primary aggregate.
    pub fn commit_all(mut self) -> Result<(), RepositoryError>
    where
        R: TransactionalCommit,
    {
        self.check_staged()?;

        let entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
        self.repo.commit_batch(CommitBatch {
            entities: entity_refs,
            read_model_plans: self.read_model_plans,
            snapshots: Vec::new(),
        })
    }

    fn check_staged(&mut self) -> Result<(), RepositoryError> {
        if let Some(err) = self.error.take() {
            return Err(err);
        }
        Ok(())
    }
}

/// Builder returned after one or more aggregates are staged explicitly.
pub struct StagedCommitBuilder<'a, R> {
    repo: &'a R,
    entities: Vec<Entity>,
    staged_entities: Vec<&'a mut Entity>,
    read_model_plans: Vec<ReadModelWritePlan>,
    error: Option<RepositoryError>,
}

impl<'a, R> StagedCommitBuilder<'a, R> {
    fn from_builder(builder: CommitBuilder<'a, R>) -> Self {
        Self {
            repo: builder.repo,
            entities: builder.entities,
            staged_entities: Vec::new(),
            read_model_plans: builder.read_model_plans,
            error: builder.error,
        }
    }

    pub fn readmodel<M: ReadModel>(mut self, model: &M) -> Self {
        if self.error.is_some() {
            return self;
        }

        match document_plan(model) {
            Ok(plan) => self.read_model_plans.push(plan),
            Err(err) => self.error = Some(err),
        }
        self
    }

    pub fn read_models(mut self, session: ReadModelSession) -> Self {
        if self.error.is_some() {
            return self;
        }

        match session.into_write_plan() {
            Ok(plan) => self.read_model_plans.push(plan),
            Err(err) => self.error = Some(err.into()),
        }
        self
    }

    pub fn outbox(mut self, msg: OutboxMessage) -> Self {
        self.entities.push(msg.into_entity());
        self
    }

    pub fn aggregate<A: Aggregate>(mut self, aggregate: &'a mut A) -> Self {
        self.staged_entities.push(aggregate.entity_mut());
        self
    }

    pub fn entity(mut self, entity: &'a mut Entity) -> Self {
        self.staged_entities.push(entity);
        self
    }

    pub fn commit(mut self) -> Result<(), RepositoryError>
    where
        R: TransactionalCommit,
    {
        self.check_staged()?;

        let mut entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
        entity_refs.extend(self.staged_entities);
        self.repo.commit_batch(CommitBatch {
            entities: entity_refs,
            read_model_plans: self.read_model_plans,
            snapshots: Vec::new(),
        })
    }

    fn check_staged(&mut self) -> Result<(), RepositoryError> {
        if let Some(err) = self.error.take() {
            return Err(err);
        }
        Ok(())
    }
}

/// Extension trait to start a commit builder chain from a read model or outbox.
pub trait CommitBuilderExt: TransactionalCommit + Sized {
    /// Start a commit builder chain with a read model.
    fn readmodel<M: ReadModel>(&self, model: &M) -> CommitBuilder<'_, Self> {
        CommitBuilder::new(self).readmodel(model)
    }

    /// Start a commit builder chain with an outbox message.
    fn outbox(&self, msg: OutboxMessage) -> CommitBuilder<'_, Self> {
        CommitBuilder::new(self).outbox(msg)
    }
}

impl<R: TransactionalCommit> CommitBuilderExt for R {}

fn document_plan<M: ReadModel>(model: &M) -> Result<ReadModelWritePlan, RepositoryError> {
    let mut session = ReadModelSession::new();
    session.document(model).map_err(|err| match err {
        ReadModelError::Serde(message) => RepositoryError::Model(format!(
            "failed to serialize read model {}:{}: {}",
            M::COLLECTION,
            model.id(),
            message
        )),
        other => other.into(),
    })?;
    Ok(session.into_write_plan()?)
}

/// Extension trait for the new relational read-model session commit entrypoints.
///
/// Kept separate from `CommitBuilderExt` so the existing
/// `ReadModelsExt::read_models::<M>()` query accessor remains unambiguous unless
/// callers explicitly opt into the session starter.
pub trait ReadModelSessionCommitExt: TransactionalCommit + Sized {
    /// Start a commit builder chain with a relational read-model session.
    fn read_models(&self, session: ReadModelSession) -> CommitBuilder<'_, Self> {
        CommitBuilder::new(self).read_models(session)
    }

    /// Start a staged commit builder with an aggregate.
    fn aggregate<'a, A: Aggregate>(
        &'a self,
        aggregate: &'a mut A,
    ) -> StagedCommitBuilder<'a, Self> {
        CommitBuilder::new(self).aggregate(aggregate)
    }
}

impl<R: TransactionalCommit> ReadModelSessionCommitExt for R {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{Lock, LockManager};
    use crate::read_model::ReadModelsExt;
    use crate::{
        impl_aggregate, Entity, EventRecord, Get, HashMapRepository, QueuedReadModelStore,
    };
    use serde::{Deserialize, Serialize};
    use std::cell::RefCell;

    #[derive(Default)]
    struct TestAggregate {
        entity: Entity,
    }

    impl TestAggregate {
        fn touch(&mut self) {
            if self.entity.id().is_empty() {
                self.entity.set_id("agg-1");
            }
            self.entity.digest_empty("Touched").unwrap();
        }

        fn replay(&mut self, _event: &EventRecord) -> Result<(), String> {
            Ok(())
        }
    }

    impl_aggregate!(TestAggregate, entity, replay);

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
    struct TestView {
        id: String,
        counter: i32,
    }

    impl ReadModel for TestView {
        const COLLECTION: &'static str = "test_view";
        fn id(&self) -> &str {
            &self.id
        }
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, crate::ReadModel)]
    #[readmodel(table = "relational_views")]
    struct RelationalView {
        #[readmodel(id)]
        id: String,
        counter: i32,
    }

    #[derive(Deserialize, Clone)]
    struct FailingView {
        id: String,
    }

    impl Serialize for FailingView {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("injected serialize failure"))
        }
    }

    impl ReadModel for FailingView {
        const COLLECTION: &'static str = "failing_view";
        fn id(&self) -> &str {
            &self.id
        }
    }

    #[derive(Default)]
    struct RecordingBatchRepo {
        fail: bool,
        entity_ids: RefCell<Vec<String>>,
        read_model_keys: RefCell<Vec<String>>,
    }

    impl TransactionalCommit for RecordingBatchRepo {
        fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError> {
            *self.entity_ids.borrow_mut() = batch
                .entities
                .iter()
                .map(|entity| entity.id().to_string())
                .collect();
            *self.read_model_keys.borrow_mut() = batch
                .read_model_plans
                .iter()
                .flat_map(|plan| {
                    plan.mutations
                        .iter()
                        .map(|mutation| mutation.lock_key())
                        .collect::<Vec<_>>()
                })
                .collect();

            if self.fail {
                return Err(RepositoryError::Model("injected batch failure".into()));
            }

            for entity in batch.entities {
                entity.mark_committed();
            }
            Ok(())
        }
    }

    fn raw_session(view: &TestView) -> crate::read_model::ReadModelSession {
        let mut session = crate::read_model::ReadModelSession::new();
        session.document(view).unwrap();
        session
    }

    #[test]
    fn commit_readmodel_and_aggregate() {
        let repo = HashMapRepository::new();

        let view = TestView {
            id: "1".into(),
            counter: 42,
        };

        let mut agg = TestAggregate::default();
        agg.touch();

        repo.readmodel(&view).commit(&mut agg).unwrap();

        // Verify read model stored
        let loaded = ReadModelsExt::read_models::<TestView>(&repo)
            .get("1")
            .unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().data.counter, 42);
    }

    #[test]
    fn readmodel_commits_document_plan() {
        let repo = HashMapRepository::new();
        let view = TestView {
            id: "alias".into(),
            counter: 12,
        };
        let mut agg = TestAggregate::default();
        agg.touch();

        repo.readmodel(&view).commit(&mut agg).unwrap();

        let loaded = ReadModelsExt::read_models::<TestView>(&repo)
            .get("alias")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.data.counter, 12);
    }

    #[test]
    fn read_models_session_primary_command_side_form_commits_document_plan() {
        let repo = HashMapRepository::new();
        let view = TestView {
            id: "session".into(),
            counter: 13,
        };
        let mut agg = TestAggregate::default();
        agg.touch();

        ReadModelSessionCommitExt::read_models(&repo, raw_session(&view))
            .commit(&mut agg)
            .unwrap();

        let loaded = ReadModelsExt::read_models::<TestView>(&repo)
            .get("session")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.data.counter, 13);
        assert_eq!(agg.entity().committed_version(), 1);
    }

    #[test]
    fn commit_multiple_readmodels() {
        let repo = HashMapRepository::new();

        let view1 = TestView {
            id: "1".into(),
            counter: 10,
        };
        let view2 = TestView {
            id: "2".into(),
            counter: 20,
        };

        let mut agg = TestAggregate::default();
        agg.touch();

        repo.readmodel(&view1)
            .readmodel(&view2)
            .commit(&mut agg)
            .unwrap();

        let loaded1 = ReadModelsExt::read_models::<TestView>(&repo)
            .get("1")
            .unwrap()
            .unwrap();
        let loaded2 = ReadModelsExt::read_models::<TestView>(&repo)
            .get("2")
            .unwrap()
            .unwrap();
        assert_eq!(loaded1.data.counter, 10);
        assert_eq!(loaded2.data.counter, 20);
    }

    #[test]
    fn commit_readmodel_with_outbox() {
        let repo = HashMapRepository::new();

        let view = TestView {
            id: "1".into(),
            counter: 42,
        };

        let outbox = OutboxMessage::create("msg-1", "TestEvent", b"{}".to_vec()).unwrap();

        let mut agg = TestAggregate::default();
        agg.touch();

        // readmodel then outbox
        repo.readmodel(&view)
            .outbox(outbox)
            .commit(&mut agg)
            .unwrap();

        let loaded = ReadModelsExt::read_models::<TestView>(&repo)
            .get("1")
            .unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().data.counter, 42);
    }

    #[test]
    fn commit_outbox_then_readmodel() {
        let repo = HashMapRepository::new();

        let view = TestView {
            id: "1".into(),
            counter: 99,
        };

        let outbox = OutboxMessage::create("msg-2", "TestEvent", b"{}".to_vec()).unwrap();

        let mut agg = TestAggregate::default();
        agg.touch();

        // outbox then readmodel — same result
        repo.outbox(outbox)
            .readmodel(&view)
            .commit(&mut agg)
            .unwrap();

        let loaded = ReadModelsExt::read_models::<TestView>(&repo)
            .get("1")
            .unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().data.counter, 99);
    }

    #[test]
    fn commit_all_without_aggregate() {
        let repo = HashMapRepository::new();

        let view1 = TestView {
            id: "standalone-1".into(),
            counter: 1,
        };
        let view2 = TestView {
            id: "standalone-2".into(),
            counter: 2,
        };

        repo.readmodel(&view1)
            .readmodel(&view2)
            .commit_all()
            .unwrap();

        let loaded1 = ReadModelsExt::read_models::<TestView>(&repo)
            .get("standalone-1")
            .unwrap()
            .unwrap();
        let loaded2 = ReadModelsExt::read_models::<TestView>(&repo)
            .get("standalone-2")
            .unwrap()
            .unwrap();
        assert_eq!(loaded1.data.id, "standalone-1");
        assert_eq!(loaded2.data.id, "standalone-2");
    }

    #[test]
    fn commit_many_multiple_aggregates() {
        let repo = HashMapRepository::new();

        let view = TestView {
            id: "multi".into(),
            counter: 77,
        };

        let mut agg1 = TestAggregate::default();
        agg1.touch();
        agg1.entity.set_id("agg-1");

        let mut agg2 = TestAggregate::default();
        agg2.touch();
        agg2.entity.set_id("agg-2");

        repo.readmodel(&view)
            .commit_many(&mut [agg1.entity_mut(), agg2.entity_mut()])
            .unwrap();

        // Verify read model stored
        let loaded = ReadModelsExt::read_models::<TestView>(&repo)
            .get("multi")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.data.counter, 77);

        // Verify both aggregates stored
        let e1 = repo.get("agg-1").unwrap();
        assert!(e1.is_some());
        let e2 = repo.get("agg-2").unwrap();
        assert!(e2.is_some());
    }

    #[test]
    fn staged_builder_ordering_is_semantic_for_outbox_session_and_aggregate() {
        fn record(order: u8) -> (Vec<String>, Vec<String>) {
            let repo = RecordingBatchRepo::default();
            let view = TestView {
                id: "ordered".into(),
                counter: 7,
            };
            let outbox = OutboxMessage::create("ordered-msg", "TestEvent", b"{}".to_vec()).unwrap();
            let mut agg = TestAggregate::default();
            agg.touch();

            match order {
                0 => ReadModelSessionCommitExt::read_models(&repo, raw_session(&view))
                    .outbox(outbox)
                    .aggregate(&mut agg)
                    .commit()
                    .unwrap(),
                1 => repo
                    .outbox(outbox)
                    .read_models(raw_session(&view))
                    .aggregate(&mut agg)
                    .commit()
                    .unwrap(),
                _ => ReadModelSessionCommitExt::aggregate(&repo, &mut agg)
                    .read_models(raw_session(&view))
                    .outbox(outbox)
                    .commit()
                    .unwrap(),
            }

            let recorded = (
                repo.entity_ids.borrow().clone(),
                repo.read_model_keys.borrow().clone(),
            );
            recorded
        }

        let baseline = record(0);
        assert_eq!(record(1), baseline);
        assert_eq!(record(2), baseline);
    }

    #[test]
    fn staged_builder_supports_multiple_aggregates() {
        let repo = RecordingBatchRepo::default();
        let view = TestView {
            id: "staged-multi".into(),
            counter: 77,
        };
        let mut agg1 = TestAggregate::default();
        agg1.touch();
        agg1.entity.set_id("agg-1");
        let mut agg2 = TestAggregate::default();
        agg2.touch();
        agg2.entity.set_id("agg-2");

        ReadModelSessionCommitExt::read_models(&repo, raw_session(&view))
            .aggregate(&mut agg1)
            .aggregate(&mut agg2)
            .commit()
            .unwrap();

        assert_eq!(
            repo.read_model_keys.borrow().as_slice(),
            &["test_view:staged-multi".to_string()]
        );
        assert_eq!(
            repo.entity_ids.borrow().as_slice(),
            &["agg-1".to_string(), "agg-2".to_string()]
        );
    }

    #[test]
    fn queued_read_model_lock_is_released_after_session_commit() {
        let repo = QueuedReadModelStore::new(HashMapRepository::new());
        let view = TestView {
            id: "locked".into(),
            counter: 1,
        };
        ReadModelsExt::read_models::<TestView>(&repo)
            .upsert(&view)
            .unwrap();
        let _loaded = ReadModelsExt::read_models::<TestView>(&repo)
            .get("locked")
            .unwrap()
            .unwrap();

        let updated = TestView {
            id: "locked".into(),
            counter: 2,
        };
        let mut agg = TestAggregate::default();
        agg.touch();

        ReadModelSessionCommitExt::read_models(&repo, raw_session(&updated))
            .commit(&mut agg)
            .unwrap();

        let lock = repo.lock_manager().get_lock("test_view:locked").unwrap();
        assert!(lock.try_lock().unwrap());
        lock.unlock().unwrap();
    }

    #[test]
    fn invalid_session_plan_does_not_commit_aggregate() {
        let repo = RecordingBatchRepo::default();
        let mut session = crate::read_model::ReadModelSession::new();
        session.mark_processed("", "message-1");
        let mut agg = TestAggregate::default();
        agg.touch();

        let err = ReadModelSessionCommitExt::read_models(&repo, session)
            .commit(&mut agg)
            .unwrap_err();

        assert!(
            matches!(err, RepositoryError::Model(message) if message.contains("processed-message"))
        );
        assert_eq!(agg.entity().committed_version(), 0);
        assert!(repo.entity_ids.borrow().is_empty());
    }

    #[test]
    fn unsupported_relational_write_plan_does_not_commit_aggregate() {
        let repo = HashMapRepository::new();
        let view = RelationalView {
            id: "relational".into(),
            counter: 3,
        };
        let mut session = crate::read_model::ReadModelSession::new();
        session.save(&view).unwrap();
        let mut agg = TestAggregate::default();
        agg.touch();

        let err = ReadModelSessionCommitExt::read_models(&repo, session)
            .commit(&mut agg)
            .unwrap_err();

        assert!(
            matches!(err, RepositoryError::Model(message) if message.contains("relational row writes"))
        );
        assert_eq!(agg.entity().committed_version(), 0);
        assert!(repo.get("agg-1").unwrap().is_none());
    }

    #[test]
    fn commit_builder_failure_does_not_mark_aggregate_committed() {
        let repo = RecordingBatchRepo {
            fail: true,
            ..Default::default()
        };

        let view = TestView {
            id: "rollback".into(),
            counter: 1,
        };
        let outbox = OutboxMessage::create("msg-rollback", "TestEvent", b"{}".to_vec()).unwrap();
        let mut agg = TestAggregate::default();
        agg.touch();

        let err = repo
            .readmodel(&view)
            .outbox(outbox)
            .commit(&mut agg)
            .unwrap_err();

        assert_eq!(err, RepositoryError::Model("injected batch failure".into()));
        assert_eq!(agg.entity().committed_version(), 0);
        assert_eq!(agg.entity().new_events().len(), 1);
        assert_eq!(
            repo.read_model_keys.borrow().as_slice(),
            &["test_view:rollback".to_string()]
        );
        assert!(repo.entity_ids.borrow().iter().any(|id| id == "agg-1"));
        assert!(repo
            .entity_ids
            .borrow()
            .iter()
            .any(|id| id == "outbox:msg-rollback"));
    }

    #[test]
    fn readmodel_serialization_failure_returns_error_without_committing() {
        let repo = RecordingBatchRepo::default();
        let view = FailingView { id: "bad".into() };
        let mut agg = TestAggregate::default();
        agg.touch();

        let err = repo.readmodel(&view).commit(&mut agg).unwrap_err();

        assert!(matches!(
            err,
            RepositoryError::Model(ref message)
                if message.contains("failed to serialize read model failing_view:bad")
                    && message.contains("injected serialize failure")
        ));
        assert_eq!(agg.entity().committed_version(), 0);
        assert!(repo.entity_ids.borrow().is_empty());
        assert!(repo.read_model_keys.borrow().is_empty());
    }

    #[test]
    fn commit_builder_empty_batch_succeeds() {
        let repo = RecordingBatchRepo::default();

        CommitBuilder::new(&repo).commit_all().unwrap();

        assert!(repo.entity_ids.borrow().is_empty());
        assert!(repo.read_model_keys.borrow().is_empty());
    }
}
