//! CommitBuilder - chain read models, write plans, outbox, and aggregates into one transactional batch.
//!
//! ## Example
//!
//! ```ignore
//! let mut read_models = sourced_rust::ReadModelWritePlanBuilder::new();
//! read_models.upsert(&player)?;
//! read_models.upsert_related(&player, "weapons", &weapon)?;
//!
//! repo
//!     .read_models(read_models)
//!     .commit(&mut game)?;
//!
//! // Ordering is semantic staging only.
//! let mut read_models = sourced_rust::ReadModelWritePlanBuilder::new();
//! read_models.upsert(&player)?;
//! read_models.upsert_related(&player, "weapons", &weapon)?;
//!
//! repo
//!     .outbox(message)
//!     .read_models(read_models)
//!     .commit(&mut game)?;
//!
//! let mut read_models = sourced_rust::ReadModelWritePlanBuilder::new();
//! read_models.upsert(&player)?;
//! read_models.upsert_related(&player, "weapons", &weapon)?;
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
use crate::read_model::{ReadModelWritePlan, ReadModelWritePlanBuilder};
use crate::repository::{CommitBatch, RepositoryError, TransactionalCommit};

/// Builder for chaining multiple items into a single transactional commit batch.
pub struct CommitBuilder<'a, R> {
    repo: &'a R,
    entities: Vec<Entity>,
    outbox_messages: Vec<OutboxMessage>,
    read_model_plans: Vec<ReadModelWritePlan>,
    error: Option<RepositoryError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OutboxSource {
    aggregate_type: String,
    aggregate_id: String,
    source_sequence: u64,
}

impl OutboxSource {
    fn from_aggregate<A: Aggregate>(aggregate: &A) -> Self {
        Self {
            aggregate_type: A::aggregate_type().to_string(),
            aggregate_id: aggregate.entity().id().to_string(),
            source_sequence: aggregate.entity().version(),
        }
    }

    fn apply_to(&self, message: &mut OutboxMessage) {
        message.source_aggregate_type = Some(self.aggregate_type.clone());
        message.source_aggregate_id = Some(self.aggregate_id.clone());
        message.source_sequence = Some(self.source_sequence);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum StagedOutboxSource {
    #[default]
    None,
    Single(OutboxSource),
    Ambiguous,
}

impl StagedOutboxSource {
    fn record(&mut self, source: OutboxSource) {
        match self {
            Self::None => *self = Self::Single(source),
            Self::Single(existing) if *existing == source => {}
            Self::Single(_) => *self = Self::Ambiguous,
            Self::Ambiguous => {}
        }
    }

    fn apply_to(&self, messages: &mut [OutboxMessage]) {
        let Self::Single(source) = self else {
            return;
        };

        for message in messages {
            source.apply_to(message);
        }
    }
}

impl<'a, R> CommitBuilder<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self {
            repo,
            entities: vec![],
            outbox_messages: vec![],
            read_model_plans: vec![],
            error: None,
        }
    }

    /// Add a read-model write plan builder to the commit.
    pub fn read_models(mut self, read_models: ReadModelWritePlanBuilder) -> Self {
        if self.error.is_some() {
            return self;
        }

        match read_models.into_write_plan() {
            Ok(plan) => self.read_model_plans.push(plan),
            Err(err) => self.error = Some(err.into()),
        }
        self
    }

    /// Add an outbox message to the commit (takes ownership).
    pub fn outbox(mut self, msg: OutboxMessage) -> Self {
        self.outbox_messages.push(msg);
        self
    }

    /// Stage an aggregate and switch to a no-argument staged commit builder.
    pub fn aggregate<A: Aggregate>(self, aggregate: &'a mut A) -> StagedCommitBuilder<'a, R> {
        let source = OutboxSource::from_aggregate(aggregate);
        let mut builder = StagedCommitBuilder::from_builder(self);
        builder.outbox_source.record(source);
        builder.staged_entities.push(aggregate.entity_mut());
        builder
    }

    /// Commit all items plus the primary aggregate.
    pub fn commit<A: Aggregate>(mut self, aggregate: &mut A) -> Result<(), RepositoryError>
    where
        R: TransactionalCommit,
    {
        self.check_staged()?;
        for message in &mut self.outbox_messages {
            message.set_source(aggregate);
        }

        let mut entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
        entity_refs.push(aggregate.entity_mut());
        self.repo.commit_batch(CommitBatch {
            entities: entity_refs,
            outbox_messages: self.outbox_messages,
            read_model_plans: self.read_model_plans,
            snapshots: Vec::new(),
        })
    }

    /// Commit multiple entities in one batch (along with any queued read-model plans and outbox).
    ///
    /// Use `entity_mut()` on each aggregate to get the entity references:
    /// ```ignore
    /// repo.read_models(read_models)
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
            outbox_messages: self.outbox_messages,
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
            outbox_messages: self.outbox_messages,
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
    outbox_messages: Vec<OutboxMessage>,
    staged_entities: Vec<&'a mut Entity>,
    outbox_source: StagedOutboxSource,
    read_model_plans: Vec<ReadModelWritePlan>,
    error: Option<RepositoryError>,
}

