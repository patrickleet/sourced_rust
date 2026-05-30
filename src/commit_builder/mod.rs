//! AsyncCommitBuilder chains read models, write plans, outbox, and aggregates
//! into one transactional batch.
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
//!     .commit(&mut game)
//!     .await?;
//! ```

use crate::aggregate::Aggregate;
use crate::entity::Entity;
use crate::outbox::OutboxMessage;
use crate::read_model::{ReadModelWritePlan, ReadModelWritePlanBuilder};
use crate::repository::{
    AsyncCommitBatch, AsyncStreamWrite, AsyncTransactionalCommit, RepositoryError, StreamIdentity,
};

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

/// Async builder for chaining multiple items into one transactional commit batch.
pub struct AsyncCommitBuilder<'a, R> {
    repo: &'a R,
    streams: Vec<AsyncStreamWrite<'a>>,
    outbox_messages: Vec<OutboxMessage>,
    read_model_plans: Vec<ReadModelWritePlan>,
    error: Option<RepositoryError>,
}

impl<'a, R> AsyncCommitBuilder<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self {
            repo,
            streams: Vec::new(),
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            error: None,
        }
    }

    /// Add a read-model write plan builder to the async commit.
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

    /// Add an outbox message to the async commit.
    pub fn outbox(mut self, msg: OutboxMessage) -> Self {
        self.outbox_messages.push(msg);
        self
    }

    /// Stage an aggregate and switch to a no-argument staged async commit builder.
    pub fn aggregate<A: Aggregate>(self, aggregate: &'a mut A) -> AsyncStagedCommitBuilder<'a, R> {
        let source = OutboxSource::from_aggregate(aggregate);
        let mut builder = AsyncStagedCommitBuilder::from_builder(self);
        builder.push_aggregate(source, A::aggregate_type(), aggregate);
        builder
    }

    /// Commit all items plus the primary aggregate.
    pub async fn commit<A: Aggregate + Send>(
        mut self,
        aggregate: &mut A,
    ) -> Result<(), RepositoryError>
    where
        R: AsyncTransactionalCommit,
    {
        self.check_staged()?;
        for message in &mut self.outbox_messages {
            message.set_source(aggregate);
        }

        let identity = StreamIdentity::new(A::aggregate_type(), aggregate.entity().id())?;
        self.streams
            .push(AsyncStreamWrite::new(identity, aggregate.entity_mut()));
        self.commit_streams().await
    }

    /// Commit multiple aggregates of the same type in one async batch.
    pub async fn commit_many<A: Aggregate + Send>(
        mut self,
        aggregates: &mut [&mut A],
    ) -> Result<(), RepositoryError>
    where
        R: AsyncTransactionalCommit,
    {
        self.check_staged()?;
        for aggregate in aggregates.iter_mut() {
            let identity = StreamIdentity::new(A::aggregate_type(), aggregate.entity().id())?;
            self.streams
                .push(AsyncStreamWrite::new(identity, aggregate.entity_mut()));
        }
        self.commit_streams().await
    }

    /// Commit without a primary aggregate.
    pub async fn commit_all(mut self) -> Result<(), RepositoryError>
    where
        R: AsyncTransactionalCommit,
    {
        self.check_staged()?;
        self.commit_streams().await
    }

    fn check_staged(&mut self) -> Result<(), RepositoryError> {
        if let Some(err) = self.error.take() {
            return Err(err);
        }
        Ok(())
    }

    async fn commit_streams(self) -> Result<(), RepositoryError>
    where
        R: AsyncTransactionalCommit,
    {
        self.repo
            .commit_batch_async(AsyncCommitBatch {
                streams: self.streams,
                outbox_messages: self.outbox_messages,
                read_model_plans: self.read_model_plans,
                snapshots: Vec::new(),
                inbox_receipts: Vec::new(),
            })
            .await
    }
}

/// Async builder returned after one or more aggregates are staged explicitly.
pub struct AsyncStagedCommitBuilder<'a, R> {
    repo: &'a R,
    streams: Vec<AsyncStreamWrite<'a>>,
    outbox_messages: Vec<OutboxMessage>,
    outbox_source: StagedOutboxSource,
    read_model_plans: Vec<ReadModelWritePlan>,
    error: Option<RepositoryError>,
}

