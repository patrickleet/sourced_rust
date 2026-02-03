mod entity;
mod error;
mod event;
mod event_record;
mod hashmap;
mod local_event;
mod outbox;
mod queued;
mod repository;

pub use entity::Entity;
pub use error::RepositoryError;
pub use event::Event;
pub use event_record::EventRecord;
pub use hashmap::HashMapRepository;
pub use local_event::LocalEvent;
pub use outbox::{
    LocalEmitterPublisher, LogPublisher, LogPublisherError, OutboxDelivery,
    OutboxDeliveryResult, OutboxPublisher, OutboxRecord, OutboxRepository, OutboxStatus,
    OutboxWorker, Outboxable,
};
pub use queued::{Queueable, QueuedRepository};
pub use repository::Repository;

// Re-export the EventEmitter from the event_emitter_rs crate
pub use event_emitter_rs::EventEmitter;

// Re-export any other types or functions that should be part of the public API
