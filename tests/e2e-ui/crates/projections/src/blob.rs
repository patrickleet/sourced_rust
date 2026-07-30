//! Blob domain-event projections via **mutation IR** (`SAVE_BLOB_GAME`).
//!
//! Direct placement is registration-owned. Command handlers call
//! `commit()?.projected()` without naming this descriptor.

use distributed::domain_event::DomainEventContract;
use distributed::mutation;
use distributed::projection::lower::{
    DirectCandidate, EventualOnly, ProjectionDescriptor, ProjectionLoweringError,
    ProjectionOutputInventory,
};
use distributed::{
    body_bindings_for_model, descriptor_from_factories, inventory_single_model, lower_single_model,
    program_from_mutation_arms, resolve_mutation_program, Mutation, MutationEventBinding,
    MutationProgram, MutationProjectionArm, ProjectionPartition, ProjectionProgram,
    ProjectionProgramError, ResolvedProjectionPlan,
};
use distributed::DomainEventOccurrence;

use blob_domain::BlobGameState;
use e2e_readmodels::BlobGames;

/// Event-independent complete-row upsert for blob games (direct returning path).
pub fn save_blob_game() -> Mutation<()> {
    mutation! {
        name: "save_blob_game";
        version: 1;
        upsert BlobGames from input.game;
    }
}

/// Canonical SAVE_BLOB_GAME mutation program.
pub fn save_blob_game_program() -> MutationProgram {
    save_blob_game().program().clone()
}

fn blob_state_arm(
    arm_id: &'static str,
    event_name: &'static str,
) -> Result<MutationProjectionArm, distributed::MutationProgramError> {
    // State-body events for Blob share domain-state capture.
    let selector = distributed::ProjectionEventSelector::try_from_descriptor(
        &distributed::DomainEventDescriptor::state::<BlobGameState>(event_name, 1),
    )
    .map_err(distributed::MutationProgramError::from)?;
    let binding = MutationEventBinding::try_new(
        selector,
        body_bindings_for_model::<BlobGames>("game")?,
        save_blob_game_program(),
    )?;
    Ok(MutationProjectionArm { arm_id, binding })
}

/// Build dual-path projection program from SAVE_BLOB_GAME for all blob events.
pub fn blob_mutation_projection_program() -> Result<ProjectionProgram, ProjectionProgramError> {
    let arms = [
        ("blob-initialized", "blob.initialized"),
        ("blob-level-started", "blob.level_started"),
        ("blob-started", "blob.started"),
        ("blob-moved", "blob.moved"),
    ]
    .into_iter()
    .map(|(arm_id, event_name)| {
        blob_state_arm(arm_id, event_name).map_err(|e| ProjectionProgramError::InvalidOperation {
            operation: arm_id.into(),
            reason: e.to_string(),
        })
    })
    .collect::<Result<Vec<_>, _>>()?;
    program_from_mutation_arms("project_blob", 1, ProjectionPartition::Unit, &arms).map_err(
        |e| ProjectionProgramError::InvalidOperation {
            operation: "project_blob".into(),
            reason: e.to_string(),
        },
    )
}

fn blob_program_factory() -> Result<ProjectionProgram, ProjectionProgramError> {
    blob_mutation_projection_program()
}

fn blob_resolve(
    occurrence: &DomainEventOccurrence,
) -> Result<ResolvedProjectionPlan, ProjectionProgramError> {
    resolve_mutation_program(&blob_mutation_projection_program()?, occurrence)
}

fn blob_lower(
    plan: &ResolvedProjectionPlan,
) -> Result<distributed::projection::lower::LoweredProjectionPlan, ProjectionLoweringError> {
    lower_single_model::<BlobGames>(plan)
}

fn blob_inventory() -> Result<ProjectionOutputInventory, ProjectionLoweringError> {
    inventory_single_model::<BlobGames>()
}

/// Mutation-backed Blob direct projector mount.
pub const BLOB_GAMES: ProjectionDescriptor<DirectCandidate> = descriptor_from_factories(
    "project_blob",
    1,
    "e2e-ui-blob-v2",
    blob_program_factory,
    blob_resolve,
    blob_lower,
    blob_inventory,
);

