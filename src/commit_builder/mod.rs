//! CommitBuilder - Chain read models, outbox, and aggregates into one transactional commit batch.
//!
//! ## Example
//!
//! ```ignore
//! // All of these are equivalent - chain methods in any order:
//! repo
//!     .readmodel(&game_view)
//!     .outbox(message)
//!     .commit(&mut game)?;
//!
//! repo
//!     .outbox(message)
//!     .readmodel(&game_view)
//!     .commit(&mut game)?;
//! ```

use crate::aggregate::Aggregate;
use crate::entity::Entity;
use crate::outbox::OutboxMessage;
use crate::read_model::ReadModel;
use crate::repository::{CommitBatch, ReadModelWrite, RepositoryError, TransactionalCommit};

/// Builder for chaining multiple items into a single transactional commit batch.
pub struct CommitBuilder<'a, R> {
    repo: &'a R,
    entities: Vec<Entity>,
    models: Vec<ReadModelWrite>,
    error: Option<RepositoryError>,
}

impl<'a, R> CommitBuilder<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self {
            repo,
            entities: vec![],
            models: vec![],
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

        let key = format!("{}:{}", M::COLLECTION, model.id());
        match serde_json::to_vec(model) {
            Ok(bytes) => self.models.push(ReadModelWrite::new(key, bytes)),
            Err(err) => {
                self.error = Some(RepositoryError::Model(format!(
                    "failed to serialize read model {}: {}",
                    key, err
                )));
            }
        }
        self
    }

    /// Add an outbox message to the commit (takes ownership).
    pub fn outbox(mut self, msg: OutboxMessage) -> Self {
        self.entities.push(msg.into_entity());
        self
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
            read_models: self.models,
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
            read_models: self.models,
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
            read_models: self.models,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_model::ReadModelsExt;
    use crate::{impl_aggregate, Entity, EventRecord, Get, HashMapRepository};
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
                .read_models
                .iter()
                .map(|write| write.key.clone())
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
        let loaded = repo.read_models::<TestView>().get("1").unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().data.counter, 42);
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

        let loaded1 = repo.read_models::<TestView>().get("1").unwrap().unwrap();
        let loaded2 = repo.read_models::<TestView>().get("2").unwrap().unwrap();
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

        let loaded = repo.read_models::<TestView>().get("1").unwrap();
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

        let loaded = repo.read_models::<TestView>().get("1").unwrap();
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

        let loaded1 = repo
            .read_models::<TestView>()
            .get("standalone-1")
            .unwrap()
            .unwrap();
        let loaded2 = repo
            .read_models::<TestView>()
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
        let loaded = repo
            .read_models::<TestView>()
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
