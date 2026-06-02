use std::marker::PhantomData;

use crate::entity::Entity;
use crate::queued_repo::{GetAllWithOpts, GetWithOpts, ReadOpts, UnlockableRepository};
use crate::repository::{
    CommitBatch, GetStream, RepositoryError, StreamIdentity, StreamWrite, TransactionalCommit,
};

use super::{hydrate, Aggregate};

fn stream_identity_for<A: Aggregate>(
    aggregate_id: &str,
) -> Result<StreamIdentity, RepositoryError> {
    StreamIdentity::new(A::aggregate_type(), aggregate_id)
}

/// Builder trait for creating typed async aggregate repositories.
pub trait AggregateBuilder: Sized {
    fn aggregate<A: Aggregate>(self) -> AggregateRepository<Self, A> {
        AggregateRepository::new(self)
    }
}

impl<T> AggregateBuilder for T {}

/// Async repository wrapper for a specific aggregate type.
pub struct AggregateRepository<R, A> {
    repo: R,
    _marker: PhantomData<A>,
}

impl<R, A> AggregateRepository<R, A> {
    pub fn new(repo: R) -> Self {
        Self {
            repo,
            _marker: PhantomData,
        }
    }

    pub fn repo(&self) -> &R {
        &self.repo
    }

    pub fn repo_mut(&mut self) -> &mut R {
        &mut self.repo
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: GetStream,
    A: Aggregate + Send,
{
    pub async fn get(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        let identity = stream_identity_for::<A>(id)?;
        let entity = self.repo.get_stream(&identity).await?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        Ok(Some(hydrate::<A>(entity)?))
    }

    /// Load existing aggregates for the provided ids.
    ///
    /// Each id is converted to a `StreamIdentity`, fetched through `get_streams`,
    /// and hydrated if present. Missing streams are skipped, and backend
    /// implementations may return aggregates in storage order rather than input
    /// order.
    pub async fn get_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        let identities = ids
            .iter()
            .map(|id| stream_identity_for::<A>(id))
            .collect::<Result<Vec<_>, _>>()?;
        let entities = self.repo.get_streams(&identities).await?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            aggregates.push(hydrate::<A>(entity)?);
        }
        Ok(aggregates)
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: TransactionalCommit,
    A: Aggregate + Send,
{
    pub async fn commit(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        let identity = stream_identity_for::<A>(aggregate.entity().id())?;
        let stream = StreamWrite::new(identity, aggregate.entity_mut());
        self.repo.commit_batch(CommitBatch::new(vec![stream])).await
    }

    pub async fn commit_all(&self, aggregates: &mut [&mut A]) -> Result<(), RepositoryError> {
        let mut streams = Vec::with_capacity(aggregates.len());
        for aggregate in aggregates.iter_mut() {
            let identity = stream_identity_for::<A>((*aggregate).entity().id())?;
            streams.push(StreamWrite::new(identity, (*aggregate).entity_mut()));
        }
        self.repo.commit_batch(CommitBatch::new(streams)).await
    }

    pub async fn commit_entities(
        &self,
        streams: Vec<(StreamIdentity, &mut Entity)>,
    ) -> Result<(), RepositoryError> {
        let streams = streams
            .into_iter()
            .map(|(identity, entity)| StreamWrite::new(identity, entity))
            .collect();
        self.repo.commit_batch(CommitBatch::new(streams)).await
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: GetWithOpts,
    A: Aggregate + Send,
{
    /// Load an aggregate with options (e.g. `ReadOpts::no_lock()` to skip the
    /// queue lock when the repository is a `queued()` wrapper).
    pub async fn get_with(&self, id: &str, opts: ReadOpts) -> Result<Option<A>, RepositoryError> {
        let identity = stream_identity_for::<A>(id)?;
        let Some(entity) = self.repo.get_stream_with(&identity, opts).await? else {
            return Ok(None);
        };
        Ok(Some(hydrate::<A>(entity)?))
    }

    /// Non-locking read (alias for `get_with(ReadOpts::no_lock())`).
    pub async fn peek(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        self.get_with(id, ReadOpts::no_lock()).await
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: GetAllWithOpts,
    A: Aggregate + Send,
{
    /// Load aggregates for the provided ids with options.
    pub async fn get_all_with(
        &self,
        ids: &[&str],
        opts: ReadOpts,
    ) -> Result<Vec<A>, RepositoryError> {
        let identities = ids
            .iter()
            .map(|id| stream_identity_for::<A>(id))
            .collect::<Result<Vec<_>, _>>()?;
        let entities = self.repo.get_streams_with(&identities, opts).await?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            aggregates.push(hydrate::<A>(entity)?);
        }
        Ok(aggregates)
    }

    /// Non-locking multi-read (alias for `get_all_with(ReadOpts::no_lock())`).
    pub async fn peek_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        self.get_all_with(ids, ReadOpts::no_lock()).await
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: UnlockableRepository,
    A: Aggregate,
{
    /// Release the lock held for an aggregate after an aborted load.
    pub async fn abort(&self, aggregate: &A) -> Result<(), RepositoryError> {
        let identity = stream_identity_for::<A>(aggregate.entity().id())?;
        // Forward to the repo's `abort` hook (not `unlock`) so an
        // `UnlockableRepository` that overrides `abort` for extra cleanup
        // is honored. The default `abort` delegates to `unlock`.
        self.repo.abort(&identity).await
    }

    /// Release the lock held for an aggregate id.
    pub async fn unlock(&self, id: &str) -> Result<(), RepositoryError> {
        let identity = stream_identity_for::<A>(id)?;
        self.repo.unlock(&identity).await
    }
}
