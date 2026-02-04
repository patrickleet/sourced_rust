use std::fmt;
use std::marker::PhantomData;

use super::entity::Entity;
use super::error::RepositoryError;
use super::event_record::EventRecord;
use super::repository::Repository;

pub trait Aggregate: Sized + Default {
    type ReplayError: fmt::Display;

    fn new_empty() -> Self {
        Self::default()
    }
    fn entity(&self) -> &Entity;
    fn entity_mut(&mut self) -> &mut Entity;
    fn replay_event(&mut self, event: &EventRecord) -> Result<(), Self::ReplayError>;
}

#[macro_export]
macro_rules! impl_aggregate {
    ($ty:ty, $entity:ident, $replay:ident) => {
        $crate::impl_aggregate!($ty, $entity, $replay, String);
    };
    ($ty:ty, $entity:ident, $replay:ident, $err:ty) => {
        impl $crate::Aggregate for $ty {
            type ReplayError = $err;

            fn entity(&self) -> &$crate::Entity {
                &self.$entity
            }

            fn entity_mut(&mut self) -> &mut $crate::Entity {
                &mut self.$entity
            }

            fn replay_event(
                &mut self,
                event: &$crate::EventRecord,
            ) -> Result<(), Self::ReplayError> {
                Self::$replay(self, event)
            }
        }
    };
}

#[macro_export]
macro_rules! aggregate {
    ($ty:ty, $entity:ident, $replay:ident, $event_ty:ty, { $($events:tt)* }) => {
        $crate::event_map!($event_ty, { $($events)* });
        $crate::impl_aggregate!($ty, $entity, $replay);
    };
}

/// Hydrate an aggregate from an entity by replaying its events.
pub fn hydrate<A: Aggregate>(entity: Entity) -> Result<A, RepositoryError> {
    let mut aggregate = A::new_empty();
    *aggregate.entity_mut() = entity;

    let events = aggregate.entity().events().to_vec();
    aggregate.entity_mut().set_replaying(true);
    for event in &events {
        if let Err(err) = aggregate.replay_event(event) {
            aggregate.entity_mut().set_replaying(false);
            return Err(RepositoryError::Replay(err.to_string()));
        }
    }
    aggregate.entity_mut().set_replaying(false);

    Ok(aggregate)
}

/// Trait for repositories that support unlocking entities.
pub trait UnlockableRepository {
    fn unlock(&self, id: &str) -> Result<(), RepositoryError>;
}

/// Trait for repositories that support non-locking reads.
pub trait PeekableRepository {
    fn peek(&self, id: &str) -> Result<Option<Entity>, RepositoryError>;
    fn peek_all(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError>;
}

/// Extension trait adding aggregate-aware methods to any Repository.
pub trait RepositoryExt: Repository {
    fn get_aggregate<A: Aggregate>(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        let entity = self.get(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        Ok(Some(hydrate::<A>(entity)?))
    }

    fn get_all_aggregates<A: Aggregate>(
        &self,
        ids: &[&str],
    ) -> Result<Vec<A>, RepositoryError> {
        let entities = self.get_all(ids)?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            aggregates.push(hydrate::<A>(entity)?);
        }
        Ok(aggregates)
    }

    fn commit_aggregate<A: Aggregate>(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.commit(aggregate.entity_mut())
    }

    fn commit_all_aggregates<A: Aggregate>(
        &self,
        aggregates: &mut [&mut A],
    ) -> Result<(), RepositoryError> {
        let mut entities: Vec<&mut Entity> = aggregates
            .iter_mut()
            .map(|aggregate| (*aggregate).entity_mut())
            .collect();
        self.commit(&mut entities[..])
    }
}

impl<R: Repository> RepositoryExt for R {}

/// Builder trait for creating typed aggregate repositories.
pub trait AggregateBuilder: Repository + Sized {
    fn aggregate<A: Aggregate>(self) -> AggregateRepository<Self, A> {
        AggregateRepository::new(self)
    }
}

impl<R: Repository> AggregateBuilder for R {}

/// A repository wrapper that provides typed access to a specific aggregate type.
pub struct AggregateRepository<R, A> {
    repo: R,
    _marker: PhantomData<A>,
}

impl<R, A> AggregateRepository<R, A> {
    pub fn new(repo: R) -> Self {
        AggregateRepository {
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
    R: Repository,
    A: Aggregate,
{
    pub fn get(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        let entity = self.repo.get(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        Ok(Some(hydrate::<A>(entity)?))
    }

    pub fn get_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        let entities = self.repo.get_all(ids)?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            aggregates.push(hydrate::<A>(entity)?);
        }
        Ok(aggregates)
    }

    pub fn commit(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.repo.commit(aggregate.entity_mut())
    }

    pub fn commit_all(&self, aggregates: &mut [&mut A]) -> Result<(), RepositoryError> {
        let mut entities: Vec<&mut Entity> = aggregates
            .iter_mut()
            .map(|aggregate| (*aggregate).entity_mut())
            .collect();
        self.repo.commit(&mut entities[..])
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: UnlockableRepository,
    A: Aggregate,
{
    pub fn abort(&self, aggregate: &A) -> Result<(), RepositoryError> {
        self.repo.unlock(aggregate.entity().id())
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: PeekableRepository,
    A: Aggregate,
{
    pub fn peek(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        let entity = self.repo.peek(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        Ok(Some(hydrate::<A>(entity)?))
    }

    pub fn peek_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        let entities = self.repo.peek_all(ids)?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            aggregates.push(hydrate::<A>(entity)?);
        }
        Ok(aggregates)
    }
}
