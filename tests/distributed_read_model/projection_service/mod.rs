mod service;

pub mod handlers;

pub use service::{projects, service, subscriber, ProjectionDependencies};

pub const CHECKOUT_SCREEN_CONSUMER: &str = "checkout-screen-projection";