impl<'a, R> StagedCommitBuilder<'a, R> {
    fn from_builder(builder: CommitBuilder<'a, R>) -> Self {
        Self {
            repo: builder.repo,
            entities: builder.entities,
            outbox_messages: builder.outbox_messages,
            staged_entities: Vec::new(),
            outbox_source: StagedOutboxSource::default(),
            read_model_plans: builder.read_model_plans,
            error: builder.error,
        }
    }

    pub fn read_models(mut self, read_models: ReadModelWritePlanBuilder) -> Self {
        if self.error.is_some() {
            return self;
        }

        match read_models.into_write_plan() {
            Ok(plan) => self.read_model_plans.push(plan),
            Err(err) => self.error = Some(err.into()),
        }
        self
    }

    pub fn outbox(mut self, msg: OutboxMessage) -> Self {
        self.outbox_messages.push(msg);
        self
    }

    pub fn aggregate<A: Aggregate>(mut self, aggregate: &'a mut A) -> Self {
        self.outbox_source
            .record(OutboxSource::from_aggregate(aggregate));
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
        self.outbox_source.apply_to(&mut self.outbox_messages);

        let mut entity_refs: Vec<&mut Entity> = self.entities.iter_mut().collect();
        entity_refs.extend(self.staged_entities);
        self.repo.commit_batch(CommitBatch {
            entities: entity_refs,
            outbox_messages: self.outbox_messages,
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

/// Extension trait to start a commit builder chain from an outbox message.
pub trait CommitBuilderExt: TransactionalCommit + Sized {
    /// Start a commit builder chain with an outbox message.
    fn outbox(&self, msg: OutboxMessage) -> CommitBuilder<'_, Self> {
        CommitBuilder::new(self).outbox(msg)
    }
}

impl<R: TransactionalCommit> CommitBuilderExt for R {}

/// Extension trait for relational read-model write-plan commit entrypoints.
///
/// Kept separate from `CommitBuilderExt` so callers explicitly opt into the
/// write-plan starter.
pub trait ReadModelWritePlanCommitExt: TransactionalCommit + Sized {
    /// Start a commit builder chain with a relational read-model write plan.
    fn read_models(&self, read_models: ReadModelWritePlanBuilder) -> CommitBuilder<'_, Self> {
        CommitBuilder::new(self).read_models(read_models)
    }

    /// Start a staged commit builder with an aggregate.
    fn aggregate<'a, A: Aggregate>(
        &'a self,
        aggregate: &'a mut A,
    ) -> StagedCommitBuilder<'a, Self> {
        CommitBuilder::new(self).aggregate(aggregate)
    }
}

