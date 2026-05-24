mod service;

pub mod handlers;

pub use service::{projects, service, subscriber};

pub const CHECKOUT_SCREEN_CONSUMER: &str = "checkout-screen-projection";
