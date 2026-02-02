mod entity;
mod error;
mod event;
mod event_record;
mod hashmap_repository;
mod local_event;
mod lock;
mod queued_repository;
mod repository;

pub use entity::Entity;
pub use error::RepositoryError;
pub use event::Event;
pub use event_record::EventRecord;
pub use hashmap_repository::HashMapRepository;
pub use local_event::LocalEvent;
pub use queued_repository::{Queueable, QueuedRepository};
pub use repository::Repository;

// Re-export the EventEmitter from the event_emitter_rs crate
pub use event_emitter_rs::EventEmitter;

// Re-export any other types or functions that should be part of the public API
