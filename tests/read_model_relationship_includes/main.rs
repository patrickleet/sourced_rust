use serde::{Deserialize, Serialize};
use sourced_rust::{
    InMemoryReadModelStore, ReadModel, ReadModelAdapterCapabilities, ReadModelCommitOutcome,
    ReadModelError, ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities,
    ReadModelSessionStore, ReadModelUnitOfWorkExt, ReadModelWritePlan,
    RelationalReadModelQueryStore, RowKey, RowValue,
};

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
    #[readmodel(belongs_to = "Player", foreign_key = "player_id")]
    player: Option<Player>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "players_with_many")]
struct PlayerWithMany {
    #[readmodel(id, column = "player_id")]
    player_id: String,
    #[readmodel(
        many_to_many = "Weapon",
        through = "player_weapon_links",
        foreign_key = "player_id"
    )]
    weapons: Vec<Weapon>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "weapons")]
struct Weapon {
    #[readmodel(id, column = "weapon_id")]
    weapon_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "weapon_label_refs")]
struct WeaponLabelRef {
    #[readmodel(id, column = "ref_id")]
    ref_id: String,
    player_id: String,
    #[readmodel(belongs_to = "CompositeWeaponLabel", foreign_key = "player_id")]
    label: Option<CompositeWeaponLabel>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "weapon_labels", primary_key = ["player_id", "weapon_id"])]
struct CompositeWeaponLabel {
    player_id: String,
    weapon_id: String,
    label: String,
}

struct NoIncludeStore {
    inner: InMemoryReadModelStore,
}

impl NoIncludeStore {
    fn new(inner: InMemoryReadModelStore) -> Self {
        Self { inner }
    }
}

impl ReadModelSessionStore for NoIncludeStore {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        self.inner.read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> Result<ReadModelCommitOutcome, ReadModelError> {
        self.inner.commit_write_plan(plan)
    }

    fn is_processed(&self, consumer_name: &str, message_id: &str) -> Result<bool, ReadModelError> {
        self.inner.is_processed(consumer_name, message_id)
    }
}

impl RelationalReadModelQueryStore for NoIncludeStore {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        ReadModelQueryCapabilities::default()
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> Result<ReadModelLoadGraph, ReadModelError> {
        request.validate_for_query_capabilities(&self.read_model_query_capabilities())?;
        self.inner.load_graph(request)
    }
}

fn player_key(player_id: &str) -> RowKey {
    RowKey::new([("player_id", RowValue::String(player_id.into()))])
}

fn weapon_key(player_id: &str, weapon_id: &str) -> RowKey {
    RowKey::new([
        ("player_id", RowValue::String(player_id.into())),
        ("weapon_id", RowValue::String(weapon_id.into())),
    ])
}

fn player(player_id: &str, display_name: &str) -> Player {
    Player {
        player_id: player_id.into(),
        display_name: display_name.into(),
        weapons: Vec::new(),
    }
}

fn weapon(player_id: &str, weapon_id: &str, acquired_at: &str) -> PlayerWeapon {
    PlayerWeapon {
        player_id: player_id.into(),
        weapon_id: weapon_id.into(),
        acquired_at: acquired_at.into(),
        player: None,
    }
}

fn store_with_player_and_weapons(
    weapons: impl IntoIterator<Item = PlayerWeapon>,
) -> InMemoryReadModelStore {
    let store = InMemoryReadModelStore::new();
    store.register_schema::<Player>().unwrap();
    store.register_schema::<PlayerWeapon>().unwrap();

    let mut session = sourced_rust::ReadModelSession::new();
    session.save(&player("player-1", "Ada")).unwrap();
    for weapon in weapons {
        session.save(&weapon).unwrap();
    }
    session.commit(&store).unwrap();
    store
}

#[test]
fn friendly_session_loads_one_root_by_primary_key_without_includes() {
    let store = store_with_player_and_weapons([]);
    let mut read_models = store.session();

    let loaded = read_models
        .load::<Player>(player_key("player-1"))
        .one()
        .unwrap();

    assert_eq!(loaded.unwrap().data.display_name, "Ada");
}

#[test]
fn friendly_session_hydrates_has_many_include() {
    let store = store_with_player_and_weapons([weapon("player-1", "sword", "2026-05-23")]);
    let mut read_models = store.session();

    let loaded = read_models
        .load::<Player>(player_key("player-1"))
        .include("weapons")
        .one()
        .unwrap()
        .unwrap();

    assert_eq!(loaded.data.weapons[0].weapon_id, "sword");
}

#[test]
fn friendly_session_hydrates_belongs_to_include() {
    let store = store_with_player_and_weapons([weapon("player-1", "sword", "2026-05-23")]);
    let mut read_models = store.session();

    let loaded = read_models
        .load::<PlayerWeapon>(weapon_key("player-1", "sword"))
        .include("player")
        .one()
        .unwrap()
        .unwrap();

    assert_eq!(loaded.data.player.unwrap().display_name, "Ada");
}

