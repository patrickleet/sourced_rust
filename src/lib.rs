pub mod core;
pub mod emitter;
mod hashmap;
mod outbox;
mod queued;

// Re-export core types at crate root for convenience
pub use core::{
    Aggregate, AggregateBuilder, AggregateRepository, ArgCountError, ArgParseError, Committable,
    Entity, Event, EventRecord, LocalEvent, PeekableRepository, Repository, RepositoryError,
    RepositoryExt, UnlockableRepository,
};

pub use hashmap::HashMapRepository;

pub use outbox::{
    // Core outbox message aggregate
    OutboxMessage, OutboxMessageStatus,
    // Commit helpers
    OutboxCommit, OutboxCommitExt, OutboxRepositoryExt,
    // Publishers
    LocalEmitterPublisher, LogPublisher, LogPublisherError, OutboxPublisher,
    // Worker
    DrainResult, OutboxWorker, ProcessOneResult,
};

pub use queued::{Queueable, QueuedRepository};

// Re-export the EventEmitter from the event_emitter_rs crate
pub use event_emitter_rs::EventEmitter;
