//! External ingress commands (provider webhooks / Actions / scrapes).
//!
//! These publish **provider** bus messages only; projectors map them into read models.

pub mod zitadel;
pub mod zitadel_scrape;
