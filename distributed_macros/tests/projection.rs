use distributed::projection::lower::{
    DirectEligible, EventualOnly, ProjectionDescriptor, ProjectionPortableType,
    ProjectionReadModelMetadata,
};
use distributed::{
    Entity, PatchMode, ProjectionProgramError, ProjectionRelationshipEffectKind,
    ReadModelWritePlanBuilder, RelationalReadModel, RowKey, RowValue, TableAdapterCapabilities,
    TableMutation,
};
use distributed_macros::{projection, DomainEvent, DomainState, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, DomainState)]
#[domain_state(version = 1)]
struct TodoState {
    todo_id: String,
    owner_id: String,
    title: String,
    completed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, DomainEvent)]
#[domain_event(name = "todo.purged", version = 1)]
struct TodoPurged {
    todo_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "todos", primary_key = ["todo_id"])]
struct Todos {
    todo_id: String,
    owner_id: String,
    title: String,
    completed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, DomainState)]
#[domain_state(version = 1)]
#[serde(rename_all = "camelCase")]
struct RenamedTodoState {
    todo_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "renamed_todos", primary_key = ["todo_id"])]
struct RenamedTodos {
    todo_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "players", primary_key = ["player_id"])]
struct Players {
    player_id: String,
    display_name: String,
    #[readmodel(has_many = "PlayerWeapons", foreign_key = "player_id")]
    weapons: Vec<PlayerWeapons>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(
    table = "player_weapons",
    primary_key = ["player_id", "weapon_id"]
)]
struct PlayerWeapons {
    #[readmodel(
        foreign_key = "players.player_id",
        delegated_from = "Players.player_id"
    )]
    player_id: String,
    weapon_id: String,
    acquired_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "account_summaries", primary_key = ["account_id"])]
struct AccountSummary {
    account_id: String,
    weapon_count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(
    table = "player_weapon_links",
    primary_key = ["player_id", "weapon_id"]
)]
struct PlayerWeaponLinks {
    #[readmodel(foreign_key = "players.player_id")]
    player_id: String,
    #[readmodel(foreign_key = "player_weapons.weapon_id")]
    weapon_id: String,
}

#[derive(Default)]
struct ProjectionAggregate {
    entity: Entity,
}

#[distributed::sourced(
    entity,
    events = "ProjectionAggregateReplayEvent",
    aggregate_type = "player"
)]
impl ProjectionAggregate {
    #[event("player.loaded", version = 1, domain = event)]
    fn load(
        &mut self,
        player_id: String,
        display_name: String,
        weapon_id: String,
        acquired_at: String,
    ) {
        self.entity.set_id(player_id);
    }

    #[event("player.weapon-removed", version = 1, domain = event)]
    fn remove_weapon(&mut self, player_id: String, display_name: String, weapon_id: String) {}

    #[event("player.purged", version = 1, domain = deleted)]
    fn purge(&mut self) {}
}

const TODO_STATE: ProjectionDescriptor<DirectEligible> = projection! {
    name: "todo-state";
    version: 1;
    epoch: "todos-v1";
    partition: unit;

    on ["todo.created", "todo.completed"] version 1 (state: TodoState) {
        upsert Todos from state as todo;
    }
};

const TODOS: ProjectionDescriptor<EventualOnly> = projection! {
    name: "todos";
    version: 1;
    epoch: "todos-v1";
    partition: unit;

    on ["todo.created", "todo.completed"] version 1 (state: TodoState) {
        upsert Todos from state as todo;
    }

    on TodoPurged(event) {
        delete Todos {
            key { todo_id: event.todo_id }
        };
    }
};

const RENAMED_TODOS: ProjectionDescriptor<DirectEligible> = projection! {
    name: "renamed-todos";
    version: 1;
    epoch: "renamed-todos-v1";
    partition: state.todo_id;

    on "todo.renamed-state" version 1 (state: RenamedTodoState) {
        upsert RenamedTodos {
            key { todo_id: state.todo_id },
            set {}
        };
    }
};

const PLAYER_GRAPH: ProjectionDescriptor<EventualOnly> = projection! {
    name: "player-graph";
    version: 1;
    epoch: "player-graph-v1";
    partition: string_concat(event.player_id, "-partition");

    on ProjectionAggregateLoadedDomainEvent(event) {
        upsert AccountSummary {
            key { account_id: event.player_id },
            set { weapon_count: 1u64 }
        };
        upsert Players {
            key { player_id: event.player_id },
            set { display_name: event.display_name }
        } as player;
        upsert_related player.weapons -> PlayerWeapons {
            key { weapon_id: event.weapon_id },
            set { acquired_at: first_present(event.acquired_at, "unknown") }
        };
        insert PlayerWeaponLinks {
            key {
                player_id: event.player_id,
                weapon_id: event.weapon_id
            },
            set {}
        };
    }
};

