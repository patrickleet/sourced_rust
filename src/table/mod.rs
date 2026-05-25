//! Neutral table/row primitives shared by read models and operational tables.
//!
//! The read-model ORM introduced these structures first, but they are not
//! inherently read-model concepts. Outbox storage, inbox/checkpoint tables, and
//! future operational tables can use the same schema and row-write vocabulary.

mod sql;

pub use crate::read_model::{
    ColumnDef as TableColumn, ColumnType, DeleteRowMutation as DeleteTableRowMutation,
    DocumentMutation as TableDocumentMutation, ExpectedVersion, ForeignKey, IndexDef as TableIndex,
    PatchMode, PatchRowMutation as PatchTableRowMutation, PrimaryKey,
    ReadModelAdapterCapabilities as TableAdapterCapabilities,
    ReadModelCommitOutcome as TableCommitOutcome, ReadModelError as TableStoreError,
    ReadModelMigrationArtifact as TableMigrationArtifact, ReadModelMutation as TableMutation,
    ReadModelSchema as TableSchema, ReadModelSchemaAdapter as TableSchemaAdapter,
    ReadModelSchemaAdapterCapabilities as TableSchemaAdapterCapabilities,
    ReadModelSchemaBootstrap as TableSchemaBootstrap, ReadModelSchemaIssue as TableSchemaIssue,
    ReadModelSchemaIssueKind as TableSchemaIssueKind,
    ReadModelSchemaRegistry as TableSchemaRegistry,
    ReadModelSchemaVerification as TableSchemaVerification, ReadModelWritePlan as TableWritePlan,
    RelationshipDef, RelationshipKind, RowKey, RowMutation as TableRowMutation, RowPatch, RowValue,
    RowValues, RowWriteMode, DEFAULT_READ_MODEL_VERSION_COLUMN as DEFAULT_TABLE_VERSION_COLUMN,
};
pub use sql::{
    bootstrap_result as table_schema_bootstrap_result, generate_table_migration_artifacts,
    table_schema_statements, TableSqlDialect, TableSqlSchemaAdapter,
};

/// Opt-in trait for non-read-model types that map to a relational table row.
pub trait TableModel: Clone + Send + Sync + Sized {
    fn table_schema() -> TableSchema;
    fn table_key(&self) -> Result<RowKey, TableStoreError>;
    fn to_table_row(&self) -> Result<RowValues, TableStoreError>;
}

/// Extension methods for registering neutral table models.
pub trait TableSchemaRegistryExt {
    fn register_table<M>(&mut self) -> Result<&mut Self, TableStoreError>
    where
        M: TableModel;
}

impl TableSchemaRegistryExt for TableSchemaRegistry {
    fn register_table<M>(&mut self) -> Result<&mut Self, TableStoreError>
    where
        M: TableModel,
    {
        self.register_schema(M::table_schema())
    }
}
