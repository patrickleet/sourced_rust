use super::committable::Committable;
use super::entity::Entity;
use super::error::RepositoryError;

/// Core repository trait for persisting entities.
///
/// # Examples
///
/// ```ignore
/// // Single entity
/// repo.commit(&mut entity)?;
///
/// // Multiple entities
/// repo.commit(&mut [&mut a, &mut b])?;
/// ```
pub trait Repository {
    /// Get an entity by ID.
    fn get(&self, id: &str) -> Result<Option<Entity>, RepositoryError>;

    /// Get multiple entities by IDs.
    fn get_all(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError>;

    /// Commit one or more entities atomically.
    ///
    /// Accepts single entities or slices via the `Committable` trait.
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError>;
}