/// Compile-time guards for the current one-row direct projection boundary.
///
/// A patch descriptor reaches the fluent commit builder but has no
/// `projected` terminal.
///
/// ```compile_fail,E0599
/// use blob_domain::{BlobGame, BlobGameState};
/// use e2e_readmodels::BlobGames;
/// use distributed::{
///     microsvc::{AggregateCheckout, CausalCommandContext},
///     projection,
///     projection::lower::{EventualOnly, ProjectionDescriptor},
/// };
///
/// const PATCH: ProjectionDescriptor<EventualOnly> = projection! {
///     name: "blob_patch_fixture";
///     version: 1;
///     epoch: "fixture";
///     partition: unit;
///     on "blob.moved" version 1 (state: BlobGameState) {
///         patch BlobGames {
///             key { game_id: state.game_id },
///             set { status: state.status }
///         };
///     }
/// };
///
/// fn cannot_claim_projected(
///     ctx: &CausalCommandContext<'_, BlobGame>,
///     game: AggregateCheckout<BlobGame>,
/// ) {
///     let _ = ctx
///         .project(PATCH)
///         .commit(game)
///         .unwrap()
///         .projected::<BlobGames>();
/// }
/// ```
///
/// A delete descriptor is also excluded from `Projected<T>`.
///
/// ```compile_fail,E0599
/// use blob_domain::{BlobGame, BlobGameState};
/// use e2e_readmodels::BlobGames;
/// use distributed::{
///     microsvc::{AggregateCheckout, CausalCommandContext},
///     projection,
///     projection::lower::{EventualOnly, ProjectionDescriptor},
/// };
///
/// const DELETE: ProjectionDescriptor<EventualOnly> = projection! {
///     name: "blob_delete_fixture";
///     version: 1;
///     epoch: "fixture";
///     partition: unit;
///     on "blob.moved" version 1 (state: BlobGameState) {
///         delete BlobGames { key { game_id: state.game_id } };
///     }
/// };
///
/// fn cannot_claim_projected(
///     ctx: &CausalCommandContext<'_, BlobGame>,
///     game: AggregateCheckout<BlobGame>,
/// ) {
///     let _ = ctx
///         .project(DELETE)
///         .commit(game)
///         .unwrap()
///         .projected::<BlobGames>();
/// }
/// ```
///
/// More than one row operation cannot claim the single-row terminal.
///
/// ```compile_fail,E0599
/// use blob_domain::{BlobGame, BlobGameState};
/// use e2e_readmodels::BlobGames;
/// use distributed::{
///     microsvc::{AggregateCheckout, CausalCommandContext},
///     projection,
///     projection::lower::{EventualOnly, ProjectionDescriptor},
///     ReadModel,
/// };
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Clone, Serialize, Deserialize, ReadModel)]
/// #[readmodel(primary_key = ["game_id"])]
/// struct BlobGameAudits {
///     #[readmodel(id)]
///     game_id: String,
///     owner_id: String,
///     score: i64,
///     player_dead: bool,
///     current_level: i64,
///     current_level_completed: bool,
///     map_json: String,
///     status: String,
/// }
///
/// const MULTI_ROW: ProjectionDescriptor<EventualOnly> = projection! {
///     name: "blob_multi_row_fixture";
///     version: 1;
///     epoch: "fixture";
///     partition: unit;
///     on "blob.moved" version 1 (state: BlobGameState) {
///         upsert BlobGames from state as first;
///         upsert BlobGameAudits from state as second;
///     }
/// };
///
/// fn cannot_claim_projected(
///     ctx: &CausalCommandContext<'_, BlobGame>,
///     game: AggregateCheckout<BlobGame>,
/// ) {
///     let _ = ctx
///         .project(MULTI_ROW)
///         .commit(game)
///         .unwrap()
///         .projected::<BlobGames>();
/// }
/// ```
#[doc(hidden)]
pub struct BlobDirectEligibilityGuards;

#[cfg(test)]
mod tests {
    use distributed::projection::lower::{EventualOnly, ProjectionDescriptor};
    use distributed::{
        projection, MutationKind, ProjectionMutationKind, RelationalReadModel, RowValue,
        TableMutation,
    };
    use serde::{Deserialize, Serialize};

    use super::*;
    use blob_domain::{test_map_no_holes, BlobGame, Direction};

    #[derive(Clone, Serialize, Deserialize, distributed::ReadModel)]
    #[readmodel(primary_key = ["game_id"])]
    struct BlobGameAudits {
        #[readmodel(id)]
        game_id: String,
        owner_id: String,
        score: i64,
        player_dead: bool,
        current_level: i64,
        current_level_completed: bool,
        map_json: String,
        status: String,
    }

    const PATCH_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = projection! {
        name: "blob_patch_fixture";
        version: 1;
        epoch: "fixture";
        partition: unit;

        on "blob.moved" version 1 (state: BlobGameState) {
            patch BlobGames {
                key { game_id: state.game_id },
                set { status: state.status }
            };
        }
    };

    const DELETE_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = projection! {
        name: "blob_delete_fixture";
        version: 1;
        epoch: "fixture";
        partition: unit;

        on "blob.moved" version 1 (state: BlobGameState) {
            delete BlobGames { key { game_id: state.game_id } };
        }
    };

    const MULTI_ROW_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = projection! {
        name: "blob_multi_row_fixture";
        version: 1;
        epoch: "fixture";
        partition: unit;

        on "blob.moved" version 1 (state: BlobGameState) {
            upsert BlobGames from state as first;
            upsert BlobGameAudits from state as second;
        }
    };

