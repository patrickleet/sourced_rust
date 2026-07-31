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
pub mod mutation;
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

// Event-independent mutation IR, portable handlers, and interpreters.
pub use mutation::{
    bind_delete_to_envelope_id, bind_event_to_mutation, bind_state_body_to_mutation,
    bind_state_events_to_mutation, body_bindings_for_model, body_field_binding,
    compile_portable_handlers, compose_event_preview, delete_by_pk_program_for_model,
    descriptor_from_factories, envelope_binding, inventory_single_model, lower_mutation_cache,
    lower_single_model, portable_binding, resolve_mutation_program, state_upsert_program_for_model,
    Mutation, MutationAssignment, MutationCacheEffect, MutationCacheProgram,
    MutationCacheVisibility, MutationConflictTarget, MutationEventBinding, MutationExpression,
    MutationField, MutationFieldCapability, MutationHandlerCatalog, MutationHandlerPlacement,
    MutationHandlerRegistration, MutationInputBinding, MutationKeyCapability, MutationKeyField,
    MutationKind, MutationOperation, MutationProgram, MutationProgramError, MutationProgramId,
    MutationProgramLimits, MutationReturning, MutationServerInterpreter, PortableHandler,
    ReadModelMutationCapabilities, ResolvedMutationValue, MAX_MUTATION_OPERATIONS,
    MUTATION_OPERATION_SEMANTICS_VERSION, MUTATION_PROGRAM_IR_VERSION,
};

/// Spec-shaped authoring: **mutations** + **event-first portable handlers**.
///
/// Declare which domain events apply which mutation (`on <events> apply …`).
/// The framework compiles that into the service mount. Dual-path projection IR
/// is an internal detail.
///
/// ```ignore
/// portable_handlers! {
///     pub const BLOB_GAMES: ProjectionDescriptor<DirectCandidate> = {
///         name: "project_blob",
///         version: 1,
///         epoch: "e2e-ui-blob-v2",
///         model: BlobGames,
///         on_state BlobGameState as "game" apply save_blob_game {
///             "blob.initialized", "blob.moved",
///         }
///     };
/// }
/// ```
#[macro_export]
macro_rules! portable_handlers {
    // Event-first: state-body events by name + DomainState type.
    (
        $vis:vis const $id:ident : $desc_ty:ty = {
            name: $name:literal,
            version: $version:expr,
            epoch: $epoch:literal,
            model: $model:ty,
            on_state $state:ty as $input_root:literal apply $mutation:path {
                $($event:literal),+ $(,)?
            }
            $(,)?
        } $(;)?
    ) => {
        $vis const $id: $desc_ty = {
            fn __handlers() -> ::core::result::Result<
                $crate::ProjectionProgram,
                $crate::ProjectionProgramError,
            > {
                let mutation = $mutation().program().clone();
                let mut handlers = ::std::vec::Vec::new();
                $(
                    {
                        let descriptor =
                            $crate::DomainEventDescriptor::state::<$state>($event, 1);
                        handlers.push(
                            $crate::bind_state_body_to_mutation::<$model>(
                                &descriptor,
                                mutation.clone(),
                                $input_root,
                            )
                            .map_err(|e| $crate::ProjectionProgramError::InvalidOperation {
                                operation: ::std::string::String::from($name),
                                reason: e.to_string(),
                            })?,
                        );
                    }
                )+
                $crate::compile_portable_handlers(
                    $name,
                    $version,
                    $crate::ProjectionPartition::Unit,
                    handlers,
                )
            }
            fn __resolve(
                occurrence: &$crate::DomainEventOccurrence,
            ) -> ::core::result::Result<
                $crate::ResolvedProjectionPlan,
                $crate::ProjectionProgramError,
            > {
                $crate::resolve_mutation_program(&__handlers()?, occurrence)
            }
            fn __lower(
                plan: &$crate::ResolvedProjectionPlan,
            ) -> ::core::result::Result<
                $crate::projection::lower::LoweredProjectionPlan,
                $crate::projection::lower::ProjectionLoweringError,
            > {
                $crate::lower_single_model::<$model>(plan)
            }
            fn __inventory() -> ::core::result::Result<
                $crate::projection::lower::ProjectionOutputInventory,
                $crate::projection::lower::ProjectionLoweringError,
            > {
                $crate::inventory_single_model::<$model>()
            }
            $crate::descriptor_from_factories(
                $name,
                $version,
                $epoch,
                __handlers,
                __resolve,
                __lower,
                __inventory,
            )
        };
    };

    // Event-first: DomainEventContract types + optional delete binding.
    (
        $vis:vis const $id:ident : $desc_ty:ty = {
            name: $name:literal,
            version: $version:expr,
            epoch: $epoch:literal,
            model: $model:ty,
            on_event { $($event_ty:ty),+ $(,)? }
            apply $mutation:path as $input_root:literal
            $(
                , on_deleted $del_event:ty => apply $del_mutation:path as $pk:literal
            )?
            $(,)?
        } $(;)?
    ) => {
        $vis const $id: $desc_ty = {
            fn __handlers() -> ::core::result::Result<
                $crate::ProjectionProgram,
                $crate::ProjectionProgramError,
            > {
                use $crate::domain_event::DomainEventContract;
                let mutation = $mutation().program().clone();
                let mut handlers = ::std::vec::Vec::new();
                $(
                    {
                        handlers.push(
                            $crate::bind_state_body_to_mutation::<$model>(
                                &<$event_ty>::descriptor(),
                                mutation.clone(),
                                $input_root,
                            )
                            .map_err(|e| $crate::ProjectionProgramError::InvalidOperation {
                                operation: ::std::string::String::from($name),
                                reason: e.to_string(),
                            })?,
                        );
                    }
                )+
                $(
                    {
                        handlers.push(
                            $crate::bind_delete_to_envelope_id(
                                &<$del_event>::descriptor(),
                                $del_mutation().program().clone(),
                                $pk,
                            )
                            .map_err(|e| $crate::ProjectionProgramError::InvalidOperation {
                                operation: ::std::string::String::from($name),
                                reason: e.to_string(),
                            })?,
                        );
                    }
                )?
                $crate::compile_portable_handlers(
                    $name,
                    $version,
                    $crate::ProjectionPartition::Unit,
                    handlers,
                )
            }
            fn __resolve(
                occurrence: &$crate::DomainEventOccurrence,
            ) -> ::core::result::Result<
                $crate::ResolvedProjectionPlan,
                $crate::ProjectionProgramError,
            > {
                $crate::resolve_mutation_program(&__handlers()?, occurrence)
            }
            fn __lower(
                plan: &$crate::ResolvedProjectionPlan,
            ) -> ::core::result::Result<
                $crate::projection::lower::LoweredProjectionPlan,
                $crate::projection::lower::ProjectionLoweringError,
            > {
                $crate::lower_single_model::<$model>(plan)
            }
            fn __inventory() -> ::core::result::Result<
                $crate::projection::lower::ProjectionOutputInventory,
                $crate::projection::lower::ProjectionLoweringError,
            > {
                $crate::inventory_single_model::<$model>()
            }
            $crate::descriptor_from_factories(
                $name,
                $version,
                $epoch,
                __handlers,
                __resolve,
                __lower,
                __inventory,
            )
        };
    };
}

