//! CommitBuilder - Chain read models, outbox, and aggregates for atomic commits.
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
use crate::read_model::{ReadModel, ReadModelStore};
use crate::outbox::OutboxMessage;
use crate::repository::{Commit, RepositoryError};

/// A queued read model save (type-erased).
struct QueuedModel {
    /// Storage key: "COLLECTION:id"
    key: String,
    /// JSON-serialized bytes
    bytes: Vec<u8>,
}

/// Builder for chaining multiple items into a single atomic commit.
pub struct CommitBuilder<'a, R> {
    repo: &'a R,
    entities: Vec<Entity>,
    models: Vec<QueuedModel>,
}

impl<'a, R> CommitBuilder<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self {
            repo,
            entities: vec![],
            models: vec![],
        }
    }

    /// Add a read model to the commit.
    pub fn readmodel<M: ReadModel>(mut self, model: &M) -> Self {
        let key = format!("{}:{}", M::COLLECTION, model.id());
        let bytes = serde_json::to_vec(model).expect("read model serialization should not fail");
        self.models.push(QueuedModel { key, bytes });
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
        R: Commit + ReadModelStore,
    {
        // Commit entities (outbox messages + aggregate)
        let mut entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
        entity_refs.push(aggregate.entity_mut());
        self.repo.commit(&mut entity_refs[..])?;

        // Write queued read models
        for queued in self.models {
            self.repo
                .upsert_raw(&queued.key, queued.bytes)?;
        }

        Ok(())
    }

    /// Commit multiple entities atomically (along with any queued read models and outbox).
    ///
    /// Use `entity_mut()` on each aggregate to get the entity references:
    /// ```ignore
    /// repo.readmodel(&view)
    ///     .commit_many(&mut [player.entity_mut(), monster.entity_mut()])?;
    /// ```
    pub fn commit_many(
        mut self,
        entities: &mut [&mut Entity],
    ) -> Result<(), RepositoryError>
    where
        R: Commit + ReadModelStore,
    {
        let mut entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
        for e in entities.iter_mut() {
            entity_refs.push(e);
        }
        self.repo.commit(&mut entity_refs[..])?;

        for queued in self.models {
            self.repo.upsert_raw(&queued.key, queued.bytes)?;
        }

        Ok(())
    }

    /// Commit without a primary aggregate.
    pub fn commit_all(mut self) -> Result<(), RepositoryError>
    where
        R: Commit + ReadModelStore,
    {
        // Commit entities if any
        if !self.entities.is_empty() {
            let mut entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
            self.repo.commit(&mut entity_refs[..])?;
        }

        // Write queued read models
        for queued in self.models {
            self.repo
                .upsert_raw(&queued.key, queued.bytes)?;
        }

        Ok(())
    }
}

/// Extension trait to start a commit builder chain from a read model or outbox.
pub trait CommitBuilderExt: Commit + ReadModelStore + Sized {
    /// Start a commit builder chain with a read model.
    fn readmodel<M: ReadModel>(&self, model: &M) -> CommitBuilder<'_, Self> {
        CommitBuilder::new(self).readmodel(model)
    }

    /// Start a commit builder chain with an outbox message.
    fn outbox(&self, msg: OutboxMessage) -> CommitBuilder<'_, Self> {
        CommitBuilder::new(self).outbox(msg)
    }
}

impl<R: Commit + ReadModelStore> CommitBuilderExt for R {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{impl_aggregate, Entity, EventRecord, Get, HashMapRepository};
    use crate::read_model::ReadModelsExt;
    use serde::{Deserialize, Serialize};

    #[derive(Default)]
    struct TestAggregate {
        entity: Entity,
    }

    impl TestAggregate {
        fn touch(&mut self) {
            if self.entity.id().is_empty() {
                self.entity.set_id("agg-1");
            }
            self.entity.digest_empty("Touched");
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

        let outbox = OutboxMessage::create("msg-1", "TestEvent", b"{}".to_vec());

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

        let outbox = OutboxMessage::create("msg-2", "TestEvent", b"{}".to_vec());

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

        let loaded1 = repo.read_models::<TestView>().get("standalone-1").unwrap().unwrap();
        let loaded2 = repo.read_models::<TestView>().get("standalone-2").unwrap().unwrap();
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
        let loaded = repo.read_models::<TestView>().get("multi").unwrap().unwrap();
        assert_eq!(loaded.data.counter, 77);

        // Verify both aggregates stored
        let e1 = repo.get("agg-1").unwrap();
        assert!(e1.is_some());
        let e2 = repo.get("agg-2").unwrap();
        assert!(e2.is_some());
    }
}
