//! CommitBuilder - Chain projections, outbox, and aggregates for atomic commits.
//!
//! ## Example
//!
//! ```ignore
//! repo
//!     .projection(&mut game_view)
//!     .projection(&mut player_stats)
//!     .outbox(&mut message)
//!     .commit(&mut game)?;
//! ```

use std::marker::PhantomData;

use crate::core::{Aggregate, Commit, Entity, Get, RepositoryError};
use crate::outbox::OutboxMessage;
use crate::projection::{Projection, ProjectionError, ProjectionSchema};

/// Builder for chaining multiple items into a single atomic commit.
pub struct CommitBuilder<'a, R> {
    repo: &'a R,
    entities: Vec<&'a mut Entity>,
}

impl<'a, R> CommitBuilder<'a, R> {
    pub fn new(repo: &'a R) -> Self {
        Self {
            repo,
            entities: vec![],
        }
    }

    /// Add a projection to the commit.
    pub fn projection<T: ProjectionSchema>(mut self, proj: &'a mut Projection<T>) -> Self {
        proj.sync();
        self.entities.push(proj.entity_mut());
        self
    }

    /// Add an outbox message to the commit.
    pub fn outbox(mut self, msg: &'a mut OutboxMessage) -> Self {
        self.entities.push(msg.entity_mut());
        self
    }

    /// Commit all items plus the primary aggregate.
    pub fn commit<A: Aggregate>(mut self, aggregate: &'a mut A) -> Result<(), RepositoryError>
    where
        R: Commit,
    {
        self.entities.push(aggregate.entity_mut());
        self.repo.commit(&mut self.entities[..])
    }

    /// Commit without a primary aggregate.
    pub fn commit_all(mut self) -> Result<(), RepositoryError>
    where
        R: Commit,
    {
        if self.entities.is_empty() {
            return Ok(());
        }
        self.repo.commit(&mut self.entities[..])
    }
}

/// Extension trait to start a commit builder chain.
pub trait CommitBuilderExt: Commit + Sized {
    /// Start a commit builder chain with a projection.
    fn projection<'a, T: ProjectionSchema>(
        &'a self,
        proj: &'a mut Projection<T>,
    ) -> CommitBuilder<'a, Self> {
        CommitBuilder::new(self).projection(proj)
    }
}

impl<R: Commit> CommitBuilderExt for R {}

// ============================================================================
// Typed ProjectionRepository
// ============================================================================

/// A typed repository wrapper for accessing projections of a specific type.
pub struct ProjectionRepository<'a, R, T> {
    repo: &'a R,
    _marker: PhantomData<T>,
}

impl<'a, R, T> ProjectionRepository<'a, R, T>
where
    R: Get,
    T: ProjectionSchema,
{
    pub fn new(repo: &'a R) -> Self {
        Self {
            repo,
            _marker: PhantomData,
        }
    }

    /// Get a projection by its ID (without prefix).
    pub fn get(&self, id: &str) -> Result<Option<Projection<T>>, RepositoryError> {
        let key = format!("{}:{}", T::PREFIX, id);
        let entity = self.repo.get(&key)?;
        match entity {
            Some(e) => match Projection::from_entity(e) {
                Ok(p) => Ok(Some(p)),
                Err(ProjectionError::NoSnapshot) => Ok(None),
                Err(e) => Err(RepositoryError::Projection(e.to_string())),
            },
            None => Ok(None),
        }
    }
}

/// Extension trait for typed projection access.
pub trait ProjectionsExt: Get + Sized {
    /// Get a typed projection repository.
    fn projections<T: ProjectionSchema>(&self) -> ProjectionRepository<'_, Self, T> {
        ProjectionRepository::new(self)
    }
}

impl<R: Get> ProjectionsExt for R {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{impl_aggregate, Entity, EventRecord, HashMapRepository};
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

    impl ProjectionSchema for TestView {
        const PREFIX: &'static str = "test_view";
        fn id(&self) -> &str {
            &self.id
        }
    }

    #[test]
    fn commit_projection_and_aggregate() {
        let repo = HashMapRepository::new();

        let mut view = Projection::new(TestView {
            id: "1".into(),
            counter: 42,
        });

        let mut agg = TestAggregate::default();
        agg.touch();

        repo.projection(&mut view).commit(&mut agg).unwrap();

        // Verify both stored
        let loaded: Option<Projection<TestView>> = repo.projections().get("1").unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().data().counter, 42);
    }

    #[test]
    fn commit_multiple_projections() {
        let repo = HashMapRepository::new();

        let mut view1 = Projection::new(TestView {
            id: "1".into(),
            counter: 10,
        });
        let mut view2 = Projection::new(TestView {
            id: "2".into(),
            counter: 20,
        });

        let mut agg = TestAggregate::default();
        agg.touch();

        repo.projection(&mut view1)
            .projection(&mut view2)
            .commit(&mut agg)
            .unwrap();

        let loaded1: Projection<TestView> = repo.projections().get("1").unwrap().unwrap();
        let loaded2: Projection<TestView> = repo.projections().get("2").unwrap().unwrap();
        assert_eq!(loaded1.data().counter, 10);
        assert_eq!(loaded2.data().counter, 20);
    }

    #[test]
    fn commit_projection_with_outbox() {
        let repo = HashMapRepository::new();

        let mut view = Projection::new(TestView {
            id: "1".into(),
            counter: 42,
        });

        let mut outbox = OutboxMessage::create("msg-1", "TestEvent", b"{}".to_vec());

        let mut agg = TestAggregate::default();
        agg.touch();

        repo.projection(&mut view)
            .outbox(&mut outbox)
            .commit(&mut agg)
            .unwrap();

        let loaded: Projection<TestView> = repo.projections().get("1").unwrap().unwrap();
        assert_eq!(loaded.data().counter, 42);
    }

    #[test]
    fn projections_get_returns_none_for_missing() {
        let repo = HashMapRepository::new();
        let result: Option<Projection<TestView>> = repo.projections().get("nonexistent").unwrap();
        assert!(result.is_none());
    }
}
