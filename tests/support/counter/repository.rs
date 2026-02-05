//! Counter repository for testing projections.

use sourced_rust::{AggregateBuilder, AggregateRepository, HashMapRepository};
use std::ops::{Deref, DerefMut};

use super::aggregate::Counter;

type Inner = AggregateRepository<HashMapRepository, Counter>;

/// Repository for Counter aggregates.
/// Wraps HashMapRepository with typed aggregate access.
pub struct CounterRepository {
    inner: Inner,
    base: HashMapRepository,
}

impl CounterRepository {
    pub fn new() -> Self {
        let base = HashMapRepository::new();
        Self {
            inner: base.clone().aggregate::<Counter>(),
            base,
        }
    }

    /// Access the base repository for projections and other operations.
    pub fn base(&self) -> &HashMapRepository {
        &self.base
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