impl<R: TransactionalCommit> ReadModelWritePlanCommitExt for R {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        impl_aggregate, Entity, EventRecord, Get, HashMapRepository, ReadModelWorkspaceExt, RowKey,
        RowValue,
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

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, crate::ReadModel)]
    #[readmodel(table = "commit_builder_views")]
    struct RelationalView {
        #[readmodel(id)]
        id: String,
        counter: i32,
    }

    #[derive(Default)]
    struct RecordingBatchRepo {
        fail: bool,
        entity_ids: RefCell<Vec<String>>,
        outbox_ids: RefCell<Vec<String>>,
        outbox_sources: RefCell<Vec<(String, Option<String>, Option<String>, Option<u64>)>>,
        read_model_keys: RefCell<Vec<String>>,
    }

    impl TransactionalCommit for RecordingBatchRepo {
        fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError> {
            *self.entity_ids.borrow_mut() = batch
                .entities
                .iter()
                .map(|entity| entity.id().to_string())
                .collect();
            *self.outbox_ids.borrow_mut() = batch
                .outbox_messages
                .iter()
                .map(|message| message.id().to_string())
                .collect();
            *self.outbox_sources.borrow_mut() = batch
                .outbox_messages
                .iter()
                .map(|message| {
                    (
                        message.id().to_string(),
                        message.source_aggregate_type.clone(),
                        message.source_aggregate_id.clone(),
                        message.source_sequence,
                    )
                })
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

    fn read_models(view: &RelationalView) -> crate::read_model::ReadModelWritePlanBuilder {
        let mut read_models = crate::read_model::ReadModelWritePlanBuilder::new();
        read_models.upsert(view).unwrap();
        read_models
    }

    fn view_key(id: &str) -> RowKey {
        RowKey::new([("id", RowValue::String(id.into()))])
    }

    fn lock_key_for(view: &RelationalView) -> String {
        read_models(view)
            .into_write_plan()
            .unwrap()
            .mutations
            .into_iter()
            .next()
            .unwrap()
            .lock_key()
    }

    fn loaded_view(repo: &HashMapRepository, id: &str) -> Option<RelationalView> {
        repo.model_store()
            .workspace()
            .load::<RelationalView>(view_key(id))
            .one()
            .unwrap()
            .map(|versioned| versioned.data)
    }

    #[test]
    fn commit_read_models_and_aggregate() {
        let repo = HashMapRepository::new();

        let view = RelationalView {
            id: "1".into(),
            counter: 42,
        };

        let mut agg = TestAggregate::default();
        agg.touch();

        repo.read_models(read_models(&view))
            .commit(&mut agg)
            .unwrap();

        let loaded = loaded_view(&repo, "1").unwrap();
        assert_eq!(loaded.counter, 42);
        assert_eq!(agg.entity().committed_version(), 1);
    }

    #[test]
    fn commit_multiple_read_models() {
        let repo = HashMapRepository::new();

        let view1 = RelationalView {
            id: "1".into(),
            counter: 10,
        };
        let view2 = RelationalView {
            id: "2".into(),
            counter: 20,
        };

        let mut agg = TestAggregate::default();
        agg.touch();

        let mut read_models = crate::read_model::ReadModelWritePlanBuilder::new();
        read_models.upsert(&view1).unwrap().upsert(&view2).unwrap();

        repo.read_models(read_models).commit(&mut agg).unwrap();

        assert_eq!(loaded_view(&repo, "1").unwrap().counter, 10);
        assert_eq!(loaded_view(&repo, "2").unwrap().counter, 20);
    }

    #[test]
    fn commit_read_models_with_outbox() {
        let repo = HashMapRepository::new();

        let view = RelationalView {
            id: "1".into(),
            counter: 42,
        };

        let outbox = OutboxMessage::create("msg-1", "TestEvent", b"{}".to_vec()).unwrap();

        let mut agg = TestAggregate::default();
        agg.touch();

        repo.read_models(read_models(&view))
            .outbox(outbox)
            .commit(&mut agg)
            .unwrap();

        assert_eq!(loaded_view(&repo, "1").unwrap().counter, 42);
    }

    #[test]
    fn commit_outbox_then_read_models() {
        let repo = HashMapRepository::new();

        let view = RelationalView {
            id: "1".into(),
            counter: 99,
        };

        let outbox = OutboxMessage::create("msg-2", "TestEvent", b"{}".to_vec()).unwrap();

        let mut agg = TestAggregate::default();
        agg.touch();

        repo.outbox(outbox)
            .read_models(read_models(&view))
            .commit(&mut agg)
            .unwrap();

        assert_eq!(loaded_view(&repo, "1").unwrap().counter, 99);
    }

    #[test]
    fn commit_all_without_aggregate() {
        let repo = HashMapRepository::new();

        let view1 = RelationalView {
            id: "standalone-1".into(),
            counter: 1,
        };
        let view2 = RelationalView {
            id: "standalone-2".into(),
            counter: 2,
        };

        let mut read_models = crate::read_model::ReadModelWritePlanBuilder::new();
        read_models.upsert(&view1).unwrap().upsert(&view2).unwrap();

        ReadModelWritePlanCommitExt::read_models(&repo, read_models)
            .commit_all()
            .unwrap();

        assert_eq!(
            loaded_view(&repo, "standalone-1").unwrap().id,
            "standalone-1"
        );
        assert_eq!(
            loaded_view(&repo, "standalone-2").unwrap().id,
            "standalone-2"
        );
    }

    #[test]
    fn commit_many_multiple_aggregates() {
        let repo = HashMapRepository::new();

        let view = RelationalView {
            id: "multi".into(),
            counter: 77,
        };

        let mut agg1 = TestAggregate::default();
        agg1.touch();
        agg1.entity.set_id("agg-1");

        let mut agg2 = TestAggregate::default();
        agg2.touch();
        agg2.entity.set_id("agg-2");

        repo.read_models(read_models(&view))
            .commit_many(&mut [agg1.entity_mut(), agg2.entity_mut()])
            .unwrap();

        assert_eq!(loaded_view(&repo, "multi").unwrap().counter, 77);

        let e1 = repo.get("agg-1").unwrap();
        assert!(e1.is_some());
        let e2 = repo.get("agg-2").unwrap();
        assert!(e2.is_some());
    }

    #[test]
    fn staged_builder_ordering_is_semantic_for_outbox_session_and_aggregate() {
        fn record(order: u8) -> (Vec<String>, Vec<String>) {
            let repo = RecordingBatchRepo::default();
            let view = RelationalView {
                id: "ordered".into(),
                counter: 7,
            };
            let outbox = OutboxMessage::create("ordered-msg", "TestEvent", b"{}".to_vec()).unwrap();
            let mut agg = TestAggregate::default();
            agg.touch();

            match order {
                0 => ReadModelWritePlanCommitExt::read_models(&repo, read_models(&view))
                    .outbox(outbox)
                    .aggregate(&mut agg)
                    .commit()
                    .unwrap(),
                1 => repo
                    .outbox(outbox)
                    .read_models(read_models(&view))
                    .aggregate(&mut agg)
                    .commit()
                    .unwrap(),
                _ => ReadModelWritePlanCommitExt::aggregate(&repo, &mut agg)
                    .read_models(read_models(&view))
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
    fn staged_commit_sets_outbox_source_from_single_aggregate() {
        let repo = RecordingBatchRepo::default();
        let mut agg = TestAggregate::default();
        agg.touch();
        let outbox = OutboxMessage::create("sourced-msg", "TestEvent", b"{}".to_vec()).unwrap();

        ReadModelWritePlanCommitExt::aggregate(&repo, &mut agg)
            .outbox(outbox)
            .commit()
            .unwrap();

        assert_eq!(
            repo.outbox_sources.borrow().as_slice(),
            &[(
                "sourced-msg".to_string(),
                Some(TestAggregate::aggregate_type().to_string()),
                Some("agg-1".to_string()),
                Some(1),
            )]
        );
    }

    #[test]
    fn staged_builder_supports_multiple_aggregates() {
        let repo = RecordingBatchRepo::default();
        let view = RelationalView {
            id: "staged-multi".into(),
            counter: 77,
        };
        let mut agg1 = TestAggregate::default();
        agg1.touch();
        agg1.entity.set_id("agg-1");
        let mut agg2 = TestAggregate::default();
        agg2.touch();
        agg2.entity.set_id("agg-2");

        ReadModelWritePlanCommitExt::read_models(&repo, read_models(&view))
            .aggregate(&mut agg1)
            .aggregate(&mut agg2)
            .commit()
            .unwrap();

        assert_eq!(
            repo.read_model_keys.borrow().as_slice(),
            &[lock_key_for(&view)]
        );
        assert_eq!(
            repo.entity_ids.borrow().as_slice(),
            &["agg-1".to_string(), "agg-2".to_string()]
        );
    }

    #[test]
    fn invalid_session_plan_does_not_commit_aggregate() {
        let repo = RecordingBatchRepo::default();
        let mut session = crate::read_model::ReadModelWritePlanBuilder::new();
        session.mark_processed("", "message-1");
        let mut agg = TestAggregate::default();
        agg.touch();

        let err = ReadModelWritePlanCommitExt::read_models(&repo, session)
            .commit(&mut agg)
            .unwrap_err();

        assert!(
            matches!(err, RepositoryError::Model(message) if message.contains("processed-message"))
        );
        assert_eq!(agg.entity().committed_version(), 0);
        assert!(repo.entity_ids.borrow().is_empty());
    }

    #[test]
    fn commit_builder_failure_does_not_mark_aggregate_committed() {
        let repo = RecordingBatchRepo {
            fail: true,
            ..Default::default()
        };

        let view = RelationalView {
            id: "rollback".into(),
            counter: 1,
        };
        let outbox = OutboxMessage::create("msg-rollback", "TestEvent", b"{}".to_vec()).unwrap();
        let mut agg = TestAggregate::default();
        agg.touch();

        let err = repo
            .read_models(read_models(&view))
            .outbox(outbox)
            .commit(&mut agg)
            .unwrap_err();

        assert_eq!(err, RepositoryError::Model("injected batch failure".into()));
        assert_eq!(agg.entity().committed_version(), 0);
        assert_eq!(agg.entity().new_events().len(), 1);
        assert_eq!(
            repo.read_model_keys.borrow().as_slice(),
            &[lock_key_for(&view)]
        );
        assert!(repo.entity_ids.borrow().iter().any(|id| id == "agg-1"));
        assert!(repo
            .outbox_ids
            .borrow()
            .iter()
            .any(|id| id == "msg-rollback"));
    }

    #[test]
    fn commit_builder_empty_batch_succeeds() {
        let repo = RecordingBatchRepo::default();

        CommitBuilder::new(&repo).commit_all().unwrap();

        assert!(repo.entity_ids.borrow().is_empty());
        assert!(repo.read_model_keys.borrow().is_empty());
    }
}
