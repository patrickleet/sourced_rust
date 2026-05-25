use crate::table::{
    ColumnType, TableMigrationArtifact, TableSchema, TableSchemaAdapter,
    TableSchemaAdapterCapabilities, TableSchemaBootstrap, TableSchemaRegistry, TableStoreError,
};

/// SQL dialect used to render table-schema migration artifacts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableSqlDialect {
    Sqlite,
    Postgres,
}

/// Stateless adapter for generating SQL artifacts from table schemas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableSqlSchemaAdapter {
    dialect: TableSqlDialect,
}

impl TableSqlSchemaAdapter {
    pub fn sqlite() -> Self {
        Self {
            dialect: TableSqlDialect::Sqlite,
        }
    }

    pub fn postgres() -> Self {
        Self {
            dialect: TableSqlDialect::Postgres,
        }
    }

    pub fn dialect(&self) -> TableSqlDialect {
        self.dialect
    }
}

impl TableSchemaAdapter for TableSqlSchemaAdapter {
    fn schema_capabilities(&self) -> TableSchemaAdapterCapabilities {
        TableSchemaAdapterCapabilities {
            migration_artifacts: true,
            schema_verification: false,
            dev_bootstrap: false,
        }
    }

    fn generate_migration_artifacts(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(registry, self.dialect)
    }
}

pub fn generate_table_migration_artifacts(
    registry: &TableSchemaRegistry,
    dialect: TableSqlDialect,
) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
    Ok(vec![TableMigrationArtifact::new(
        artifact_name(dialect),
        table_schema_statements(registry, dialect)?,
    )])
}

pub fn table_schema_statements(
    registry: &TableSchemaRegistry,
    dialect: TableSqlDialect,
) -> Result<Vec<String>, TableStoreError> {
    registry.validate()?;

    let mut statements = Vec::new();
    for schema in registry.schemas() {
        statements.push(create_table_statement(schema, dialect)?);
        statements.extend(index_statements(schema));
    }
    Ok(statements)
}

fn create_table_statement(
    schema: &TableSchema,
    dialect: TableSqlDialect,
) -> Result<String, TableStoreError> {
    let mut definitions = schema
        .columns
        .iter()
        .map(|column| {
            let mut definition = format!(
                "{} {}",
                quote_identifier(&column.column_name),
                sql_type(&column.column_type, column.jsonb, dialect)?
            );
            if !column.nullable || column.primary_key {
                definition.push_str(" NOT NULL");
            }
            if column.has_default {
                if let Some(default) = column.default.as_deref() {
                    definition.push_str(" DEFAULT ");
                    definition.push_str(default);
                }
            }
            Ok(definition)
        })
        .collect::<Result<Vec<_>, TableStoreError>>()?;

    if let Some(version_column) = schema.version_column.as_deref() {
        definitions.push(format!(
            "{} {} NOT NULL DEFAULT 1",
            quote_identifier(version_column),
            sql_type(&ColumnType::UnsignedInteger, false, dialect)?
        ));
    }

    definitions.push(format!(
        "PRIMARY KEY ({})",
        schema
            .primary_key
            .columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    for column in &schema.columns {
        if let Some(foreign_key) = &column.foreign_key {
            definitions.push(format!(
                "FOREIGN KEY ({}) REFERENCES {} ({})",
                quote_identifier(&column.column_name),
                quote_identifier(&foreign_key.table),
                quote_identifier(&foreign_key.column)
            ));
        }
    }

    for foreign_key in &schema.foreign_keys {
        let already_declared_on_column = schema.columns.iter().any(|column| {
            column.column_name == foreign_key.column
                && column.foreign_key.as_ref() == Some(foreign_key)
        });
        if already_declared_on_column {
            continue;
        }
        definitions.push(format!(
            "FOREIGN KEY ({}) REFERENCES {} ({})",
            quote_identifier(&foreign_key.column),
            quote_identifier(&foreign_key.table),
            quote_identifier(&foreign_key.column)
        ));
    }

    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {} (\n  {}\n);",
        quote_identifier(&schema.table_name),
        definitions.join(",\n  ")
    ))
}

