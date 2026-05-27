mod service;

pub mod handlers;

pub use service::{projects, service, ProjectionDependencies};

pub const CHECKOUT_SCREEN_CONSUMER: &str = "checkout-screen-projection";
