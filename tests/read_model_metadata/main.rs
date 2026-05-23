use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sourced_rust::{
    ColumnType, ReadModel, ReadModelError, RelationalReadModel, RelationshipKind, RowValue,
    DEFAULT_READ_MODEL_VERSION_COLUMN,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "account_summaries")]
struct AccountSummary {
    #[readmodel(id, column = "account_id")]
    account_id: String,
    #[readmodel(index)]
    owner: Option<String>,
    balance_cents: i64,
    #[readmodel(default = "0")]
    deposit_count: u32,
    #[readmodel(jsonb)]
    counters_by_game: HashMap<String, i64>,
    #[readmodel(skip_query)]
    projected_event_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "players")]
struct Player {
    #[readmodel(id, column = "player_id")]
    player_id: String,
    display_name: String,
    #[readmodel(has_many = "PlayerWeapon", foreign_key = "player_id")]
    weapons: Vec<PlayerWeapon>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "player_weapons", primary_key = ["player_id", "weapon_id"])]
struct PlayerWeapon {
    #[readmodel(foreign_key = "players.player_id", delegated_from = "Player.player_id")]
    player_id: String,
    weapon_id: String,
    acquired_at: String,
}

#[test]
fn derive_allows_table_models_with_string_ids_to_use_document_rows() {
    let summary = AccountSummary {
        account_id: "acct-1".into(),
        owner: Some("Ada".into()),
        balance_cents: 100,
        deposit_count: 1,
        counters_by_game: HashMap::new(),
        projected_event_ids: Vec::new(),
    };

    assert_eq!(AccountSummary::COLLECTION, "account_summaries");
    assert_eq!(summary.id(), "acct-1");
}

#[test]
fn derive_describes_columns_indexes_nullability_and_jsonb() {
    let schema = AccountSummary::schema();

    schema.validate().unwrap();
    assert_eq!(schema.table_name, "account_summaries");
    assert_eq!(schema.primary_key.columns, vec!["account_id"]);
    assert_eq!(
        schema.version_column.as_deref(),
        Some(DEFAULT_READ_MODEL_VERSION_COLUMN)
    );

    let owner = schema
        .columns
        .iter()
        .find(|column| column.column_name == "owner")
        .unwrap();
    assert!(owner.nullable);

    let counters = schema
        .columns
        .iter()
        .find(|column| column.column_name == "counters_by_game")
        .unwrap();
    assert_eq!(counters.column_type, ColumnType::Json);
    assert!(counters.jsonb);

    assert_eq!(schema.indexes.len(), 1);
    assert_eq!(schema.indexes[0].columns, vec!["owner"]);

    let deposit_count = schema
        .columns
        .iter()
        .find(|column| column.column_name == "deposit_count")
        .unwrap();
    assert!(deposit_count.has_default);
    assert_eq!(deposit_count.default.as_deref(), Some("0"));
}

#[test]
fn row_conversion_round_trips_scalar_option_and_jsonb_fields() {
    let mut counters = HashMap::new();
    counters.insert("arena".to_string(), 3);
    let summary = AccountSummary {
        account_id: "acct-1".into(),
        owner: Some("Ada".into()),
        balance_cents: 2500,
        deposit_count: 2,
        counters_by_game: counters,
        projected_event_ids: Vec::new(),
    };

    let row = summary.to_row().unwrap();
    assert_eq!(
        row.get("account_id"),
        Some(&RowValue::String("acct-1".into()))
    );

    let round_trip = AccountSummary::from_row(row).unwrap();
    assert_eq!(round_trip, summary);
}

#[test]
fn derive_represents_relationships_composite_keys_and_delegated_foreign_keys() {
    let player_schema = Player::schema();
    let weapon_schema = PlayerWeapon::schema();

    player_schema.validate().unwrap();
    weapon_schema.validate().unwrap();

    assert_eq!(player_schema.relationships.len(), 1);
    assert_eq!(
        player_schema.relationships[0].kind,
        RelationshipKind::HasMany
    );
    assert_eq!(player_schema.relationships[0].target_model, "PlayerWeapon");
    assert_eq!(
        player_schema.relationships[0].foreign_key.as_deref(),
        Some("player_id")
    );

    assert_eq!(
        weapon_schema.primary_key.columns,
        vec!["player_id", "weapon_id"]
    );
    let player_id = weapon_schema
        .columns
        .iter()
        .find(|column| column.column_name == "player_id")
        .unwrap();
    assert_eq!(
        player_id.delegated_from.as_deref(),
        Some("Player.player_id")
    );
    assert_eq!(player_id.foreign_key.as_ref().unwrap().table, "players");
}

#[test]
fn metadata_validation_reports_missing_keys_before_storage_writes() {
    #[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
    #[readmodel(table = "missing_key_models")]
    struct MissingKeyModel {
        value: String,
    }

    let err = MissingKeyModel::schema().validate().unwrap_err();

    assert!(matches!(err, ReadModelError::Metadata(message) if message.contains("primary-key")));
}