impl<'a, R> AsyncStagedCommitBuilder<'a, R> {
    fn from_builder(builder: AsyncCommitBuilder<'a, R>) -> Self {
        Self {
            repo: builder.repo,
            streams: builder.streams,
            outbox_messages: builder.outbox_messages,
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
        let source = OutboxSource::from_aggregate(aggregate);
        self.push_aggregate(source, A::aggregate_type(), aggregate);
        self
    }

    pub fn entity(mut self, identity: StreamIdentity, entity: &'a mut Entity) -> Self {
        self.streams.push(AsyncStreamWrite::new(identity, entity));
        self
    }

    pub async fn commit(mut self) -> Result<(), RepositoryError>
    where
        R: AsyncTransactionalCommit,
    {
        self.check_staged()?;
        self.outbox_source.apply_to(&mut self.outbox_messages);
        self.repo
            .commit_batch_async(AsyncCommitBatch {
                streams: self.streams,
                outbox_messages: self.outbox_messages,
                read_model_plans: self.read_model_plans,
                snapshots: Vec::new(),
                inbox_receipts: Vec::new(),
            })
            .await
    }

    fn push_aggregate<A: Aggregate>(
        &mut self,
        source: OutboxSource,
        aggregate_type: &'static str,
        aggregate: &'a mut A,
    ) {
        if self.error.is_some() {
            return;
        }

        match StreamIdentity::new(aggregate_type, aggregate.entity().id()) {
            Ok(identity) => {
                self.outbox_source.record(source);
                self.streams
                    .push(AsyncStreamWrite::new(identity, aggregate.entity_mut()));
            }
            Err(err) => self.error = Some(err),
        }
    }

    fn check_staged(&mut self) -> Result<(), RepositoryError> {
        if let Some(err) = self.error.take() {
            return Err(err);
        }
        Ok(())
    }
}

/// Extension trait to start an async commit builder chain from an outbox message.
pub trait AsyncCommitBuilderExt: AsyncTransactionalCommit + Sized {
    /// Start an async commit builder chain with an outbox message.
    fn outbox(&self, msg: OutboxMessage) -> AsyncCommitBuilder<'_, Self> {
        AsyncCommitBuilder::new(self).outbox(msg)
    }
}

impl<R: AsyncTransactionalCommit> AsyncCommitBuilderExt for R {}

/// Extension trait for async relational read-model write-plan commit entrypoints.
pub trait AsyncReadModelWritePlanCommitExt: AsyncTransactionalCommit + Sized {
    /// Start an async commit builder chain with a relational read-model write plan.
    fn read_models(&self, read_models: ReadModelWritePlanBuilder) -> AsyncCommitBuilder<'_, Self> {
        AsyncCommitBuilder::new(self).read_models(read_models)
    }

    /// Start a staged async commit builder with an aggregate.
    fn aggregate<'a, A: Aggregate>(
        &'a self,
        aggregate: &'a mut A,
    ) -> AsyncStagedCommitBuilder<'a, Self> {
        AsyncCommitBuilder::new(self).aggregate(aggregate)
    }
}