const PLAYER_DELETIONS: ProjectionDescriptor<EventualOnly> = projection! {
    name: "player-deletions";
    version: 1;
    epoch: "player-deletions-v1";
    partition: envelope.aggregate_id;

    on "player.purged" version 1 (deleted: ProjectionAggregateDomainIdentity) {
        delete Players {
            key { player_id: envelope.aggregate_id }
        };
    }
};

const PLAYER_WEAPON_REMOVALS: ProjectionDescriptor<EventualOnly> = projection! {
    name: "player-weapon-removals";
    version: 1;
    epoch: "player-weapon-removals-v1";
    partition: event.player_id;

    on ProjectionAggregateWeaponRemovedDomainEvent(event) {
        upsert Players {
            key { player_id: event.player_id },
            set { display_name: event.display_name }
        } as player;
        delete_related player.weapons -> PlayerWeapons {
            key { weapon_id: event.weapon_id }
        };
    }
};

const INCOMPLETE_PLAYER_ROW: ProjectionDescriptor<DirectEligible> = projection! {
    name: "incomplete-player-row";
    version: 1;
    epoch: "invalid-v1";
    partition: unit;
    on ProjectionAggregateLoadedDomainEvent(event) {
        upsert Players {
            key { player_id: event.player_id },
            set {}
        };
    }
};

const INCOMPLETE_PLAYER_KEY: ProjectionDescriptor<EventualOnly> = projection! {
    name: "incomplete-player-key";
    version: 1;
    epoch: "invalid-v1";
    partition: unit;
    on ProjectionAggregateLoadedDomainEvent(event) {
        delete Players { key {} };
    }
};

const AMBIGUOUS_PLAYER_PATCH: ProjectionDescriptor<EventualOnly> = projection! {
    name: "ambiguous-player-patch";
    version: 1;
    epoch: "invalid-v1";
    partition: unit;
    on ProjectionAggregateLoadedDomainEvent(event) {
        patch Players {
            key { player_id: event.player_id },
            set { display_name: event.display_name }
        };
        patch Players {
            key { player_id: event.player_id },
            set { display_name: "conflict" }
        };
    }
};

const PATCH_MODES: ProjectionDescriptor<EventualOnly> = projection! {
    name: "patch-modes";
    version: 1;
    epoch: "patch-modes-v1";
    partition: unit;
    on ProjectionAggregateLoadedDomainEvent(event) {
        patch Players {
            key { player_id: event.player_id },
            set { display_name: event.display_name }
        };
        upsert_patch AccountSummary {
            key { account_id: event.player_id },
            set { weapon_count: 1u64 }
        };
    }
};

#[test]
fn projection_declaration_builds_program_digest_inventory_and_executor() {
    let program = TODOS.program().expect("program");
    let id = TODOS.program_id().expect("program ID");
    let inventory = TODOS.output_inventory().expect("inventory");
    let executor = TODOS.server_executor().expect("server executor");

    assert_eq!(program.name(), "todos");
    assert_eq!(executor.program_id, id);
    assert_eq!(inventory.models.len(), 1);
    assert_eq!(inventory.models[0].model, "Todos");
    assert_eq!(inventory.models[0].storage, "todos");
    assert_eq!(inventory.models[0].schema, *Todos::schema());
}

#[test]
fn projection_exact_state_upsert_is_direct_eligible_and_metadata_is_typed() {
    assert_eq!(TODO_STATE.name(), "todo-state");
    assert_eq!(TODO_STATE.program().expect("program").arms().len(), 2);
    assert_eq!(
        Todos::PROJECTION_FIELDS[0].portable_type,
        ProjectionPortableType::String
    );
    assert_ne!(Todos::PROJECTION_SCHEMA_FINGERPRINT, "");
    let canonical = String::from_utf8(
        RENAMED_TODOS
            .program()
            .expect("renamed explicit projection")
            .canonical_bytes()
            .expect("canonical bytes"),
    )
    .expect("utf8 canonical JSON");
    assert!(canonical.contains("todoId"));
}

#[test]
fn multi_table_projection_is_byte_identical_to_the_fluent_orm_plan() {
    let mut aggregate = ProjectionAggregate::default();
    aggregate
        .load(
            "player-1".into(),
            "Ada".into(),
            "sword".into(),
            "2026-07-28T00:00:00Z".into(),
        )
        .expect("capture loaded event");
    let occurrence = aggregate
        .entity
        .pending_domain_events()
        .last()
        .expect("loaded occurrence");
    let generated = PLAYER_GRAPH
        .server_executor()
        .expect("executor")
        .plan(occurrence)
        .expect("generated physical plan")
        .write_plan;

    let player = Players {
        player_id: "player-1".into(),
        display_name: "Ada".into(),
        weapons: Vec::new(),
    };
    let weapon = PlayerWeapons {
        player_id: "player-1".into(),
        weapon_id: "sword".into(),
        acquired_at: "2026-07-28T00:00:00Z".into(),
    };
    let summary = AccountSummary {
        account_id: "player-1".into(),
        weapon_count: 1,
    };
    let link = PlayerWeaponLinks {
        player_id: "player-1".into(),
        weapon_id: "sword".into(),
    };
    let mut manual = ReadModelWritePlanBuilder::new();
    manual
        .upsert(&summary)
        .expect("summary")
        .upsert(&player)
        .expect("player")
        .upsert_related(&player, "weapons", &weapon)
        .expect("related weapon")
        .insert(&link)
        .expect("join row");
    let manual = manual.into_write_plan().expect("manual physical plan");

    assert_eq!(generated, manual);
    assert_eq!(generated.mutations[0].table_name(), "account_summaries");
    assert_eq!(generated.mutations[1].table_name(), "players");
    assert_eq!(generated.mutations[2].table_name(), "player_weapons");
    assert_eq!(generated.mutations[3].table_name(), "player_weapon_links");
}

