use distributed::projection::lower::{EventualOnly, ProjectionDescriptor};
use distributed::table::TableSchemaRegistry;
use distributed::{
    Entity, InMemoryRepository, ReadModelWorkspaceExt, ReadModelWritePlanBuilder,
    ReadModelWritePlanStore, RowKey, RowValue,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, distributed::DomainState)]
#[domain_state(version = 1)]
struct PlayerState {
    player_id: String,
    display_name: String,
    weapon_id: String,
    acquired_at: String,
}

#[derive(Default)]
struct PlayerAggregate {
    entity: Entity,
    display_name: String,
    weapon_id: String,
    acquired_at: String,
}

impl From<&PlayerAggregate> for PlayerState {
    fn from(player: &PlayerAggregate) -> Self {
        Self {
            player_id: player.entity.id().to_owned(),
            display_name: player.display_name.clone(),
            weapon_id: player.weapon_id.clone(),
            acquired_at: player.acquired_at.clone(),
        }
    }
}

#[distributed::sourced(
    entity,
    events = "PlayerAggregateReplayEvent",
    aggregate_type = "projection.player",
    domain_state = PlayerState,
)]
impl PlayerAggregate {
    #[event("player.loaded", version = 1, domain)]
    fn load(
        &mut self,
        player_id: String,
        display_name: String,
        weapon_id: String,
        acquired_at: String,
    ) {
        self.entity.set_id(player_id);
        self.display_name = display_name;
        self.weapon_id = weapon_id;
        self.acquired_at = acquired_at;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, distributed::ReadModel)]
#[readmodel(table = "projection_players", primary_key = ["player_id"])]
struct Players {
    player_id: String,
    display_name: String,
    #[readmodel(has_many = "PlayerWeapons", foreign_key = "player_id")]
    weapons: Vec<PlayerWeapons>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, distributed::ReadModel)]
#[readmodel(table = "projection_player_weapons", primary_key = ["weapon_id"])]
struct PlayerWeapons {
    #[readmodel(
        foreign_key = "projection_players.player_id",
        delegated_from = "Players.player_id"
    )]
    player_id: String,
    weapon_id: String,
    acquired_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, distributed::ReadModel)]
#[readmodel(
    table = "projection_player_weapon_links",
    primary_key = ["player_id", "weapon_id"]
)]
struct PlayerWeaponLinks {
    #[readmodel(foreign_key = "projection_players.player_id")]
    player_id: String,
    #[readmodel(foreign_key = "projection_player_weapons.weapon_id")]
    weapon_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, distributed::ReadModel)]
#[readmodel(
    table = "projection_player_summaries",
    primary_key = ["player_id"]
)]
struct PlayerSummary {
    player_id: String,
    weapon_count: u64,
}

const PLAYER_GRAPH: ProjectionDescriptor<EventualOnly> = distributed_macros::projection! {
    name: "domain-event-player-graph";
    version: 1;
    epoch: "domain-event-player-graph-v1";
    partition: state.player_id;

    on "player.loaded" version 1 (state: PlayerState) {
        upsert PlayerSummary {
            key { player_id: state.player_id },
            set { weapon_count: 1u64 }
        };
        upsert Players {
            key { player_id: state.player_id },
            set { display_name: state.display_name }
        } as player;
        upsert_related player.weapons -> PlayerWeapons {
            key { weapon_id: state.weapon_id },
            set { acquired_at: state.acquired_at }
        };
        insert PlayerWeaponLinks {
            key {
                player_id: state.player_id,
                weapon_id: state.weapon_id
            },
            set {}
        };
    }
};

fn player_key() -> RowKey {
    RowKey::new([("player_id", RowValue::String("player-1".into()))])
}

fn weapon_key() -> RowKey {
    RowKey::new([
        ("player_id", RowValue::String("player-1".into())),
        ("weapon_id", RowValue::String("sword".into())),
    ])
}

fn occurrence() -> distributed::DomainEventOccurrence {
    let mut aggregate = PlayerAggregate::default();
    aggregate
        .load(
            "player-1".into(),
            "Ada".into(),
            "sword".into(),
            "2026-07-28T00:00:00Z".into(),
        )
        .expect("capture state-bearing domain event");
    aggregate
        .entity
        .pending_domain_events()
        .last()
        .expect("outward occurrence")
        .clone()
}

