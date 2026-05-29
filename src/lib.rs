#![allow(clippy::module_inception)]

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
    AsyncCommitBatch, AsyncGetStream, AsyncInboxStore, AsyncReadModelWritePlanStore,
    AsyncRelationalReadModelQueryStore, AsyncRepository, AsyncSnapshotStore, AsyncSnapshotWrite,
    AsyncStreamWrite, AsyncTransactionalCommit, Commit, CommitBatch, Get, GetMany, GetOne,
    Gettable, InboxOutcome, InboxReceipt, PreparedEventAppend, Repository, RepositoryError,
    SnapshotWrite, StreamIdentity, TransactionalCommit,
};

// Re-export aggregate types at crate root for convenience
pub use aggregate::{
    hydrate, Aggregate, AggregateBuilder, AggregateRepository, AsyncAggregateBuilder,
    AsyncAggregateRepository, CommitAggregate, GetAggregate, GetAllAggregates, RepositoryExt,
};

pub use hashmap_repo::{HashMapOutboxStore, HashMapRepository};
#[cfg(feature = "postgres")]
pub use postgres_repo::{PostgresOutboxStore, PostgresRepository};
#[cfg(feature = "sqlite")]
pub use sqlite_repo::{SqliteOutboxStore, SqliteRepository};

// Re-export lock traits and types at crate root for convenience
pub use lock::{InMemoryLock, InMemoryLockManager, Lock, LockError, LockManager};

// Outbox: commit concerns (aggregate + outbox in one commit)
pub use outbox::{
    outbox_message_insert_plan, outbox_message_key, outbox_message_row_values,
    outbox_message_schema, AsyncOutboxCommit, OutboxMessage, OutboxMessageStatus, SyncOutboxCommit,
    SyncOutboxCommitExt, OUTBOX_MESSAGES_TABLE,
};

// Outbox Worker: drain and publish concerns
pub use outbox_worker::{
    AsyncOutboxStore,
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

// Threaded outbox worker (requires bus feature)
#[cfg(feature = "bus")]
pub use outbox_worker::{OutboxWorkerJoinError, OutboxWorkerThread, WorkerStats};

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
    GetAllWithOpts,
    GetWithOpts,
    // Queued repository
    Queueable,
    QueuedRepository,
    ReadOpts,
};

// Read models: projections and read-optimized views
pub use read_model::{
    ColumnDef, ColumnType, DeleteRowMutation, ExpectedVersion, ForeignKey, InMemoryReadModelStore,
    IndexDef, PatchMode, PatchRowMutation, PrimaryKey, ReadModel, ReadModelAdapterCapabilities,
    ReadModelCommitOutcome, ReadModelError, ReadModelIncludeRows, ReadModelLoadGraph,
    ReadModelLoadRequest, ReadModelMigrationArtifact, ReadModelMutation,
    ReadModelQueryCapabilities, ReadModelSchema, ReadModelSchemaAdapter,
    ReadModelSchemaAdapterCapabilities, ReadModelSchemaBootstrap, ReadModelSchemaIssue,
    ReadModelSchemaIssueKind, ReadModelSchemaRegistry, ReadModelSchemaVerification,
    ReadModelWorkspace, ReadModelWorkspaceExt, ReadModelWritePlan, ReadModelWritePlanBuilder,
    ReadModelWritePlanStore, RelationalReadModel, RelationalReadModelIncludes,
    RelationalReadModelQueryStore, RelationshipDef, RelationshipKind, RowKey, RowMutation,
    RowPatch, RowValue, RowValues, RowWriteMode, Versioned, DEFAULT_READ_MODEL_VERSION_COLUMN,
};

// Neutral table/row primitives shared by read models and operational tables.
pub use table::{
    generate_table_migration_artifacts, table_schema_bootstrap_result, table_schema_statements,
    DeleteTableRowMutation, PatchTableRowMutation, TableAdapterCapabilities, TableColumn,
    TableCommitOutcome, TableIndex, TableMigrationArtifact, TableModel, TableMutation,
    TableRowMutation, TableSchema, TableSchemaAdapter, TableSchemaAdapterCapabilities,
    TableSchemaBootstrap, TableSchemaIssue, TableSchemaIssueKind, TableSchemaRegistry,
    TableSchemaRegistryExt, TableSchemaVerification, TableSqlDialect, TableSqlSchemaAdapter,
    TableStoreError, TableWritePlan, DEFAULT_TABLE_VERSION_COLUMN,
};

// SyncCommitBuilder: transactional batches of read models, outbox, and aggregates
pub use commit_builder::{
    AsyncCommitBuilder, AsyncCommitBuilderExt, AsyncReadModelWritePlanCommitExt,
    AsyncStagedCommitBuilder, SyncCommitBuilder, SyncCommitBuilderExt,
    SyncReadModelWritePlanCommitExt, SyncStagedCommitBuilder,
};

// Snapshot: state snapshot payloads and rebuildable cache records for hydration
pub use snapshot::{
    hydrate_from_snapshot, AsyncSnapshotAggregateRepository, InMemorySnapshotStore,
    SnapshotAggregateRepository, SnapshotRecord, SnapshotStore, Snapshottable,
};

// Re-export the EventEmitter from the event_emitter_rs crate (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use event_emitter_rs::EventEmitter;

// Re-export proc macros
pub use sourced_rust_macros::{aggregate, digest, sourced, ReadModel, Snapshot};

// Re-export enqueue macro (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use sourced_rust_macros::enqueue;
