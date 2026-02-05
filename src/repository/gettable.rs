use crate::entity::Entity;
use super::RepositoryError;

/// Trait for types that can be used as get arguments.
pub trait Gettable {
    type Output;
    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError>;
}

/// Internal trait for getting a single entity.
pub trait GetOne {
    fn get_one(&self, id: &str) -> Result<Option<Entity>, RepositoryError>;
}

/// Internal trait for getting multiple entities.
pub trait GetMany {
    fn get_many(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError>;
}

// Single ID (&str)
impl Gettable for &str {
    type Output = Option<Entity>;

    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError> {
        repo.get_one(self)
    }
}

// Single ID (String)
impl Gettable for String {
    type Output = Option<Entity>;

    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError> {
        repo.get_one(self.as_str())
    }
}

// Single ID (&String)
impl Gettable for &String {
    type Output = Option<Entity>;

    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError> {
        repo.get_one(self.as_str())
    }
}

// Slice of &str
impl<'a> Gettable for &[&'a str] {
    type Output = Vec<Entity>;

    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError> {
        repo.get_many(self)
    }
}

// Fixed-size arrays
impl<'a, const N: usize> Gettable for [&'a str; N] {
    type Output = Vec<Entity>;

    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError> {
        repo.get_many(self.as_slice())
    }
}

// Fixed-size arrays by reference
impl<'a, const N: usize> Gettable for &[&'a str; N] {
    type Output = Vec<Entity>;

    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError> {
        repo.get_many(self.as_slice())
    }
}

// Vec of &str
impl<'a> Gettable for Vec<&'a str> {
    type Output = Vec<Entity>;

    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError> {
        repo.get_many(self.as_slice())
    }
}

// Vec of &str by reference
impl<'a> Gettable for &Vec<&'a str> {
    type Output = Vec<Entity>;

    fn get_from<R: GetOne + GetMany>(&self, repo: &R) -> Result<Self::Output, RepositoryError> {
        repo.get_many(self.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockRepo;

    impl GetOne for MockRepo {
        fn get_one(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
            Ok(Some(Entity::with_id(id)))
        }
    }

    impl GetMany for MockRepo {
        fn get_many(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
            Ok(ids.iter().map(|id| Entity::with_id(*id)).collect())
        }
    }

    #[test]
    fn single_str() {
        let repo = MockRepo;
        let result: Option<Entity> = "test-id".get_from(&repo).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id(), "test-id");
    }

    #[test]
    fn single_string() {
        let repo = MockRepo;
        let id = String::from("test-id");
        let result: Option<Entity> = id.get_from(&repo).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id(), "test-id");
    }

    #[test]
    fn slice_of_ids() {
        let repo = MockRepo;
        let ids: &[&str] = &["a", "b", "c"];
        let result: Vec<Entity> = ids.get_from(&repo).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn array_of_ids() {
        let repo = MockRepo;
        let ids = ["a", "b"];
        let result: Vec<Entity> = ids.get_from(&repo).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn vec_of_ids() {
        let repo = MockRepo;
        let ids = vec!["a", "b", "c", "d"];
        let result: Vec<Entity> = ids.get_from(&repo).unwrap();
        assert_eq!(result.len(), 4);
    }
}
