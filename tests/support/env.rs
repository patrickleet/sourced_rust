//! Broker/service env-var skip guard, shared across test targets via
//! `#[path = "../support/env.rs"]`.
#![allow(dead_code)] // not every including target consumes it under all features

/// Read `var`, printing a "skipping {what}" notice and returning `None` when it
/// is unset — the convention that lets broker suites pass without services.
pub fn broker_env(var: &str, what: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(value) => Some(value),
        Err(_) => {
            eprintln!("skipping {what}: {var} is not set");
            None
        }
    }
}