impl<R: AsyncTransactionalCommit> AsyncReadModelWritePlanCommitExt for R {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sourced, AsyncReadModelWorkspaceExt, AsyncTransactionalCommit, Entity, HashMapRepository,
        RowKey, RowValue,
    };
    use serde::{Deserialize, Serialize};
    use std::sync::Mutex;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::ptr;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
        }
    }

    type OutboxSourceRecord = (String, Option<String>, Option<String>, Option<u64>);

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

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, crate::ReadModel)]
    #[table("commit_builder_views")]
    struct RelationalView {
        #[id]
        id: String,
        counter: i32,
    }

    #[derive(Default)]
    struct RecordingAsyncBatchRepo {
        fail: bool,
        stream_ids: Mutex<Vec<(String, String)>>,
        outbox_ids: Mutex<Vec<String>>,
        outbox_sources: Mutex<Vec<OutboxSourceRecord>>,
        read_model_keys: Mutex<Vec<String>>,
    }

    impl AsyncTransactionalCommit for RecordingAsyncBatchRepo {
        async fn commit_batch_async<'a>(
            &'a self,
            batch: AsyncCommitBatch<'a>,
        ) -> Result<(), RepositoryError> {
            *self.stream_ids.lock().unwrap() = batch
                .streams
                .iter()
                .map(|stream| {
                    (
                        stream.identity.aggregate_type().to_string(),
                        stream.identity.aggregate_id().to_string(),
                    )
                })
                .collect();
            *self.outbox_ids.lock().unwrap() = batch
                .outbox_messages
                .iter()
                .map(|message| message.id().to_string())
                .collect();
            *self.outbox_sources.lock().unwrap() = batch
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
            *self.read_model_keys.lock().unwrap() = batch
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
                return Err(RepositoryError::Model(
                    "injected async batch failure".into(),
                ));
            }

            for stream in batch.streams {
                stream.entity.mark_committed();
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
        block_on(
            repo.model_store()
                .workspace_async()
                .load_async::<RelationalView>(view_key(id))
                .one(),
        )
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
        agg.touch().unwrap();

        block_on(
            AsyncReadModelWritePlanCommitExt::read_models(&repo, read_models(&view))
                .commit(&mut agg),
        )
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
        agg.touch().unwrap();

        let mut read_models = crate::read_model::ReadModelWritePlanBuilder::new();
        read_models.upsert(&view1).unwrap().upsert(&view2).unwrap();

        block_on(
            AsyncReadModelWritePlanCommitExt::read_models(&repo, read_models).commit(&mut agg),
        )
        .unwrap();

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
        agg.touch().unwrap();

        block_on(
            AsyncReadModelWritePlanCommitExt::read_models(&repo, read_models(&view))
                .outbox(outbox)
                .commit(&mut agg),
        )
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
        agg.touch().unwrap();

        block_on(
            AsyncCommitBuilderExt::outbox(&repo, outbox)
                .read_models(read_models(&view))
                .commit(&mut agg),
        )
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

        block_on(AsyncReadModelWritePlanCommitExt::read_models(&repo, read_models).commit_all())
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
        agg1.touch().unwrap();
        agg1.entity.set_id("agg-1");

        let mut agg2 = TestAggregate::default();
        agg2.touch().unwrap();
        agg2.entity.set_id("agg-2");

        block_on(
            AsyncReadModelWritePlanCommitExt::read_models(&repo, read_models(&view))
                .commit_many(&mut [&mut agg1, &mut agg2]),
        )
        .unwrap();

        assert_eq!(loaded_view(&repo, "multi").unwrap().counter, 77);

        let agg_type = TestAggregate::aggregate_type();
        let e1 = block_on(crate::AsyncGetStream::get_stream(
            &repo,
            &StreamIdentity::new(agg_type, "agg-1").unwrap(),
        ))
        .unwrap();
        assert!(e1.is_some());
        let e2 = block_on(crate::AsyncGetStream::get_stream(
            &repo,
            &StreamIdentity::new(agg_type, "agg-2").unwrap(),
        ))
        .unwrap();
        assert!(e2.is_some());
    }

    #[test]
    fn staged_builder_ordering_is_semantic_for_outbox_session_and_aggregate() {
        fn record(order: u8) -> (Vec<(String, String)>, Vec<String>) {
            let repo = RecordingAsyncBatchRepo::default();
            let view = RelationalView {
                id: "ordered".into(),
                counter: 7,
            };
            let outbox = OutboxMessage::create("ordered-msg", "TestEvent", b"{}".to_vec()).unwrap();
            let mut agg = TestAggregate::default();
            agg.touch().unwrap();

            match order {
                0 => block_on(
                    AsyncReadModelWritePlanCommitExt::read_models(&repo, read_models(&view))
                        .outbox(outbox)
                        .aggregate(&mut agg)
                        .commit(),
                )
                .unwrap(),
                1 => block_on(
                    repo.outbox(outbox)
                        .read_models(read_models(&view))
                        .aggregate(&mut agg)
                        .commit(),
                )
                .unwrap(),
                _ => block_on(
                    AsyncReadModelWritePlanCommitExt::aggregate(&repo, &mut agg)
                        .read_models(read_models(&view))
                        .outbox(outbox)
                        .commit(),
                )
                .unwrap(),
            }

            let stream_ids = repo.stream_ids.lock().unwrap().clone();
            let read_model_keys = repo.read_model_keys.lock().unwrap().clone();
            (stream_ids, read_model_keys)
        }

        let baseline = record(0);
        assert_eq!(record(1), baseline);
        assert_eq!(record(2), baseline);
    }

    #[test]
    fn staged_commit_sets_outbox_source_from_single_aggregate() {
        let repo = RecordingAsyncBatchRepo::default();
        let mut agg = TestAggregate::default();
        agg.touch().unwrap();
        let outbox = OutboxMessage::create("sourced-msg", "TestEvent", b"{}".to_vec()).unwrap();

        block_on(
            AsyncReadModelWritePlanCommitExt::aggregate(&repo, &mut agg)
                .outbox(outbox)
                .commit(),
        )
        .unwrap();

        assert_eq!(
            repo.outbox_sources.lock().unwrap().as_slice(),
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
        let repo = RecordingAsyncBatchRepo::default();
        let view = RelationalView {
            id: "staged-multi".into(),
            counter: 77,
        };
        let mut agg1 = TestAggregate::default();
        agg1.touch().unwrap();
        agg1.entity.set_id("agg-1");
        let mut agg2 = TestAggregate::default();
        agg2.touch().unwrap();
        agg2.entity.set_id("agg-2");

        block_on(
            AsyncReadModelWritePlanCommitExt::read_models(&repo, read_models(&view))
                .aggregate(&mut agg1)
                .aggregate(&mut agg2)
                .commit(),
        )
        .unwrap();

        assert_eq!(
            repo.read_model_keys.lock().unwrap().as_slice(),
            &[lock_key_for(&view)]
        );
        assert_eq!(
            repo.stream_ids.lock().unwrap().as_slice(),
            &[
                (
                    TestAggregate::aggregate_type().to_string(),
                    "agg-1".to_string()
                ),
                (
                    TestAggregate::aggregate_type().to_string(),
                    "agg-2".to_string()
                ),
            ]
        );
    }

    #[test]
    fn commit_builder_failure_does_not_mark_aggregate_committed() {
        let repo = RecordingAsyncBatchRepo {
            fail: true,
            ..Default::default()
        };

        let view = RelationalView {
            id: "rollback".into(),
            counter: 1,
        };
        let outbox = OutboxMessage::create("msg-rollback", "TestEvent", b"{}".to_vec()).unwrap();
        let mut agg = TestAggregate::default();
        agg.touch().unwrap();

        let err = block_on(
            AsyncReadModelWritePlanCommitExt::read_models(&repo, read_models(&view))
                .outbox(outbox)
                .commit(&mut agg),
        )
        .unwrap_err();

        assert_eq!(
            err,
            RepositoryError::Model("injected async batch failure".into())
        );
        assert_eq!(agg.entity().committed_version(), 0);
        assert_eq!(agg.entity().new_events().len(), 1);
        assert_eq!(
            repo.read_model_keys.lock().unwrap().as_slice(),
            &[lock_key_for(&view)]
        );
        assert!(repo
            .stream_ids
            .lock()
            .unwrap()
            .iter()
            .any(|(_, id)| id == "agg-1"));
        assert!(repo
            .outbox_ids
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == "msg-rollback"));
    }

    #[test]
    fn commit_builder_empty_batch_succeeds() {
        let repo = RecordingAsyncBatchRepo::default();

        block_on(AsyncCommitBuilder::new(&repo).commit_all()).unwrap();

        assert!(repo.stream_ids.lock().unwrap().is_empty());
        assert!(repo.read_model_keys.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn async_commit_read_models_and_aggregate() {
        let repo = RecordingAsyncBatchRepo::default();
        let view = RelationalView {
            id: "async-view".into(),
            counter: 42,
        };
        let mut agg = TestAggregate::default();
        agg.touch().unwrap();

        repo.read_models(read_models(&view))
            .commit(&mut agg)
            .await
            .unwrap();

        assert_eq!(
            repo.stream_ids.lock().unwrap().as_slice(),
            &[(
                TestAggregate::aggregate_type().to_string(),
                "agg-1".to_string()
            )]
        );
        assert_eq!(
            repo.read_model_keys.lock().unwrap().as_slice(),
            &[lock_key_for(&view)]
        );
        assert_eq!(agg.entity().committed_version(), 1);
    }

    #[tokio::test]
    async fn async_staged_builder_ordering_sets_outbox_source() {
        let repo = RecordingAsyncBatchRepo::default();
        let view = RelationalView {
            id: "async-staged".into(),
            counter: 7,
        };
        let mut agg = TestAggregate::default();
        agg.touch().unwrap();
        let outbox = OutboxMessage::create("async-msg", "TestEvent", b"{}".to_vec()).unwrap();

        repo.read_models(read_models(&view))
            .outbox(outbox)
            .aggregate(&mut agg)
            .commit()
            .await
            .unwrap();

        assert_eq!(
            repo.outbox_sources.lock().unwrap().as_slice(),
            &[(
                "async-msg".to_string(),
                Some(TestAggregate::aggregate_type().to_string()),
                Some("agg-1".to_string()),
                Some(1),
            )]
        );
        assert_eq!(
            repo.read_model_keys.lock().unwrap().as_slice(),
            &[lock_key_for(&view)]
        );
    }

    #[tokio::test]
    async fn async_commit_many_supports_same_type_aggregates() {
        let repo = RecordingAsyncBatchRepo::default();
        let mut agg1 = TestAggregate::default();
        agg1.touch().unwrap();
        agg1.entity.set_id("async-agg-1");
        let mut agg2 = TestAggregate::default();
        agg2.touch().unwrap();
        agg2.entity.set_id("async-agg-2");

        AsyncCommitBuilder::new(&repo)
            .commit_many(&mut [&mut agg1, &mut agg2])
            .await
            .unwrap();

        assert_eq!(
            repo.stream_ids.lock().unwrap().as_slice(),
            &[
                (
                    TestAggregate::aggregate_type().to_string(),
                    "async-agg-1".to_string(),
                ),
                (
                    TestAggregate::aggregate_type().to_string(),
                    "async-agg-2".to_string(),
                ),
            ]
        );
    }
}
