//! Domain Service integration tests.
//!
//! Demonstrates the `DomainService` pattern for command handling:
//! - Register named command handlers
//! - Dispatch messages to handlers
//! - Background service thread listening on a queue

mod support;
mod basic;
mod threaded;
#[cfg(feature = "http")]
mod http;