#[test]
fn save_changes_persists_loaded_scalar_field_without_manual_patch() {
    let store = store_with_player_and_weapons([]);
    let mut read_models = store.session();
    let mut loaded = read_models
        .load::<Player>(player_key("player-1"))
        .one()
        .unwrap()
        .unwrap()
        .data;
    loaded.display_name = "Ada Lovelace".into();

    read_models.save_changes(loaded).unwrap();
    read_models.commit().unwrap();

    let mut check = store.session();
    let reloaded = check
        .load::<Player>(player_key("player-1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.data.display_name, "Ada Lovelace");
}

#[test]
fn save_changes_persists_added_and_modified_related_rows() {
    let store = store_with_player_and_weapons([weapon("player-1", "sword", "2026-05-23")]);
    let mut read_models = store.session();
    let mut loaded = read_models
        .load::<Player>(player_key("player-1"))
        .include("weapons")
        .one()
        .unwrap()
        .unwrap()
        .data;
    loaded.weapons[0].acquired_at = "2026-05-24".into();
    loaded.weapons.push(weapon("", "shield", "2026-05-25"));

    read_models.save_changes(loaded).unwrap();
    read_models.commit().unwrap();

    let mut check = store.session();
    let mut reloaded = check
        .load::<Player>(player_key("player-1"))
        .include("weapons")
        .one()
        .unwrap()
        .unwrap()
        .data;
    reloaded
        .weapons
        .sort_by(|left, right| left.weapon_id.cmp(&right.weapon_id));
    assert_eq!(reloaded.weapons[0].player_id, "player-1");
    assert_eq!(reloaded.weapons[0].weapon_id, "shield");
    assert_eq!(reloaded.weapons[1].acquired_at, "2026-05-24");
}

#[test]
fn save_changes_does_not_delete_removed_related_rows_by_default() {
    let store = store_with_player_and_weapons([
        weapon("player-1", "shield", "2026-05-24"),
        weapon("player-1", "sword", "2026-05-23"),
    ]);
    let mut read_models = store.session();
    let mut loaded = read_models
        .load::<Player>(player_key("player-1"))
        .include("weapons")
        .one()
        .unwrap()
        .unwrap()
        .data;
    loaded.weapons.retain(|weapon| weapon.weapon_id == "sword");

    read_models.save_changes(loaded).unwrap();
    read_models.commit().unwrap();

    let mut check = store.session();
    let reloaded = check
        .load::<Player>(player_key("player-1"))
        .include("weapons")
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.data.weapons.len(), 2);
}

#[test]
fn missing_root_returns_none_without_include_loading() {
    let store = store_with_player_and_weapons([weapon("player-1", "sword", "2026-05-23")]);
    let mut read_models = store.session();

    let loaded = read_models
        .load::<Player>(player_key("missing"))
        .include("weapons")
        .one()
        .unwrap();

    assert!(loaded.is_none());
}

#[test]
fn unregistered_relationship_target_fails_before_loading() {
    let store = InMemoryReadModelStore::new();
    store.register_schema::<Player>().unwrap();
    let mut session = sourced_rust::ReadModelSession::new();
    session.save(&player("player-1", "Ada")).unwrap();
    session.commit(&store).unwrap();
    let mut read_models = store.session();

    let err = read_models
        .load::<Player>(player_key("player-1"))
        .include("weapons")
        .one()
        .unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("unregistered model `PlayerWeapon`"))
    );
}

#[test]
fn unregistered_root_schema_fails_before_loading() {
    let store = InMemoryReadModelStore::new();
    let mut read_models = store.session();

    let err = read_models
        .load::<Player>(player_key("player-1"))
        .one()
        .unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("is not registered"))
    );
}

#[test]
fn adapter_without_include_capability_rejects_includes() {
    let inner = store_with_player_and_weapons([weapon("player-1", "sword", "2026-05-23")]);
    let store = NoIncludeStore::new(inner);
    let mut read_models = store.session();

    let err = read_models
        .load::<Player>(player_key("player-1"))
        .include("weapons")
        .one()
        .unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("relationship includes"))
    );
}

#[test]
fn nested_query_style_include_paths_are_not_a_public_query_dsl() {
    let store = store_with_player_and_weapons([weapon("player-1", "sword", "2026-05-23")]);
    let mut read_models = store.session();

    let err = read_models
        .load::<Player>(player_key("player-1"))
        .include("weapons.owner")
        .one()
        .unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("has no relationship"))
    );
}

#[test]
fn many_to_many_include_fails_until_join_metadata_is_rich_enough() {
    let store = InMemoryReadModelStore::new();
    store.register_schema::<PlayerWithMany>().unwrap();
    let mut read_models = store.session();

    let err = read_models
        .load::<PlayerWithMany>(player_key("player-1"))
        .include("weapons")
        .one()
        .unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("many-to-many relationship"))
    );
}

#[test]
fn belongs_to_include_rejects_composite_target_primary_key() {
    let store = InMemoryReadModelStore::new();
    store.register_schema::<WeaponLabelRef>().unwrap();
    store.register_schema::<CompositeWeaponLabel>().unwrap();
    let mut session = sourced_rust::ReadModelSession::new();
    session
        .save(&WeaponLabelRef {
            ref_id: "ref-1".into(),
            player_id: "player-1".into(),
            label: None,
        })
        .unwrap()
        .save(&CompositeWeaponLabel {
            player_id: "player-1".into(),
            weapon_id: "sword".into(),
            label: "Sword".into(),
        })
        .unwrap();
    session.commit(&store).unwrap();
    let mut read_models = store.session();

    let err = read_models
        .load::<WeaponLabelRef>(RowKey::new([("ref_id", RowValue::String("ref-1".into()))]))
        .include("label")
        .one()
        .unwrap_err();

    assert!(
        matches!(err, ReadModelError::Metadata(message) if message.contains("CompositeWeaponLabel")
            && message.contains("player_id")
            && message.contains("single-column primary key"))
    );
}
