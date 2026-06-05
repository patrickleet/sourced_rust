//! Typed dependency wrappers for microsvc handlers.

use crate::aggregate::AggregateRepository;
use crate::outbox_worker::AsyncOutboxStore;
use crate::repository::{ReadModelWritePlanStore, RelationalReadModelQueryStore, Repository};

/// Dependency capability for services that expose an aggregate repository.
pub trait HasRepo {
    type Repo;

    fn repo(&self) -> &Self::Repo;
}

/// Dependency capability for services that expose a read-model store.
pub trait HasReadModelStore {
    type ReadModelStore;

    fn read_model_store(&self) -> &Self::ReadModelStore;
}

/// Dependency capability for repositories that expose a durable outbox store.
///
/// A runtime uses this to build an `OutboxDispatcher` that drains committed
/// outbox rows to a transport, without naming the concrete repository type. The
/// capability resolves through the repository wrappers
/// (`AggregateRepository` -> `QueuedRepository` -> the leaf SQL/in-memory repo).
pub trait HasOutboxStore {
    /// The concrete outbox store this repository produces.
    type OutboxStore: AsyncOutboxStore;

    /// Produce a handle to the durable outbox store.
    fn outbox_store(&self) -> Self::OutboxStore;
}

// Leaf repositories own the store. Each calls its inherent `outbox_store()` —
// inherent methods take precedence over the trait method of the same name, so
// `self.outbox_store()` here does not recurse.
impl HasOutboxStore for crate::HashMapRepository {
    type OutboxStore = crate::HashMapOutboxStore;
    fn outbox_store(&self) -> Self::OutboxStore {
        crate::HashMapRepository::outbox_store(self)
    }
}

#[cfg(feature = "sqlite")]
impl HasOutboxStore for crate::SqliteRepository {
    type OutboxStore = crate::SqliteOutboxStore;
    fn outbox_store(&self) -> Self::OutboxStore {
        crate::SqliteRepository::outbox_store(self)
    }
}

#[cfg(feature = "postgres")]
impl HasOutboxStore for crate::PostgresRepository {
    type OutboxStore = crate::PostgresOutboxStore;
    fn outbox_store(&self) -> Self::OutboxStore {
        crate::PostgresRepository::outbox_store(self)
    }
}

// Wrappers delegate inward.
impl<R, A> HasOutboxStore for AggregateRepository<R, A>
where
    R: HasOutboxStore,
{
    type OutboxStore = R::OutboxStore;
    fn outbox_store(&self) -> Self::OutboxStore {
        self.repo().outbox_store()
    }
}

impl<R, L> HasOutboxStore for crate::QueuedRepository<R, L>
where
    R: HasOutboxStore,
{
    type OutboxStore = R::OutboxStore;
    fn outbox_store(&self) -> Self::OutboxStore {
        self.inner().outbox_store()
    }
}

impl<R> HasRepo for R
where
    R: Repository,
{
    type Repo = R;

    fn repo(&self) -> &Self::Repo {
        self
    }
}

impl<R, A> HasRepo for AggregateRepository<R, A> {
    type Repo = Self;

    fn repo(&self) -> &Self::Repo {
        self
    }
}

impl<S> HasReadModelStore for S
where
    S: ReadModelWritePlanStore + RelationalReadModelQueryStore,
{
    type ReadModelStore = S;

    fn read_model_store(&self) -> &Self::ReadModelStore {
        self
    }
}

/// Dependencies for a service that only needs an aggregate repository.
#[derive(Clone)]
pub struct RepoDependencies<R> {
    repo: R,
}

impl<R> RepoDependencies<R> {
    pub fn new(repo: R) -> Self {
        Self { repo }
    }
}

impl<R> HasRepo for RepoDependencies<R> {
    type Repo = R;

    fn repo(&self) -> &Self::Repo {
        &self.repo
    }
}

/// Dependencies for a service that only needs a read-model store.
#[derive(Clone)]
pub struct ReadModelStoreDependencies<S> {
    read_model_store: S,
}

impl<S> ReadModelStoreDependencies<S> {
    pub fn new(read_model_store: S) -> Self {
        Self { read_model_store }
    }
}

impl<S> HasReadModelStore for ReadModelStoreDependencies<S> {
    type ReadModelStore = S;

    fn read_model_store(&self) -> &Self::ReadModelStore {
        &self.read_model_store
    }
}

/// Dependencies for a service that needs both an aggregate repository and a
/// read-model store.
#[derive(Clone)]
pub struct RepoReadModelDependencies<R, S> {
    repo: R,
    read_model_store: S,
}

impl<R, S> RepoReadModelDependencies<R, S> {
    pub fn new(repo: R, read_model_store: S) -> Self {
        Self {
            repo,
            read_model_store,
        }
    }
}

impl<R, S> HasRepo for RepoReadModelDependencies<R, S> {
    type Repo = R;

    fn repo(&self) -> &Self::Repo {
        &self.repo
    }
}

impl<R, S> HasReadModelStore for RepoReadModelDependencies<R, S> {
    type ReadModelStore = S;

    fn read_model_store(&self) -> &Self::ReadModelStore {
        &self.read_model_store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_has_outbox_store<T: HasOutboxStore>() {}

    #[test]
    fn has_outbox_store_resolves_through_repo_wrappers() {
        // The capability must resolve for the leaf repo and through the
        // AggregateRepository -> QueuedRepository wrapper chain the canonical
        // `repo.queued().aggregate::<A>()` builder produces. `A` is unbounded,
        // so the unit type stands in for any aggregate.
        assert_has_outbox_store::<crate::HashMapRepository>();
        assert_has_outbox_store::<AggregateRepository<crate::HashMapRepository, ()>>();
        assert_has_outbox_store::<
            AggregateRepository<crate::QueuedRepository<crate::HashMapRepository>, ()>,
        >();
    }
}
