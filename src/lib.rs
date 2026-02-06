// Allow proc-macros to reference this crate by name even when used internally
extern crate self as sourced_rust;

pub mod aggregate;
pub mod entity;
pub mod repository;

#[cfg(feature = "emitter")]
pub mod emitter;
#[cfg(feature = "bus")]
pub mod bus;
mod commit_builder;
mod hashmap_repo;
pub(crate) mod lock;
pub mod read_model;
mod outbox;
mod outbox_worker;
pub mod queued_repo;

// Re-export entity types at crate root for convenience
pub use entity::{Committable, Entity, Event, EventRecord, LocalEvent, PayloadError};

// Re-export repository traits at crate root for convenience
pub use repository::{
    Commit, Count, Exists, Find, FindOne, Get, GetMany, GetOne, Gettable, Repository,
    RepositoryError,
};

// Re-export aggregate types at crate root for convenience
pub use aggregate::{
    hydrate, Aggregate, AggregateBuilder, AggregateRepository, CommitAggregate, CountAggregate,
    ExistsAggregate, FindAggregate, FindOneAggregate, GetAggregate, GetAllAggregates, RepositoryExt,
    UnlockableRepository,
};

pub use hashmap_repo::HashMapRepository;

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

pub use queued_repo::{
    // Queued repository
    Queueable, QueuedRepository,
    // WithOpts traits for opting out of locking
    FindOneWithOpts, FindWithOpts, GetAllWithOpts, GetWithOpts, ReadOpts,
};

// Read models: projections and read-optimized views
pub use read_model::{
    InMemoryReadModelStore, QueuedReadModelStore, ReadModel, ReadModelError, ReadModelStore,
    ReadModelsExt, Versioned,
};

// CommitBuilder: atomic commits of read models, outbox, and aggregates
pub use commit_builder::{CommitBuilder, CommitBuilderExt};

// Re-export the EventEmitter from the event_emitter_rs crate (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use event_emitter_rs::EventEmitter;

// Re-export proc macros
pub use sourced_rust_macros::{aggregate, digest, ReadModel};

// Re-export enqueue macro (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use sourced_rust_macros::enqueue;
