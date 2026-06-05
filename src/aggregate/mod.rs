mod aggregate;
mod repository;

pub use aggregate::{hydrate, Aggregate};
pub(crate) use repository::SnapshotPolicy;
pub use repository::{AggregateBuilder, AggregateRepository};
