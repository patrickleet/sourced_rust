use std::collections::{BTreeSet, HashMap};

use distributed::{
    ColumnType, ReadModel, ReadModelError, ReadModelMigrationArtifact, ReadModelSchema,
    ReadModelSchemaAdapter, ReadModelSchemaAdapterCapabilities, ReadModelSchemaBootstrap,
    ReadModelSchemaIssue, ReadModelSchemaIssueKind, ReadModelSchemaRegistry,
    ReadModelSchemaVerification, RelationalReadModel, DEFAULT_READ_MODEL_VERSION_COLUMN,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("account_summaries")]
struct AccountSummary {
    #[id("account_id")]
    account_id: String,
    #[unique]
    owner_slug: String,
    balance_cents: i64,
    #[readmodel(default = "0")]
    deposit_count: u32,
    #[readmodel(jsonb)]
    counters_by_game: HashMap<String, i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("players")]
struct Player {
    #[id("player_id")]
    player_id: String,
    display_name: String,
    #[readmodel(has_many = "PlayerWeapon", foreign_key = "player_id")]
    weapons: Vec<PlayerWeapon>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("player_weapons")]
#[readmodel(primary_key = ["player_id", "weapon_id"])]
struct PlayerWeapon {
    #[readmodel(foreign_key = "players.player_id", delegated_from = "Player.player_id")]
    player_id: String,
    weapon_id: String,
    #[index]
    acquired_at: String,
}

struct UnsupportedSchemaAdapter;

impl ReadModelSchemaAdapter for UnsupportedSchemaAdapter {
    fn schema_capabilities(&self) -> ReadModelSchemaAdapterCapabilities {
        ReadModelSchemaAdapterCapabilities::default()
    }
}

struct FakeSqlSchemaAdapter {
    existing_tables: BTreeSet<String>,
}

impl FakeSqlSchemaAdapter {
    fn new(existing_tables: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            existing_tables: existing_tables.into_iter().map(Into::into).collect(),
        }
    }
}

impl ReadModelSchemaAdapter for FakeSqlSchemaAdapter {
    fn schema_capabilities(&self) -> ReadModelSchemaAdapterCapabilities {
        ReadModelSchemaAdapterCapabilities::all()
    }

    fn generate_migration_artifacts(
        &self,
        registry: &ReadModelSchemaRegistry,
    ) -> Result<Vec<ReadModelMigrationArtifact>, ReadModelError> {
        registry.validate()?;
        Ok(vec![ReadModelMigrationArtifact::new(
            "read-models",
            registry.schemas().map(create_table_statement),
        )])
    }

    fn verify_schema(
        &self,
        registry: &ReadModelSchemaRegistry,
    ) -> Result<ReadModelSchemaVerification, ReadModelError> {
        registry.validate()?;
        let mut issues = Vec::new();
        for schema in registry.schemas() {
            if !self.existing_tables.contains(&schema.table_name) {
                issues.push(ReadModelSchemaIssue::new(
                    schema.table_name.clone(),
                    None::<String>,
                    ReadModelSchemaIssueKind::MissingTable,
                    format!("missing table `{}`", schema.table_name),
                ));
            }
        }
        Ok(ReadModelSchemaVerification { issues })
    }

    fn bootstrap_schema_for_dev(
        &self,
        registry: &ReadModelSchemaRegistry,
    ) -> Result<ReadModelSchemaBootstrap, ReadModelError> {
        registry.validate()?;
        Ok(ReadModelSchemaBootstrap::new(
            registry.table_names().map(str::to_string),
        ))
    }
}

fn registry() -> ReadModelSchemaRegistry {
    let mut registry = ReadModelSchemaRegistry::new();
    registry
        .register::<AccountSummary>()
        .unwrap()
        .register::<Player>()
        .unwrap()
        .register::<PlayerWeapon>()
        .unwrap();
    registry
}

fn create_table_statement(schema: &ReadModelSchema) -> String {
    let columns = schema
        .columns
        .iter()
        .map(|column| {
            let nullability = if column.nullable { "null" } else { "not null" };
            format!(
                "{} {} {}",
                column.column_name,
                logical_type_name(&column.column_type, column.jsonb),
                nullability
            )
        })
        .chain(
            schema
                .version_column
                .iter()
                .map(|column| format!("{column} unsigned_integer not null default 1")),
        )
        .collect::<Vec<_>>()
        .join(", ");
    let primary_key = schema.primary_key.columns.join(", ");
    format!(
        "create table {} ({columns}, primary key ({primary_key}));",
        schema.table_name
    )
}

