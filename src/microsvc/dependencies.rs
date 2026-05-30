//! Typed dependency wrappers for microsvc handlers.

use crate::aggregate::AsyncAggregateRepository;
use crate::repository::{
    AsyncReadModelWritePlanStore, AsyncRelationalReadModelQueryStore, AsyncRepository,
};
use crate::snapshot::AsyncSnapshotAggregateRepository;

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

impl<R> HasRepo for R
where
    R: AsyncRepository,
{
    type Repo = R;

    fn repo(&self) -> &Self::Repo {
        self
    }
}

impl<R, A> HasRepo for AsyncAggregateRepository<R, A> {
    type Repo = Self;

    fn repo(&self) -> &Self::Repo {
        self
    }
}

impl<R, A> HasRepo for AsyncSnapshotAggregateRepository<R, A> {
    type Repo = Self;

    fn repo(&self) -> &Self::Repo {
        self
    }
}

impl<S> HasReadModelStore for S
where
    S: AsyncReadModelWritePlanStore + AsyncRelationalReadModelQueryStore,
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
