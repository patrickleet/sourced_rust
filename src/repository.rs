use crate::entity::Entity;

// Repository trait
pub trait Repository {
    fn get(&self, id: &str) -> Option<Entity>;
    fn commit(&self, entity: &mut Entity) -> Result<(), String> ;
    fn get_all(&self, ids: &[&str]) -> Vec<Entity>;
    fn commit_all(&self, entities: &mut [&mut Entity]) -> Result<(), String>;
}
