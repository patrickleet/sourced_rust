#![allow(clippy::module_inception)]
#![doc = include_str!("../README.md")]

// Allow proc-macros to reference this crate by name even when used internally
extern crate self as distributed;

pub mod aggregate;
pub mod bus;
pub mod entity;
pub mod repository;

mod commit_builder;
#[cfg(feature = "emitter")]
pub mod emitter;
mod hashmap_repo;
pub mod lock;
pub mod manifest;
pub mod microsvc;
pub mod outbox;
pub mod outbox_worker;
#[cfg(feature = "postgres")]
pub mod postgres_repo;
pub mod queued_repo;
pub mod read_model;
pub mod snapshot;
#[cfg(feature = "sqlite")]
pub mod sqlite_repo;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod sqlx_repo;
pub mod table;

// Re-export entity types at crate root for convenience
pub use entity::{
    upcast_events, upcast_payload, BitcodePayloadCodec, Committable, Entity, Event, EventRecord,
    EventRecordError, EventUpcaster, LocalEvent, PayloadCodec, UpcastError, BITCODE_PAYLOAD_CODEC,
    BITCODE_PAYLOAD_CODEC_VERSION,
};

pub type SourcedResult<T = ()> = std::result::Result<T, EventRecordError>;

// Re-export repository traits at crate root for convenience
pub use repository::{
    CommitBatch, GetStream, InboxOutcome, InboxReceipt, InboxStore, PreparedEventAppend,
    ReadModelWritePlanStore, RelationalReadModelQueryStore, Repository, RepositoryError,
    SnapshotStore, SnapshotWrite, StreamIdentity, StreamWrite, TransactionalCommit,
};

// Re-export aggregate types at crate root for convenience
pub use aggregate::{hydrate, Aggregate, AggregateBuilder, AggregateRepository};

pub use hashmap_repo::{HashMapOutboxStore, HashMapRepository};
#[cfg(feature = "postgres")]
pub use postgres_repo::{PostgresOutboxStore, PostgresRepository};
#[cfg(feature = "sqlite")]
pub use sqlite_repo::{SqliteOutboxStore, SqliteRepository};

// Re-export lock traits and types at crate root for convenience
pub use lock::{
    InMemoryLock, InMemoryLockFuture, InMemoryLockManager, Lock, LockError, LockManager,
};
// Durable SQLx lease-table lock managers (feature-gated like the SQLx repos).
#[cfg(feature = "postgres")]
pub use lock::{PostgresLock, PostgresLockManager};
#[cfg(feature = "sqlite")]
pub use lock::{SqliteLock, SqliteLockManager};

// Outbox: commit concerns (aggregate + outbox in one commit).
//
// Low-level row plumbing (`outbox_message_insert_plan`, `outbox_message_row_values`)
// stays reachable under `distributed::outbox::*` and is intentionally NOT re-exported
// at the crate root — it is an adapter-authoring detail, not part of the quick-start API.
pub use outbox::{
    outbox_message_key, outbox_message_schema, AggregateCommit, CommitReceipt, OutboxMessage,
    OutboxMessageStatus, OutboxPublishHook, OutboxPublisherConfig, OUTBOX_MESSAGES_TABLE,
};

// Outbox Worker: drain and publish concerns
pub use outbox_worker::{
    ClaimOutboxMessages,
    // Worker
    DrainResult,
    // Publishers
    LogPublisher,
    LogPublisherError,
    OutboxClaimRef,
    OutboxPublishFailureAction,
    OutboxPublisher,
    OutboxStore,
    OutboxWorker,
    ProcessOneResult,
};

// LocalEmitterPublisher requires the emitter feature
#[cfg(feature = "emitter")]
pub use outbox_worker::LocalEmitterPublisher;
pub use outbox_worker::{
    BusOutboxPublishHook, BusPublisher, OutboxDispatchOutcome, OutboxDispatcher, OutboxSource,
    ReceivedOutboxMessage, DEFAULT_OUTBOX_SOURCE_BATCH, DEFAULT_OUTBOX_SOURCE_LEASE,
    SOURCED_METADATA_PREFIX,
};

pub use queued_repo::{
    // WithOpts + unlock traits for the queued repository variant.
    GetAllWithOpts,
    GetWithOpts,
    // Queued repository
    Queueable,
    QueuedRepository,
    ReadOpts,
    UnlockableRepository,
};

// Read models: projections and read-optimized views
pub use read_model::{
    ColumnDef, ColumnType, DeleteRowMutation, ExpectedVersion, ForeignKey, InMemoryReadModelStore,
    IndexDef, PatchMode, PatchRowMutation, PrimaryKey, ReadModel, ReadModelAdapterCapabilities,
    ReadModelCommitOutcome, ReadModelError, ReadModelIncludeRows, ReadModelLoadBuilder,
    ReadModelLoadGraph, ReadModelLoadRequest, ReadModelMigrationArtifact, ReadModelMutation,
    ReadModelQueryCapabilities, ReadModelSchema, ReadModelSchemaAdapter,
    ReadModelSchemaAdapterCapabilities, ReadModelSchemaBootstrap, ReadModelSchemaIssue,
    ReadModelSchemaIssueKind, ReadModelSchemaRegistry, ReadModelSchemaVerification,
    ReadModelWorkspace, ReadModelWorkspaceExt, ReadModelWritePlan, ReadModelWritePlanBuilder,
    RelationalReadModel, RelationalReadModelIncludes, RelationshipDef, RelationshipKind, RowKey,
    RowMutation, RowPatch, RowValue, RowValues, RowWriteMode, Versioned,
    DEFAULT_READ_MODEL_VERSION_COLUMN,
};

// Neutral table/row primitives shared by read models and operational tables stay
// reachable under their module path (`distributed::table::*`). They are low-level
// schema/adapter plumbing, not part of the quick-start surface, so they are not
// re-exported at the crate root.
//
// Exception: `TableSchemaRegistry` is the entry point for registering operational
// table schemas (it is what callers build before bootstrapping migrations), so it
// is consumed directly by downstream users and integration tests. It stays at the
// crate root as part of the public surface.
pub use table::TableSchemaRegistry;

pub use manifest::{
    DistributedManifestEnvelope, DistributedProjectManifest, MessageEndpointManifest,
    ServiceManifest, TransportManifest, DISTRIBUTED_MANIFEST_SCHEMA_VERSION,
};

// CommitBuilder: transactional batches of read models, outbox, and aggregates
pub use commit_builder::{
    CommitBuilder, CommitBuilderExt, ReadModelWritePlanCommitExt, StagedCommitBuilder,
};

// Snapshot: state snapshot payloads and rebuildable cache records for hydration
pub use snapshot::{hydrate_from_snapshot, InMemorySnapshotStore, SnapshotRecord, Snapshottable};

// Re-export the EventEmitter from the event_emitter_rs crate (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use event_emitter_rs::EventEmitter;

// Re-export proc macros
pub use distributed_macros::{aggregate, digest, sourced, ReadModel, Snapshot};

// Re-export enqueue macro (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use distributed_macros::enqueue;
