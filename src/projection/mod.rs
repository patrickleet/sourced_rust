//! Projections - Read models optimized for queries.
//!
//! Projections store their current state as a snapshot event within an Entity,
//! allowing them to be committed atomically with aggregates and outbox messages.
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::{Projection, ProjectionSchema};
//!
//! #[derive(Serialize, Deserialize)]
//! struct GameView {
//!     pub id: String,
//!     pub score: u32,
//! }
//!
//! impl ProjectionSchema for GameView {
//!     const PREFIX: &'static str = "game_view";
//!     fn id(&self) -> &str { &self.id }
//! }
//!
//! // Load or create
//! let mut view = repo.projections::<GameView>().upsert(GameView { id: "123".into(), score: 0 })?;
//!
//! // Update
//! view.data_mut().score = 100;
//!
//! // Commit atomically with aggregate (takes ownership of view)
//! repo
//!     .projection(view)
//!     .commit(&mut game)?;
//! ```

use serde::{de::DeserializeOwned, Serialize};
use std::fmt;

use crate::core::Entity;

/// Error type for projection operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionError {
    /// The entity has no snapshot event to deserialize from.
    NoSnapshot,
    /// Failed to deserialize the snapshot data.
    Deserialize(String),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectionError::NoSnapshot => write!(f, "projection entity has no snapshot event"),
            ProjectionError::Deserialize(e) => write!(f, "failed to deserialize projection: {}", e),
        }
    }
}

impl std::error::Error for ProjectionError {}

/// Trait for types that can be stored as projections.
pub trait ProjectionSchema: Serialize + DeserializeOwned {
    /// The key prefix for this projection type (e.g., "game_view", "player_stats").
    const PREFIX: &'static str;

    /// Returns the unique identifier for this projection instance.
    fn id(&self) -> &str;

    /// Returns the full storage key (prefix:id).
    fn key(&self) -> String {
        format!("{}:{}", Self::PREFIX, self.id())
    }
}

/// A projection wraps typed data with an Entity for storage.
///
/// Unlike aggregates, projections overwrite state (single snapshot event)
/// rather than append events.
#[derive(Debug)]
pub struct Projection<T> {
    entity: Entity,
    data: T,
    dirty: bool,
}

impl<T: ProjectionSchema> Projection<T> {
    /// Create a new projection with initial data.
    pub fn new(data: T) -> Self {
        let key = data.key();
        Self {
            entity: Entity::with_id(&key),
            data,
            dirty: true,
        }
    }

    /// Load a projection from an existing entity.
    pub(crate) fn from_entity(entity: Entity) -> Result<Self, ProjectionError> {
        let snapshot = entity.events().last().ok_or(ProjectionError::NoSnapshot)?;
        let data: T = bitcode::deserialize(snapshot.payload_bytes())
            .map_err(|e| ProjectionError::Deserialize(e.to_string()))?;
        Ok(Self {
            entity,
            data,
            dirty: false,
        })
    }

    /// Get a reference to the projection data.
    pub fn data(&self) -> &T {
        &self.data
    }

    /// Get a mutable reference to the projection data.
    /// Marks the projection as dirty.
    pub fn data_mut(&mut self) -> &mut T {
        self.dirty = true;
        &mut self.data
    }

    /// Consume the projection and return the inner data.
    pub fn into_data(self) -> T {
        self.data
    }

    /// Check if the projection has been modified.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Get the storage key.
    pub fn key(&self) -> &str {
        self.entity.id()
    }

    /// Sync data to the entity (writes snapshot if dirty).
    pub(crate) fn sync(&mut self) {
        if self.dirty {
            self.entity.set_snapshot(&self.data);
            self.dirty = false;
        }
    }

    /// Consume the projection, sync data, and return the underlying entity.
    pub(crate) fn into_entity(mut self) -> Entity {
        self.sync();
        self.entity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
    struct TestView {
        id: String,
        counter: i32,
    }

    impl ProjectionSchema for TestView {
        const PREFIX: &'static str = "test";
        fn id(&self) -> &str {
            &self.id
        }
    }

    #[test]
    fn new_projection_is_dirty() {
        let view = TestView {
            id: "1".into(),
            counter: 0,
        };
        let proj = Projection::new(view);

        assert!(proj.is_dirty());
        assert_eq!(proj.key(), "test:1");
        assert_eq!(proj.data().counter, 0);
    }

    #[test]
    fn sync_writes_snapshot() {
        let view = TestView {
            id: "1".into(),
            counter: 42,
        };
        let mut proj = Projection::new(view);
        proj.sync();

        assert!(!proj.is_dirty());
        assert_eq!(proj.entity.events().len(), 1);
        assert_eq!(proj.entity.events()[0].event_name, "Snapshot");
    }

    #[test]
    fn from_entity_loads_snapshot() {
        let view = TestView {
            id: "1".into(),
            counter: 99,
        };
        let mut proj = Projection::new(view);
        proj.sync();

        let entity = proj.entity.clone();
        let loaded: Projection<TestView> = Projection::from_entity(entity).unwrap();

        assert!(!loaded.is_dirty());
        assert_eq!(loaded.data().counter, 99);
    }

    #[test]
    fn data_mut_marks_dirty() {
        let view = TestView {
            id: "1".into(),
            counter: 0,
        };
        let mut proj = Projection::new(view);
        proj.sync();
        assert!(!proj.is_dirty());

        proj.data_mut().counter = 100;
        assert!(proj.is_dirty());
    }
}
