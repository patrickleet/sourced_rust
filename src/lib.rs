// Allow proc-macros to reference this crate by name even when used internally
extern crate self as sourced_rust;

pub mod aggregate;
pub mod entity;
pub mod repository;

#[cfg(feature = "bus")]
pub mod bus;
mod commit_builder;
#[cfg(feature = "emitter")]
pub mod emitter;
mod hashmap_repo;
pub mod lock;
pub mod microsvc;
mod outbox;
mod outbox_worker;
pub mod queued_repo;
pub mod read_model;
pub mod snapshot;

// Re-export entity types at crate root for convenience
pub use entity::{
    upcast_events, BitcodePayloadCodec, Committable, Entity, Event, EventRecord, EventRecordError,
    EventUpcaster, LocalEvent, PayloadCodec, UpcastError, BITCODE_PAYLOAD_CODEC,
    BITCODE_PAYLOAD_CODEC_VERSION,
};

pub type SourcedResult<T = ()> = std::result::Result<T, EventRecordError>;

// Re-export repository traits at crate root for convenience
pub use repository::{
    Commit, CommitBatch, Count, Exists, Find, FindOne, Get, GetMany, GetOne, Gettable,
    ReadModelWrite, Repository, RepositoryError, SnapshotWrite, TransactionalCommit,
};

// Re-export aggregate types at crate root for convenience
pub use aggregate::{
    hydrate, Aggregate, AggregateBuilder, AggregateRepository, CommitAggregate, CountAggregate,
    ExistsAggregate, FindAggregate, FindOneAggregate, GetAggregate, GetAllAggregates,
    RepositoryExt,
};

pub use hashmap_repo::HashMapRepository;

// Re-export lock traits and types at crate root for convenience
pub use lock::{InMemoryLock, InMemoryLockManager, Lock, LockError, LockManager};

// Outbox: commit concerns (aggregate + outbox in one commit)
pub use outbox::{OutboxCommit, OutboxCommitExt, OutboxMessage, OutboxMessageStatus};

// Outbox Worker: drain and publish concerns
pub use outbox_worker::{
    // Worker
    DrainResult,
    // Publishers
    LogPublisher,
    LogPublisherError,
    OutboxPublisher,
    // Repository extension for claiming/completing messages
    OutboxRepositoryExt,
    OutboxWorker,
    ProcessOneResult,
};

// Threaded outbox worker (requires bus feature)
#[cfg(feature = "bus")]
pub use outbox_worker::{OutboxWorkerThread, WorkerStats};

// In-memory queue for testing and development (requires bus feature)
#[cfg(feature = "bus")]
pub use bus::InMemoryQueue;

// Message alias for command contexts (requires bus feature)
#[cfg(feature = "bus")]
pub use bus::Message;

// LocalEmitterPublisher requires the emitter feature
#[cfg(feature = "emitter")]
pub use outbox_worker::LocalEmitterPublisher;

pub use queued_repo::{
    // WithOpts traits for opting out of locking
    FindOneWithOpts,
    FindWithOpts,
    GetAllWithOpts,
    GetWithOpts,
    // Queued repository
    Queueable,
    QueuedRepository,
    ReadOpts,
};

// Read models: projections and read-optimized views
pub use read_model::{
    InMemoryReadModelStore, QueuedReadModelStore, ReadModel, ReadModelError, ReadModelStore,
    ReadModelsExt, Versioned,
};

// CommitBuilder: transactional batches of read models, outbox, and aggregates
pub use commit_builder::{CommitBuilder, CommitBuilderExt};

// Snapshot: periodic aggregate snapshots for fast hydration
pub use snapshot::{
    hydrate_from_snapshot, InMemorySnapshotStore, SnapshotAggregateRepository, SnapshotRecord,
    SnapshotStore, Snapshottable,
};

// Re-export the EventEmitter from the event_emitter_rs crate (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use event_emitter_rs::EventEmitter;

// Re-export proc macros
pub use sourced_rust_macros::{aggregate, digest, sourced, ReadModel, Snapshot};

// Re-export enqueue macro (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use sourced_rust_macros::enqueue;
