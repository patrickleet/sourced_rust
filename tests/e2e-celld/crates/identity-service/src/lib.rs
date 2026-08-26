//! Identity service crate: Zitadel ingress/scrape + AuthUsers projection.
//!
//! Not a cell. Chat and Blob stay in their own crates; this only owns IdP
//! import so AuthUsers joins are not mounted on the chat aggregate.

mod aggregate;
mod bounds;
mod deps;
pub mod handlers;
mod routes;

pub use aggregate::Identity;
pub use handlers::ingestors::zitadel::{
    scrape_users_to_outbox, spawn_scrape_loop, ScrapeReport, ZitadelScrapeConfig,
};
pub use routes::{routes, MODULE_ID};