/// Lower-level mount when handlers need a custom partition (e.g. chat room_id).
///
/// Prefer [`portable_handlers!`] when partition is unit.
#[macro_export]
macro_rules! mutation_projector {
    (
        $vis:vis const $id:ident : $desc_ty:ty = {
            name: $name:literal,
            version: $version:expr,
            epoch: $epoch:literal,
            model: $model:ty,
            program: $program:path $(,)?
        } $(;)?
    ) => {
        $vis const $id: $desc_ty = {
            fn __program() -> ::core::result::Result<
                $crate::ProjectionProgram,
                $crate::ProjectionProgramError,
            > {
                $program()
            }
            fn __resolve(
                occurrence: &$crate::DomainEventOccurrence,
            ) -> ::core::result::Result<
                $crate::ResolvedProjectionPlan,
                $crate::ProjectionProgramError,
            > {
                $crate::resolve_mutation_program(&__program()?, occurrence)
            }
            fn __lower(
                plan: &$crate::ResolvedProjectionPlan,
            ) -> ::core::result::Result<
                $crate::projection::lower::LoweredProjectionPlan,
                $crate::projection::lower::ProjectionLoweringError,
            > {
                $crate::lower_single_model::<$model>(plan)
            }
            fn __inventory() -> ::core::result::Result<
                $crate::projection::lower::ProjectionOutputInventory,
                $crate::projection::lower::ProjectionLoweringError,
            > {
                $crate::inventory_single_model::<$model>()
            }
            $crate::descriptor_from_factories(
                $name,
                $version,
                $epoch,
                __program,
                __resolve,
                __lower,
                __inventory,
            )
        };
    };
}

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
// `ReadModel` derive expands to), the in-memory default store, workspace
// entry points, and the version marker. Physical write-plan builders are
// low-level adapter detail under `distributed::read_model::*` (not projector
// authoring). Load-graph, query, and row-include plumbing stays there too.
pub use read_model::{
    InMemoryReadModelStore, ReadModel, ReadModelChange, ReadModelWorkspaceExt, RelationalReadModel,
    RelationalReadModelIncludes, Versioned,
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

// Re-export proc macros (event-owning `projection!` and separately authored
// `command_effects!` / `command_confirmations!` removed — use `mutation!` +
// portable/modeled handlers; commands predict events via `.emits`/`.preview`).
pub use distributed_macros::{
    aggregate, command_input_defaults, digest, mutation, mutation_file, sourced, DomainEvent,
    DomainState,
    GraphqlInput, GraphqlOutput, ReadModel, Snapshot,
};

// Re-export enqueue macro (requires "emitter" feature)
#[cfg(feature = "emitter")]
pub use distributed_macros::enqueue;
