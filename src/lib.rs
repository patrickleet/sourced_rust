#![allow(clippy::module_inception)]
#![doc = include_str!("../README.md")]
// Projection + GraphQL client surfaces always compile (dctl / shared types), but many
// call sites live behind optional features. Without those features rustc reports
// false "never used" warnings for the protocol store helpers. CI builds with features.
#![cfg_attr(
    not(any(feature = "graphql", feature = "sqlite", feature = "postgres", test)),
    allow(dead_code)
)]

// Allow proc-macros to reference this crate by name even when used internally
extern crate self as distributed;

pub mod aggregate;
pub mod bus;
pub mod domain_event;
pub mod entity;
pub mod repository;

pub(crate) mod command_ledger;
mod commit_builder;
#[cfg(feature = "emitter")]
pub mod emitter;
pub mod graphql;
mod in_memory_repo;
pub mod lock;
pub mod manifest;
#[cfg(feature = "metrics")]
pub mod metrics;
pub mod microsvc;
pub mod outbox;
pub mod outbox_worker;
#[cfg(feature = "postgres")]
pub mod postgres_repo;
pub mod projection;
pub mod projection_protocol;
pub mod queued_repo;
pub mod read_model;
pub mod snapshot;
#[cfg(feature = "sqlite")]
pub mod sqlite_repo;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod sqlx_repo;
pub mod table;
mod telemetry;
pub mod trace_context;

// Re-export entity types at crate root for convenience
pub use entity::{
    upcast_events, upcast_events_for_replay, upcast_payload, BitcodePayloadCodec, Entity,
    EventRecord, EventRecordError, EventUpcaster, PayloadCodec, UpcastError, BITCODE_PAYLOAD_CODEC,
    BITCODE_PAYLOAD_CODEC_VERSION,
};

// Domain events: typed outward contracts distinct from replay events/snapshots.
pub use domain_event::{
    DomainDeletion, DomainDeletionError, DomainEvent, DomainEventBodyDescriptor,
    DomainEventBodyKind, DomainEventCaptureError, DomainEventCaptureOutcome,
    DomainEventCapturePoison, DomainEventCommitGuardError, DomainEventDescriptor,
    DomainEventEnvelope, DomainEventOccurrence, DomainState, DomainStateDescriptor,
    DOMAIN_EVENT_BODY_CODEC, DOMAIN_EVENT_BODY_CODEC_VERSION, DOMAIN_EVENT_OCCURRENCE_VERSION,
    MAX_DOMAIN_EVENT_BODY_BYTES, MAX_DOMAIN_EVENT_OCCURRENCE_WIRE_BYTES,
};