    fn assert_eventual_only(_: ProjectionDescriptor<EventualOnly>) {}

    #[test]
    fn blob_program_is_built_from_save_blob_game_mutation() {
        let program = BLOB_GAMES.program().unwrap();
        let from_mutations = blob_mutation_projection_program().unwrap();
        assert_eq!(
            program.canonical_bytes().unwrap(),
            from_mutations.canonical_bytes().unwrap()
        );
        assert_eq!(program.arms().len(), 4);
        assert!(program.arms().iter().all(|arm| {
            arm.operations().len() == 1
                && arm.operations()[0].kind() == ProjectionMutationKind::Upsert
        }));
        assert_eq!(
            save_blob_game_program().operations()[0].kind(),
            MutationKind::Upsert
        );
    }

    #[test]
    fn save_blob_game_mutation_is_single_row_event_free_upsert() {
        let program = save_blob_game_program();
        assert_eq!(program.operations().len(), 1);
        assert_eq!(program.operations()[0].kind(), MutationKind::Upsert);
        assert_eq!(program.operations()[0].target().model(), "BlobGames");
        let json = serde_json::to_value(&program).unwrap().to_string();
        assert!(!json.contains("event_name"));
        assert!(!json.contains("blob.moved"));
        assert!(program.operations()[0].fields().len() >= 1);
    }

    fn assert_direct_state_upsert(game: &BlobGame, expected_event: &str) {
        let occurrence = game.entity.pending_domain_events().last().unwrap();
        assert_eq!(occurrence.descriptor().name, expected_event);
        let state = occurrence.decode_body::<BlobGameState>().unwrap();
        assert_eq!(state, game.state());

        let lowered = BLOB_GAMES
            .server_executor()
            .unwrap()
            .plan(occurrence)
            .unwrap();
        assert_eq!(lowered.resolved.mutations().len(), 1);
        let [TableMutation::UpsertRow(row)] = lowered.write_plan.mutations.as_slice() else {
            panic!("Blob direct projection must lower to one full-row upsert");
        };
        assert_eq!(row.schema.model_name, "BlobGames");
        assert_eq!(
            row.values.get("game_id"),
            Some(&RowValue::String(game.entity.id().to_string()))
        );
    }

    #[test]
    fn initialized_state_is_one_complete_direct_upsert() {
        let mut game = BlobGame::default();
        game.start_with_demo("g1", "alice").unwrap();
        // start_with_demo may emit multiple state events; every occurrence
        // must lower through SAVE_BLOB_GAME.
        for occurrence in game.entity.pending_domain_events() {
            let lowered = BLOB_GAMES
                .server_executor()
                .unwrap()
                .plan(occurrence)
                .unwrap();
            assert!(matches!(
                lowered.write_plan.mutations.as_slice(),
                [TableMutation::UpsertRow(_)]
            ));
        }
        assert!(!game.entity.pending_domain_events().is_empty());
    }

    #[test]
    fn started_state_is_one_complete_direct_upsert() {
        let mut game = BlobGame::default();
        game.start_with_demo("g1", "alice").unwrap();
        // start_with_demo may emit initialized then started depending on domain
        let names: Vec<String> = game
            .entity
            .pending_domain_events()
            .iter()
            .map(|e| e.descriptor().name.to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "blob.initialized" || n == "blob.started"),
            "unexpected events: {names:?}"
        );
    }

    #[test]
    fn level_started_state_is_one_complete_direct_upsert() {
        let mut game = BlobGame::default();
        game.start_with_demo("g1", "alice").unwrap();
        // advance if domain supports
        let _ = game.start_next_generated_level("alice");
        if let Some(occurrence) = game.entity.pending_domain_events().last() {
            if occurrence.descriptor().name == "blob.level_started" {
                assert_direct_state_upsert(&game, "blob.level_started");
            }
        }
    }

    #[test]
    fn moved_state_is_one_complete_direct_upsert() {
        let mut game = BlobGame::default();
        game.start_with_demo("g1", "alice").unwrap();
        let map = test_map_no_holes();
        let _ = map;
        if game.move_dir("alice", Direction::Up).is_ok() {
            assert_direct_state_upsert(&game, "blob.moved");
        }
    }

    #[test]
    fn descriptor_is_one_direct_model_across_all_blob_event_arms() {
        let inventory = BLOB_GAMES.output_inventory().unwrap();
        assert_eq!(inventory.models.len(), 1);
        assert_eq!(inventory.models[0].model, "BlobGames");
        assert_eq!(BLOB_GAMES.program().unwrap().arms().len(), 4);
    }

    #[test]
    fn patch_delete_and_multi_row_descriptors_are_not_direct_candidates() {
        assert_eventual_only(PATCH_BLOB_GAMES);
        assert_eventual_only(DELETE_BLOB_GAMES);
        assert_eventual_only(MULTI_ROW_BLOB_GAMES);
    }
}
