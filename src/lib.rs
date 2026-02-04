// Allow proc-macros to reference this crate by name even when used internally
extern crate self as sourced_rust;

pub mod core;
#[cfg(feature = "emitter")]
pub mod emitter;
#[cfg(feature = "bus")]
pub mod bus;
mod hashmap;
mod outbox;
mod outbox_worker;
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

// Outbox: commit concerns (atomic aggregate + outbox commit)
pub use outbox::{
    OutboxCommit, OutboxCommitExt,
    OutboxMessage, OutboxMessageStatus,
};

// Outbox Worker: drain and publish concerns
pub use outbox_worker::{
    // Repository extension for claiming/completing messages
    OutboxRepositoryExt,
    // Publishers
    LogPublisher, LogPublisherError, OutboxPublisher,
    // Worker
    DrainResult, OutboxWorker, ProcessOneResult,
};

// Threaded outbox worker (requires bus feature)
#[cfg(feature = "bus")]
pub use outbox_worker::{OutboxWorkerThread, WorkerStats};

// In-memory queue for testing and development (requires bus feature)
#[cfg(feature = "bus")]
pub use bus::InMemoryQueue;

// LocalEmitterPublisher requires the emitter feature
#[cfg(feature = "emitter")]
pub use outbox_worker::LocalEmitterPublisher;

pub use queued::{
    // Queued repository
    Queueable, QueuedRepository,
    // WithOpts traits for opting out of locking
    FindOneWithOpts, FindWithOpts, GetAllWithOpts, GetWithOpts, ReadOpts,
};

// Re-export the EventEmitter from the event_emitter_rs crate (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use event_emitter_rs::EventEmitter;

// Re-export proc macros
pub use sourced_rust_macros::{aggregate, digest};

// Re-export enqueue macro (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use sourced_rust_macros::enqueue;
