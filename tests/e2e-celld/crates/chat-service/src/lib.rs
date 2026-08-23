//! Chat + Zitadel identity-ingestor service crate (in-process).

mod bounds;
mod deps;
pub mod handlers;
mod routes;

pub use handlers::ingestors::zitadel::{
    scrape_users_to_outbox, spawn_scrape_loop, ScrapeReport, ZitadelScrapeConfig,
};
pub use routes::{routes, MODULE_ID};
