use distributed::projection::lower::{
    body_path, finish_lowering, lower_model_mutation, lower_related_mutation, model_operation,
    related_operation, state_selector, LoweredProjectionPlan, ProjectionAuthoringField,
    ProjectionLoweringError,
};
use distributed::table::TableSchemaRegistry;
use distributed::{
    Entity, InMemoryRepository, ProjectionArm, ProjectionEventSet, ProjectionExpression,
    ProjectionMutationKind, ProjectionPartition, ProjectionPlanTemplate, ProjectionProgram,
    ProjectionProgramError, ProjectionValue, ReadModelWorkspaceExt, ReadModelWritePlanBuilder,
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

struct PlayerEvents;

impl ProjectionEventSet for PlayerEvents {
    fn projection_event_selectors(
    ) -> Result<Vec<distributed::ProjectionEventSelector>, ProjectionProgramError> {
        Ok(vec![state_selector::<PlayerState>("player.loaded", 1)?])
    }
}

fn projected(field: &'static str) -> Result<ProjectionAuthoringField, ProjectionProgramError> {
    Ok(ProjectionAuthoringField::set(
        field,
        body_path::<PlayerState>(field)?,
    ))
}

fn player_program() -> Result<ProjectionProgram, ProjectionProgramError> {
    let summary = model_operation::<PlayerSummary>(
        "upsert-summary",
        0,
        ProjectionMutationKind::Upsert,
        vec![projected("player_id")?],
        vec![ProjectionAuthoringField::set(
            "weapon_count",
            ProjectionExpression::constant(ProjectionValue::unsigned(1)),
        )],
    )?;
    let player = model_operation::<Players>(
        "upsert-player",
        1,
        ProjectionMutationKind::Upsert,
        vec![projected("player_id")?],
        vec![projected("display_name")?],
    )?;
    let weapon = related_operation::<__DistributedPlayersEffectRelationship_weapons>(
        "upsert-weapon",
        2,
        ProjectionMutationKind::UpsertRelated,
        &player,
        vec![projected("weapon_id")?],
        vec![projected("acquired_at")?],
    )?;
    let link = model_operation::<PlayerWeaponLinks>(
        "insert-link",
        3,
        ProjectionMutationKind::Insert,
        vec![projected("player_id")?, projected("weapon_id")?],
        Vec::new(),
    )?;
    ProjectionProgram::try_new(
        "domain-event-player-graph",
        1,
        ProjectionPartition::Expression(body_path::<PlayerState>("player_id")?),
        vec![ProjectionArm::try_new(
            "player-loaded",
            state_selector::<PlayerState>("player.loaded", 1)?,
            vec![summary, player, weapon, link],
        )?],
    )
}

fn lower_player_program(
    plan: &distributed::ResolvedProjectionPlan,
) -> Result<LoweredProjectionPlan, ProjectionLoweringError> {
    let mut builder = ReadModelWritePlanBuilder::new();
    for mutation in plan.mutations() {
        let operation = mutation
            .provenance()
            .operation_ids()
            .first()
            .map(String::as_str)
            .expect("each resolved mutation retains an operation ID");
        match operation {
            "upsert-summary" => lower_model_mutation::<PlayerSummary>(&mut builder, mutation)?,
            "upsert-player" => lower_model_mutation::<Players>(&mut builder, mutation)?,
            "upsert-weapon" => lower_related_mutation::<
                __DistributedPlayersEffectRelationship_weapons,
            >(&mut builder, plan, mutation, "upsert-player")?,
            "insert-link" => lower_model_mutation::<PlayerWeaponLinks>(&mut builder, mutation)?,
            other => panic!("unexpected manual projection operation `{other}`"),
        }
    }
    finish_lowering(builder, plan)
}

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

fn portable_and_fluent_plans() -> (distributed::TableWritePlan, distributed::TableWritePlan) {
    let resolved = ProjectionPlanTemplate::<PlayerEvents>::try_new(
        player_program().expect("manual portable program"),
    )
    .expect("exact event set")
    .resolve(&occurrence())
    .expect("portable program resolves");
    let portable = lower_player_program(&resolved)
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
        portable,
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
    let (portable, fluent) = portable_and_fluent_plans();
    assert_eq!(portable, fluent);
    assert_eq!(
        portable
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
        .commit_write_plan(portable)
        .await
        .expect("one atomic four-table commit");
    assert_in_memory_rows(&repository).await;
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_applies_the_same_four_table_server_plan() {
    use distributed::SqliteRepository;

    let (portable, fluent) = portable_and_fluent_plans();
    assert_eq!(portable, fluent);
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
        .commit_write_plan(portable)
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
