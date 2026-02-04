// Allow proc-macros to reference this crate by name even when used internally
extern crate self as sourced_rust;

pub mod core;
pub mod emitter;
mod hashmap;
mod outbox;
pub mod queued;

// Re-export core types at crate root for convenience
pub use core::{
    // Aggregate support
    hydrate, Aggregate, AggregateBuilder, AggregateRepository,
    // Aggregate extension traits
    CommitAggregate, CountAggregate, ExistsAggregate, FindAggregate, FindOneAggregate,
    GetAggregate, GetAllAggregates,
    // Entity/Event types
    Committable, Entity, Event, EventRecord, LocalEvent, PayloadError,
    // Repository traits
    Commit, Count, Exists, Find, FindOne, Get, GetMany, GetOne, Gettable, Repository,
    RepositoryError, RepositoryExt,
    UnlockableRepository,
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

pub use queued::{
    // Queued repository
    Queueable, QueuedRepository,
    // WithOpts traits for opting out of locking
    FindOneWithOpts, FindWithOpts, GetAllWithOpts, GetWithOpts, ReadOpts,
};

// Re-export the EventEmitter from the event_emitter_rs crate
pub use event_emitter_rs::EventEmitter;

// Re-export proc macros
pub use sourced_rust_macros::{aggregate, digest};
