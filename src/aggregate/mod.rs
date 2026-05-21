mod aggregate;

pub use aggregate::{
    hydrate, Aggregate, AggregateBuilder, AggregateRepository, CommitAggregate, GetAggregate,
    GetAllAggregates, RepositoryExt,
};