// Logical projection contracts. Physical read-model lowering deliberately lives
// behind adapters and is not part of this semantic surface.
pub use projection::{
    ProjectionArm, ProjectionAssignment, ProjectionEnvelopeField, ProjectionEventSelector,
    ProjectionEventSet, ProjectionExpression, ProjectionField, ProjectionInvalidation,
    ProjectionKeyField, ProjectionMutationKind, ProjectionMutationProvenance,
    ProjectionObjectValueField, ProjectionOccurrenceProvenance, ProjectionOperation,
    ProjectionPartition, ProjectionPlanTemplate, ProjectionProgram, ProjectionProgramError,
    ProjectionProgramId, ProjectionProgramLimits, ProjectionRelationship,
    ProjectionRelationshipEffect, ProjectionRelationshipEffectKind, ProjectionScalarTransform,
    ProjectionTarget, ProjectionValue, ProjectionValueRef, ProjectionValueType,
    ResolvedProjectionKey, ResolvedProjectionMutation, ResolvedProjectionMutationScope,
    ResolvedProjectionPartition, ResolvedProjectionPartitionRef, ResolvedProjectionPlan,
    ResolvedProjectionRelationshipEffect, ResolvedProjectionValue, MAX_PROJECTION_EXPRESSION_DEPTH,
    MAX_PROJECTION_OPERATIONS_PER_OCCURRENCE, MAX_PROJECTION_PATH_SEGMENTS,
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

pub use in_memory_repo::{InMemoryOutboxStore, InMemoryRepository};
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

// Outbox Worker: drain and publish concerns.
//
// Adapter-authoring constants (`SOURCED_METADATA_PREFIX`,
// `DEFAULT_OUTBOX_SOURCE_BATCH`, `DEFAULT_OUTBOX_SOURCE_LEASE`) stay reachable
// under `distributed::outbox_worker::*` and are intentionally NOT re-exported
// at the crate root.
pub use outbox_worker::{
    BusOutboxPublishHook, BusPublisher, ClaimOutboxMessages, OutboxClaimRef, OutboxDispatchOutcome,
    OutboxDispatcher, OutboxPublishFailureAction, OutboxSource, OutboxStore, ReceivedOutboxMessage,
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

// Read models: projections and read-optimized views.
//
// Only the quick-start surface is re-exported at the crate root: the traits
// user models implement (incl. `RelationalReadModelIncludes`, which the
// `ReadModel` derive expands to), the in-memory default store, the
// workspace/plan entry points, and the version marker. Load-graph, query, and
// row-include plumbing stays reachable under `distributed::read_model::*`.
pub use read_model::{
    InMemoryReadModelStore, ReadModel, ReadModelChange, ReadModelWorkspaceExt,
    ReadModelWritePlanBuilder, RelationalReadModel, RelationalReadModelIncludes, Versioned,
};

// Neutral table/row primitives: the canonical schema, row, mutation, write-plan,
// and error vocabulary shared by read models and operational tables (outbox,
// inbox/checkpoint, and future operational tables). Read models build on these,
// so they are part of the crate-root surface; SQL rendering helpers stay under
// `distributed::table::*`.
pub use table::{
    ColumnType, DeleteTableRowMutation, ExpectedVersion, ForeignKey, PatchMode,
    PatchTableRowMutation, PrimaryKey, RelationshipDef, RelationshipKind, RowKey, RowPatch,
    RowValue, RowValues, RowWriteMode, TableAdapterCapabilities, TableColumn, TableCommitOutcome,
    TableIndex, TableKind, TableMigrationArtifact, TableModel, TableMutation, TableRowMutation,
    TableSchema, TableSchemaAdapter, TableSchemaAdapterCapabilities, TableSchemaBootstrap,
    TableSchemaIssue, TableSchemaIssueKind, TableSchemaRegistry, TableSchemaRegistryExt,
    TableSchemaVerification, TableStoreError, TableWritePlan, DEFAULT_TABLE_VERSION_COLUMN,
};

pub use manifest::{
    DistributedManifestEnvelope, DistributedProjectManifest, MessageEndpointManifest,
    MetricsEndpointManifest, ServiceManifest, ServiceObservabilityManifest, TraceExportMode,
    TracePropagationMode, TracingManifest, TransportManifest, DISTRIBUTED_MANIFEST_SCHEMA_VERSION,
};
pub use trace_context::{
    is_valid_traceparent, TraceContext, CAUSATION_ID, CORRELATION_ID, TRACEPARENT, TRACESTATE,
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

/// Register read models + permissions on a GraphQL engine builder.
///
/// ```ignore
/// let builder = graphql_models!(builder, orders, players);
/// // expands to builder.model::<orders::Model>(orders::permissions())...
/// ```
#[macro_export]
macro_rules! graphql_models {
    ($builder:expr, $($m:ident),+ $(,)?) => {
        $builder $( .model::<$m::Model>($m::permissions()) )+
    };
}

// Session convenience re-exports used by GraphQL permission filters.
pub use microsvc::{ROLE_KEY, USER_ID_KEY};

// Re-export proc macros
pub use distributed_macros::{
    aggregate, command_confirmations, command_effects, command_input_defaults, digest, sourced,
    DomainEvent, DomainState, GraphqlInput, GraphqlOutput, ReadModel, Snapshot,
};

// Re-export enqueue macro (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use distributed_macros::enqueue;
