//! Simple counter example for projection tests.

mod aggregate;
mod repository;
mod views;

pub use aggregate::Counter;
pub use repository::CounterRepository;
pub use views::{CounterView, UserCountersIndex};