fn logical_type_name(column_type: &ColumnType, jsonb: bool) -> &'static str {
    if jsonb {
        return "jsonb";
    }

    match column_type {
        ColumnType::Text => "text",
        ColumnType::Boolean => "boolean",
        ColumnType::Integer => "integer",
        ColumnType::UnsignedInteger => "unsigned_integer",
        ColumnType::Float => "float",
        ColumnType::Bytes => "bytes",
        ColumnType::Json => "json",
        ColumnType::Timestamp => "timestamp",
        ColumnType::Unsupported(_) => "unsupported",
    }
}

#[test]
fn registry_registers_relational_models_and_exposes_schema_metadata() {
    let registry = registry();

    registry.validate().unwrap();
    assert_eq!(registry.len(), 3);
    assert_eq!(
        registry.table_names().collect::<Vec<_>>(),
        vec!["account_summaries", "player_weapons", "players"]
    );

    let summary = registry.schema_for_model("AccountSummary").unwrap();
    assert_eq!(summary.table_name, "account_summaries");
    assert_eq!(
        summary.version_column.as_deref(),
        Some(DEFAULT_READ_MODEL_VERSION_COLUMN)
    );
    assert!(summary
        .columns
        .iter()
        .any(|column| column.column_name == "counters_by_game" && column.jsonb));
    assert!(summary
        .indexes
        .iter()
        .any(|index| index.unique && index.columns == vec!["owner_slug"]));

    let weapon = registry.schema_for_table("player_weapons").unwrap();
    assert_eq!(weapon.primary_key.columns, vec!["player_id", "weapon_id"]);
    assert!(weapon.columns.iter().any(|column| {
        column.column_name == "player_id"
            && column.delegated_from.as_deref() == Some("Player.player_id")
            && column
                .foreign_key
                .as_ref()
                .is_some_and(|foreign_key| foreign_key.table == "players")
    }));
}

#[test]
fn registry_rejects_duplicate_tables_and_invalid_foreign_key_targets() {
    let mut duplicate_registry = ReadModelSchemaRegistry::new();
    duplicate_registry.register::<AccountSummary>().unwrap();

    let err = duplicate_registry.register::<AccountSummary>().unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("already contains table"))
    );

    let mut invalid_registry = ReadModelSchemaRegistry::new();
    invalid_registry
        .register_schema(PlayerWeapon::schema().clone())
        .unwrap();

    let err = invalid_registry.validate().unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("unregistered foreign-key table"))
    );
}

#[test]
fn registry_rejects_relationship_foreign_keys_missing_from_target_model() {
    let mut player_schema = Player::schema().clone();
    player_schema.relationships[0].foreign_key = Some("missing_player_id".into());
    let mut registry = ReadModelSchemaRegistry::new();
    registry
        .register::<AccountSummary>()
        .unwrap()
        .register_schema(player_schema)
        .unwrap()
        .register::<PlayerWeapon>()
        .unwrap();

    let err = registry.validate().unwrap_err();

    assert!(matches!(err, ReadModelError::Metadata(message)
            if message.contains("foreign key `missing_player_id`")
                && message.contains("target model `PlayerWeapon`")));
}

#[test]
fn adapters_can_generate_migration_artifacts_or_report_unsupported() {
    let registry = registry();
    let unsupported = UnsupportedSchemaAdapter;

    let err = unsupported
        .generate_migration_artifacts(&registry)
        .unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("migration artifact generation"))
    );

    let adapter = FakeSqlSchemaAdapter::new(Vec::<String>::new());
    let artifacts = adapter.generate_migration_artifacts(&registry).unwrap();

    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].name, "read-models");
    assert!(artifacts[0]
        .statements
        .iter()
        .any(|statement| statement.contains("account_summaries")
            && statement.contains("jsonb")
            && statement.contains(DEFAULT_READ_MODEL_VERSION_COLUMN)));
    assert!(artifacts[0]
        .statements
        .iter()
        .any(|statement| statement.contains("primary key (player_id, weapon_id)")));
}

#[test]
fn adapters_can_verify_schema_and_explicitly_bootstrap_dev_schema() {
    let registry = registry();
    let adapter = FakeSqlSchemaAdapter::new(["account_summaries"]);

    let verification = adapter.verify_schema(&registry).unwrap();

    assert!(!verification.is_verified());
    assert_eq!(
        verification
            .issues
            .iter()
            .filter(|issue| issue.kind == ReadModelSchemaIssueKind::MissingTable)
            .count(),
        2
    );

    let bootstrap = adapter.bootstrap_schema_for_dev(&registry).unwrap();

    assert_eq!(
        bootstrap.bootstrapped_tables,
        vec!["account_summaries", "player_weapons", "players"]
    );
}
