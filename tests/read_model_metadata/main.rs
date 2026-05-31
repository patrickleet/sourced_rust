use std::collections::HashMap;

use distributed::{
    ColumnType, ReadModel, ReadModelError, RelationalReadModel, RelationshipKind, RowValue,
    DEFAULT_READ_MODEL_VERSION_COLUMN,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("account_summaries")]
struct AccountSummary {
    #[id("account_id")]
    account_id: String,
    #[index]
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
    acquired_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[collection("direct_document_views")]
struct DirectDocumentView {
    #[id]
    id: String,
    value: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("direct_table_views")]
#[index(
    name = "idx_direct_table_views_tenant_value",
    columns = ["tenant_id", "value"]
)]
#[unique(columns = ["tenant_id", "slug"])]
struct DirectTableView {
    #[id("direct_id")]
    id: String,
    tenant_id: String,
    slug: String,
    #[column("direct_value")]
    #[index("idx_direct_table_views_direct_value")]
    value: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("binary_assets")]
struct BinaryAsset {
    #[id("asset_id")]
    id: String,
    payload: Vec<u8>,
    optional_payload: Option<Vec<u8>>,
}

#[test]
fn derive_preserves_read_model_identity_for_table_models() {
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
fn row_conversion_keeps_binary_columns_as_bytes_values() {
    let asset = BinaryAsset {
        id: "asset-1".into(),
        payload: vec![0, 1, 255],
        optional_payload: Some(vec![2, 3, 5]),
    };

    let row = asset.to_row().unwrap();

    assert_eq!(row.get("payload"), Some(&RowValue::Bytes(vec![0, 1, 255])));
}

#[test]
fn row_conversion_keeps_optional_binary_columns_as_bytes_values() {
    let asset = BinaryAsset {
        id: "asset-1".into(),
        payload: vec![0, 1, 255],
        optional_payload: Some(vec![2, 3, 5]),
    };

    let row = asset.to_row().unwrap();

    assert_eq!(
        row.get("optional_payload"),
        Some(&RowValue::Bytes(vec![2, 3, 5]))
    );
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
    #[table("missing_key_models")]
    struct MissingKeyModel {
        value: String,
    }

    let err = MissingKeyModel::schema().validate().unwrap_err();

    assert!(matches!(err, ReadModelError::Metadata(message) if message.contains("primary-key")));
}

#[test]
fn direct_collection_table_column_and_index_attributes_wrap_readmodel_metadata() {
    let direct = DirectDocumentView {
        id: "doc-1".into(),
        value: 7,
    };
    let schema = DirectTableView::schema();

    assert_eq!(DirectDocumentView::COLLECTION, "direct_document_views");
    assert_eq!(direct.id(), "doc-1");
    assert_eq!(DirectTableView::COLLECTION, "direct_table_views");
    assert_eq!(schema.table_name, "direct_table_views");
    assert_eq!(schema.primary_key.columns, vec!["direct_id"]);
    assert!(schema
        .columns
        .iter()
        .any(|column| column.field_name == "id" && column.column_name == "direct_id"));
    assert!(schema
        .columns
        .iter()
        .any(|column| column.field_name == "value" && column.column_name == "direct_value"));
    assert!(schema.indexes.iter().any(|index| {
        index.name.as_deref() == Some("idx_direct_table_views_direct_value")
            && index.columns == vec!["direct_value"]
            && !index.unique
    }));
    assert!(schema.indexes.iter().any(|index| {
        index.name.as_deref() == Some("idx_direct_table_views_tenant_value")
            && index.columns == vec!["tenant_id", "direct_value"]
            && !index.unique
    }));
    assert!(schema.indexes.iter().any(|index| {
        index.name.as_deref() == Some("uq_direct_table_views_tenant_id_slug")
            && index.columns == vec!["tenant_id", "slug"]
            && index.unique
    }));
}
