//! Counter repository for testing projections.

use sourced_rust::{AggregateBuilder, AggregateRepository, HashMapRepository};
use std::ops::{Deref, DerefMut};

use super::aggregate::Counter;

type Inner = AggregateRepository<HashMapRepository, Counter>;

/// Repository for Counter aggregates with projection support.
pub struct CounterRepository {
    inner: Inner,
}

impl CounterRepository {
    pub fn new() -> Self {
        Self {
            inner: HashMapRepository::new().aggregate::<Counter>(),
        }
    }

    /// Access the underlying repository for projection and commit operations.
    pub fn repo(&self) -> &HashMapRepository {
        self.inner.repo()
    }
}

impl Deref for CounterRepository {
    type Target = Inner;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for CounterRepository {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
