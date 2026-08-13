//! Prometheus-compatible framework metrics.
//!
//! This module intentionally has no SDK dependency. It records the framework
//! counters and gauges Distributed owns, then renders Prometheus text exposition
//! for HTTP scrape endpoints.

mod api;

#[cfg(feature = "http")]
mod http;
mod prometheus;
mod registry;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;

pub use api::{
    describe_service, prometheus_text, record_graphql_request, record_microsvc_dispatch,
    record_outbox_message, record_outbox_messages, record_transport_failure,
    record_transport_message, set_outbox_backlog,
};

#[cfg(feature = "http")]
pub use http::{http_router, http_router_for_service, prometheus_response, serve_http};

#[cfg(test)]
pub(crate) use test_support::{async_lock_for_tests, lock_for_tests, reset_for_tests};