fn expected_models() -> (Players, PlayerWeapons, PlayerSummary, PlayerWeaponLinks) {
    (
        Players {
            player_id: "player-1".into(),
            display_name: "Ada".into(),
            weapons: Vec::new(),
        },
        PlayerWeapons {
            player_id: "player-1".into(),
            weapon_id: "sword".into(),
            acquired_at: "2026-07-28T00:00:00Z".into(),
        },
        PlayerSummary {
            player_id: "player-1".into(),
            weapon_count: 1,
        },
        PlayerWeaponLinks {
            player_id: "player-1".into(),
            weapon_id: "sword".into(),
        },
    )
}

fn generated_and_fluent_plans() -> (distributed::TableWritePlan, distributed::TableWritePlan) {
    let generated = PLAYER_GRAPH
        .server_executor()
        .expect("server executor")
        .plan(&occurrence())
        .expect("portable program lowers")
        .write_plan;
    let (player, weapon, summary, link) = expected_models();
    let mut fluent = ReadModelWritePlanBuilder::new();
    fluent
        .upsert(&summary)
        .expect("summary")
        .upsert(&player)
        .expect("parent")
        .upsert_related(&player, "weapons", &weapon)
        .expect("related child")
        .insert(&link)
        .expect("join");
    (
        generated,
        fluent.into_write_plan().expect("fluent physical plan"),
    )
}

async fn assert_in_memory_rows(repository: &InMemoryRepository) {
    let (player, weapon, summary, link) = expected_models();
    let mut workspace = repository.workspace();
    let hydrated = workspace
        .load::<Players>(player_key())
        .include("weapons")
        .one()
        .await
        .expect("load player graph")
        .expect("player graph exists");
    assert_eq!(hydrated.data.weapons, vec![weapon.clone()]);
    assert_eq!(
        Players {
            weapons: Vec::new(),
            ..hydrated.data
        },
        player
    );
    assert_eq!(
        workspace
            .load::<PlayerSummary>(player_key())
            .one()
            .await
            .expect("load summary")
            .expect("summary exists")
            .data,
        summary
    );
    assert_eq!(
        workspace
            .load::<PlayerWeaponLinks>(weapon_key())
            .one()
            .await
            .expect("load join")
            .expect("join exists")
            .data,
        link
    );
}

#[tokio::test]
async fn portable_four_table_program_matches_fluent_plan_and_applies_in_memory() {
    let (generated, fluent) = generated_and_fluent_plans();
    assert_eq!(generated, fluent);
    assert_eq!(
        generated
            .mutations
            .iter()
            .map(|mutation| mutation.table_name())
            .collect::<Vec<_>>(),
        vec![
            "projection_player_summaries",
            "projection_players",
            "projection_player_weapons",
            "projection_player_weapon_links",
        ]
    );

    let repository = InMemoryRepository::new();
    repository
        .model_store()
        .register_schema::<Players>()
        .expect("register parent includes");
    repository
        .model_store()
        .register_schema::<PlayerWeapons>()
        .expect("register child includes");
    repository
        .commit_write_plan(generated)
        .await
        .expect("one atomic four-table commit");
    assert_in_memory_rows(&repository).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_applies_the_same_four_table_server_plan() {
    use distributed::SqliteRepository;

    let (generated, fluent) = generated_and_fluent_plans();
    assert_eq!(generated, fluent);
    let repository = SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("SQLite repository");
    let mut registry = TableSchemaRegistry::new();
    registry.register::<Players>().expect("players schema");
    registry
        .register::<PlayerWeapons>()
        .expect("weapons schema");
    registry
        .register::<PlayerSummary>()
        .expect("summary schema");
    registry
        .register::<PlayerWeaponLinks>()
        .expect("join schema");
    repository
        .bootstrap_table_schema_for_dev(&registry)
        .await
        .expect("bootstrap four read models");
    repository
        .commit_write_plan(generated)
        .await
        .expect("one atomic four-table SQLite commit");

    let (player, weapon, summary, link) = expected_models();
    let mut workspace = repository.workspace();
    let hydrated = workspace
        .load::<Players>(player_key())
        .include("weapons")
        .one()
        .await
        .expect("load SQLite player graph")
        .expect("SQLite player exists");
    assert_eq!(hydrated.data.weapons, vec![weapon]);
    assert_eq!(
        Players {
            weapons: Vec::new(),
            ..hydrated.data
        },
        player
    );
    assert_eq!(
        workspace
            .load::<PlayerSummary>(player_key())
            .one()
            .await
            .expect("load SQLite summary")
            .expect("SQLite summary exists")
            .data,
        summary
    );
    assert_eq!(
        workspace
            .load::<PlayerWeaponLinks>(weapon_key())
            .one()
            .await
            .expect("load SQLite join")
            .expect("SQLite join exists")
            .data,
        link
    );
}