fn index_statements(schema: &TableSchema) -> impl Iterator<Item = String> + '_ {
    schema.indexes.iter().map(|index| {
        let name = index
            .name
            .clone()
            .unwrap_or_else(|| format!("{}_{}_idx", schema.table_name, index.columns.join("_")));
        let unique = if index.unique { "UNIQUE " } else { "" };
        format!(
            "CREATE {unique}INDEX IF NOT EXISTS {} ON {} ({});",
            quote_identifier(&name),
            quote_identifier(&schema.table_name),
            index
                .columns
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

fn sql_type(
    column_type: &ColumnType,
    jsonb: bool,
    dialect: TableSqlDialect,
) -> Result<&'static str, TableStoreError> {
    let type_name = match (dialect, column_type) {
        (TableSqlDialect::Sqlite, ColumnType::Text) => "TEXT",
        (TableSqlDialect::Sqlite, ColumnType::Boolean) => "INTEGER",
        (TableSqlDialect::Sqlite, ColumnType::Integer | ColumnType::UnsignedInteger) => "INTEGER",
        (TableSqlDialect::Sqlite, ColumnType::Float) => "REAL",
        (TableSqlDialect::Sqlite, ColumnType::Bytes) => "BLOB",
        (TableSqlDialect::Sqlite, ColumnType::Json) => "TEXT",
        (TableSqlDialect::Postgres, ColumnType::Text) => "text",
        (TableSqlDialect::Postgres, ColumnType::Boolean) => "boolean",
        (TableSqlDialect::Postgres, ColumnType::Integer | ColumnType::UnsignedInteger) => "bigint",
        (TableSqlDialect::Postgres, ColumnType::Float) => "double precision",
        (TableSqlDialect::Postgres, ColumnType::Bytes) => "bytea",
        (TableSqlDialect::Postgres, ColumnType::Json) if jsonb => "jsonb",
        (TableSqlDialect::Postgres, ColumnType::Json) => "jsonb",
        (_, ColumnType::Unsupported(type_name)) => {
            return Err(TableStoreError::Metadata(format!(
                "unsupported table column type `{type_name}`"
            )));
        }
    };
    Ok(type_name)
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn artifact_name(dialect: TableSqlDialect) -> &'static str {
    match dialect {
        TableSqlDialect::Sqlite => "sqlite-tables",
        TableSqlDialect::Postgres => "postgres-tables",
    }
}

pub fn bootstrap_result(registry: &TableSchemaRegistry) -> TableSchemaBootstrap {
    TableSchemaBootstrap::new(registry.table_names().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::{outbox_message_schema, OUTBOX_MESSAGES_TABLE};

    #[test]
    fn renders_outbox_table_schema_for_sqlite() {
        let mut registry = TableSchemaRegistry::new();
        registry
            .register_schema(outbox_message_schema())
            .expect("schema should register");

        let artifact = generate_table_migration_artifacts(&registry, TableSqlDialect::Sqlite)
            .expect("artifact should render")
            .pop()
            .expect("artifact should exist");

        assert_eq!(artifact.name, "sqlite-tables");
        assert!(artifact
            .statements
            .iter()
            .any(|statement| statement.contains("CREATE TABLE IF NOT EXISTS \"outbox_messages\"")));
        assert!(artifact
            .statements
            .iter()
            .any(|statement| statement.contains("\"message_id\" TEXT NOT NULL")));
    }

    #[test]
    fn bootstrap_result_lists_registered_tables() {
        let mut registry = TableSchemaRegistry::new();
        registry
            .register_schema(outbox_message_schema())
            .expect("schema should register");

        let result = bootstrap_result(&registry);

        assert_eq!(
            result.bootstrapped_tables,
            vec![OUTBOX_MESSAGES_TABLE.to_string()]
        );
    }
}
