//! microsvc integration tests.

mod support;
mod handlers;
mod basic;
mod session;
mod convention;
mod transport_listen;
mod transport_subscribe;

#[cfg(feature = "http")]
mod transport_http;
