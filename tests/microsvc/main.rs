//! microsvc integration tests.

mod support;
mod handlers;
mod basic;
mod session;
mod convention;
mod transport;

#[cfg(feature = "http")]
mod http;