#[test]
fn projection_sourced_deletion_selector_matches_capture_and_lowers_delete() {
    let mut aggregate = ProjectionAggregate::default();
    aggregate
        .load(
            "player-1".into(),
            "Ada".into(),
            "sword".into(),
            "2026-07-28T00:00:00Z".into(),
        )
        .expect("establish aggregate identity");
    aggregate.purge().expect("capture deletion");
    let occurrence = aggregate
        .entity
        .pending_domain_events()
        .last()
        .expect("deletion occurrence");
    let lowered = PLAYER_DELETIONS
        .server_executor()
        .expect("executor")
        .plan(occurrence)
        .expect("deletion physical plan");

    assert_eq!(lowered.write_plan.mutations.len(), 1);
    assert!(matches!(
        &lowered.write_plan.mutations[0],
        TableMutation::DeleteRow(delete)
            if delete.schema.table_name == "players"
                && delete.key.get("player_id").is_some()
    ));
}

#[test]
fn projection_relationship_delete_is_byte_identical_and_retains_unlink_provenance() {
    let mut aggregate = ProjectionAggregate::default();
    aggregate
        .load(
            "player-1".into(),
            "Ada".into(),
            "sword".into(),
            "2026-07-28T00:00:00Z".into(),
        )
        .expect("establish aggregate identity");
    aggregate
        .remove_weapon("player-1".into(), "Ada".into(), "sword".into())
        .expect("capture weapon removal");
    let occurrence = aggregate
        .entity
        .pending_domain_events()
        .last()
        .expect("weapon removal occurrence");
    let generated = PLAYER_WEAPON_REMOVALS
        .server_executor()
        .expect("executor")
        .plan(occurrence)
        .expect("relationship deletion plan");

    let player = Players {
        player_id: "player-1".into(),
        display_name: "Ada".into(),
        weapons: Vec::new(),
    };
    let child_key = RowKey::new([
        ("player_id", RowValue::String("player-1".into())),
        ("weapon_id", RowValue::String("sword".into())),
    ]);
    let mut manual = ReadModelWritePlanBuilder::new();
    manual
        .upsert(&player)
        .expect("parent")
        .delete::<PlayerWeapons>(child_key)
        .expect("child delete");

    assert_eq!(
        generated.write_plan,
        manual.into_write_plan().expect("manual plan")
    );
    let unlink = generated
        .resolved
        .mutations()
        .iter()
        .flat_map(|mutation| mutation.provenance().relationship_effects())
        .find(|effect| effect.kind() == ProjectionRelationshipEffectKind::Unlink);
    assert!(
        unlink.is_some(),
        "relationship deletion retains unlink proof"
    );
}

#[test]
fn projection_generated_path_rejects_shape_ambiguity_and_capabilities_before_io() {
    assert!(matches!(
        INCOMPLETE_PLAYER_ROW.program(),
        Err(ProjectionProgramError::InvalidOperation { .. })
    ));
    assert!(matches!(
        INCOMPLETE_PLAYER_KEY.program(),
        Err(ProjectionProgramError::InvalidOperation { .. })
    ));
    assert!(matches!(
        AMBIGUOUS_PLAYER_PATCH.program(),
        Err(ProjectionProgramError::AmbiguousMutation { .. })
    ));

    let mut aggregate = ProjectionAggregate::default();
    aggregate
        .load(
            "player-1".into(),
            "Ada".into(),
            "sword".into(),
            "2026-07-28T00:00:00Z".into(),
        )
        .expect("capture event");
    let occurrence = aggregate
        .entity
        .pending_domain_events()
        .last()
        .expect("occurrence");
    let lowered = PATCH_MODES
        .server_executor()
        .expect("executor")
        .plan(occurrence)
        .expect("lowered patch modes");
    let modes = lowered
        .write_plan
        .mutations
        .iter()
        .filter_map(|mutation| match mutation {
            TableMutation::PatchRow(patch) => Some(patch.mode.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        modes,
        vec![PatchMode::InsertMissing, PatchMode::UpdateExisting]
    );
    assert!(
        lowered
            .validate_for(&TableAdapterCapabilities {
                relational_rows: false,
                sparse_patches: false,
                deletes: false,
            })
            .is_err(),
        "unsupported adapter capabilities fail during planning, before I/O"
    );
}
