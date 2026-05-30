mod aggregate;
mod async_aggregate;

pub use aggregate::{hydrate, Aggregate};
pub use async_aggregate::{AsyncAggregateBuilder, AsyncAggregateRepository};
