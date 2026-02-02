use crate::entity::Entity;
use crate::error::RepositoryError;

// Repository trait
pub trait Repository {
    fn get(&self, id: &str) -> Result<Option<Entity>, RepositoryError>;
    fn commit(&self, entity: &mut Entity) -> Result<(), RepositoryError>;
    fn get_all(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError>;
    fn commit_all(&self, entities: &mut [&mut Entity]) -> Result<(), RepositoryError>;
}
