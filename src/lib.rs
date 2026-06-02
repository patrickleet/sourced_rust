#![allow(clippy::module_inception)]

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
    AsyncLock, AsyncLockManager, InMemoryAsyncLock, InMemoryAsyncLockFuture,
    InMemoryAsyncLockManager, LockError,
};
// Durable SQLx lease-table lock managers (feature-gated like the SQLx repos).
#[cfg(feature = "postgres")]
pub use lock::{PostgresLock, PostgresLockManager};
#[cfg(feature = "sqlite")]
pub use lock::{SqliteLock, SqliteLockManager};

// Outbox: commit concerns (aggregate + outbox in one commit)
pub use outbox::{
    outbox_message_insert_plan, outbox_message_key, outbox_message_row_values,
    outbox_message_schema, OutboxCommit, OutboxMessage, OutboxMessageStatus, OUTBOX_MESSAGES_TABLE,
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

// LocalEmitterPublisher requires the emitter feature
#[cfg(feature = "emitter")]
pub use outbox_worker::LocalEmitterPublisher;
pub use outbox_worker::{
    OutboxDispatchOutcome, OutboxDispatcher, OutboxSource, ReceivedOutboxMessage,
    DEFAULT_OUTBOX_SOURCE_BATCH, DEFAULT_OUTBOX_SOURCE_LEASE, SOURCED_METADATA_PREFIX,
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

// CommitBuilder: transactional batches of read models, outbox, and aggregates
pub use commit_builder::{
    CommitBuilder, CommitBuilderExt, ReadModelWritePlanCommitExt, StagedCommitBuilder,
};

// Snapshot: state snapshot payloads and rebuildable cache records for hydration
pub use snapshot::{
    hydrate_from_snapshot, InMemorySnapshotStore, SnapshotAggregateRepository, SnapshotRecord,
    Snapshottable,
};

// Re-export the EventEmitter from the event_emitter_rs crate (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use event_emitter_rs::EventEmitter;

// Re-export proc macros
pub use distributed_macros::{aggregate, digest, sourced, ReadModel, Snapshot};

// Re-export enqueue macro (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use distributed_macros::enqueue;
