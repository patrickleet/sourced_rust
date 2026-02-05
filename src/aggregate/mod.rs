mod aggregate;

pub use aggregate::{
    hydrate, Aggregate, AggregateBuilder, AggregateRepository, CommitAggregate, CountAggregate,
    ExistsAggregate, FindAggregate, FindOneAggregate, GetAggregate, GetAllAggregates,
    GetAllWithOpts, GetWithOpts, ReadOpts, RepositoryExt, UnlockableRepository,
};
