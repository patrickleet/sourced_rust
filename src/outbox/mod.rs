mod aggregate;
mod outbox_entity;
mod publisher;
mod repository;
mod worker;

// Event-sourced Outbox aggregate
pub use aggregate::{Outbox, OutboxMessage, OutboxMessageStatus};

// OutboxEntity wrapper
pub use outbox_entity::{HasOutbox, OutboxEntity, OutboxEvent};

// OutboxRepository wrapper
pub use repository::{OutboxRepository, WithOutbox};

// Publishers
pub use publisher::{LocalEmitterPublisher, LogPublisher, LogPublisherError, OutboxPublisher};

// Worker
pub use worker::{DrainResult, OutboxWorker, ProcessOneResult};
