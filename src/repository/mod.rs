mod error;
mod gettable;
mod repository;

pub use error::RepositoryError;
pub use gettable::{GetMany, GetOne, Gettable};
pub use repository::{Commit, Count, Exists, Find, FindOne, Get, Repository};
