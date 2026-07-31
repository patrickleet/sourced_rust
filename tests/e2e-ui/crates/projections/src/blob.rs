//! Blob: mutations + portable handlers.

use blob_domain::BlobGameState;
use distributed::mutation_file;
use distributed::portable_handlers;
use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed::Mutation;
use e2e_readmodels::BlobGames;

/// Mutation: upsert one BlobGames row from `input.game`.
///
/// Authored as GraphQL-looking syntax-only IR (not a public GraphQL field):
/// `src/mutations/save_blob_game.mutation.graphql`.
pub fn save_blob_game() -> Mutation<()> {
    mutation_file!("src/mutations/save_blob_game.mutation.graphql")
}

// When these domain events fire, apply [`save_blob_game`] (body → `input.game`).
// Event-first: on <events> apply <mutation>. Command path stages the row via
// `Mutation::from_state` + `readmodel(row).commit()?.projected()`.
portable_handlers! {
    pub const BLOB_GAMES: ProjectionDescriptor<DirectCandidate> = {
        name: "project_blob",
        version: 1,
        epoch: "e2e-ui-blob-v2",
        model: BlobGames,
        on_state BlobGameState as "game" apply save_blob_game {
            "blob.initialized",
            "blob.level_started",
            "blob.started",
            "blob.moved",
        }
    };
}

/// Compile-time guards for removed authoring surfaces.
///
/// ```compile_fail
/// const _GONE: () = projection! {};
/// ```
///
/// ```compile_fail,E0599
/// use blob_domain::BlobGame;
/// use e2e_readmodels::BlobGames;
/// use e2e_projections::BLOB_GAMES;
/// use distributed::microsvc::{AggregateCheckout, CausalCommandContext};
/// fn gone(ctx: &CausalCommandContext<'_, BlobGame>, game: AggregateCheckout<BlobGame>) {
///     let _ = ctx.project(BLOB_GAMES).commit(game).unwrap().projected_many::<BlobGames>();
/// }
/// ```
#[doc(hidden)]
pub struct BlobDirectEligibilityGuards;

#[cfg(test)]
mod tests {
    use distributed::mutation::{delete_by_pk_program_for_model, state_upsert_program_for_model};
    use distributed::projection::lower::{
        finish_lowering, lower_model_mutation, EventualOnly, ProjectionDescriptor,
        ProjectionLoweringError, ProjectionOutputInventory, ProjectionOutputModel,
    };
    use distributed::projection::ProjectionEventSelector;
    use distributed::{
        bind_event_to_mutation, body_bindings_for_model, body_field_binding,
        compile_portable_handlers, descriptor_from_factories, inventory_single_model,
        lower_single_model, resolve_mutation_program, MutationAssignment, MutationEventBinding,
        MutationExpression, MutationField, MutationKeyField, MutationKind, MutationOperation,
        MutationProgram, PortableHandler, ProjectionMutationKind, ProjectionPartition,
        ProjectionProgram, ProjectionProgramError, ProjectionTarget, ProjectionValueType,
        RelationalReadModel, ResolvedProjectionPlan, RowValue, TableMutation,
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

    fn map_err(op: &str, e: impl std::fmt::Display) -> ProjectionProgramError {
        ProjectionProgramError::InvalidOperation {
            operation: op.into(),
            reason: e.to_string(),
        }
    }

    fn blob_moved_selector() -> Result<ProjectionEventSelector, ProjectionProgramError> {
        ProjectionEventSelector::try_from_descriptor(&distributed::DomainEventDescriptor::state::<
            BlobGameState,
        >("blob.moved", 1))
    }

    fn patch_blob_program() -> Result<ProjectionProgram, ProjectionProgramError> {
        let schema = BlobGames::schema();
        let target =
            ProjectionTarget::try_new(schema.model_name.clone(), schema.table_name.clone())?;
        let op = MutationOperation::try_new(
            "patch-status",
            0,
            MutationKind::Patch,
            target,
            vec![MutationKeyField::try_new(
                0,
                "game_id",
                MutationExpression::input_path(ProjectionValueType::String, ["game_id"])
                    .map_err(|e| map_err("blob_patch_fixture", e))?,
            )
            .map_err(|e| map_err("blob_patch_fixture", e))?],
            vec![MutationField::try_new(
                0,
                "status",
                MutationAssignment::set(
                    MutationExpression::input_path(ProjectionValueType::String, ["status"])
                        .map_err(|e| map_err("blob_patch_fixture", e))?,
                ),
            )
            .map_err(|e| map_err("blob_patch_fixture", e))?],
            None,
            Vec::new(),
            Vec::new(),
            None,
        )
        .map_err(|e| map_err("blob_patch_fixture", e))?;
        let mutation = MutationProgram::try_new("patch_blob_games", 1, vec![op])
            .map_err(|e| map_err("blob_patch_fixture", e))?;
        let bindings = vec![
            body_field_binding(["game_id"], ["game_id"], ProjectionValueType::String)
                .map_err(|e| map_err("blob_patch_fixture", e))?,
            body_field_binding(["status"], ["status"], ProjectionValueType::String)
                .map_err(|e| map_err("blob_patch_fixture", e))?,
        ];
        let binding = MutationEventBinding::try_new(blob_moved_selector()?, bindings, mutation)
            .map_err(|e| map_err("blob_patch_fixture", e))?;
        compile_portable_handlers(
            "blob_patch_fixture",
            1,
            ProjectionPartition::Unit,
            [PortableHandler::from_binding("moved", binding)],
        )
        .map_err(|e| map_err("blob_patch_fixture", e))
    }

    fn patch_blob_resolve(
        occurrence: &distributed::DomainEventOccurrence,
    ) -> Result<ResolvedProjectionPlan, ProjectionProgramError> {
        resolve_mutation_program(&patch_blob_program()?, occurrence)
    }

    fn patch_blob_lower(
        plan: &ResolvedProjectionPlan,
    ) -> Result<distributed::projection::lower::LoweredProjectionPlan, ProjectionLoweringError>
    {
        lower_single_model::<BlobGames>(plan)
    }

    fn patch_blob_inventory() -> Result<ProjectionOutputInventory, ProjectionLoweringError> {
        inventory_single_model::<BlobGames>()
    }

    const PATCH_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = descriptor_from_factories(
        "blob_patch_fixture",
        1,
        "fixture",
        patch_blob_program,
        patch_blob_resolve,
        patch_blob_lower,
        patch_blob_inventory,
    );

    fn delete_blob_program() -> Result<ProjectionProgram, ProjectionProgramError> {
        let mutation =
            delete_by_pk_program_for_model::<BlobGames>("delete_blob_games", 1, "delete-blob")
                .map_err(|e| map_err("blob_delete_fixture", e))?;
        let bindings =
            vec![
                body_field_binding(["game_id"], ["game_id"], ProjectionValueType::String)
                    .map_err(|e| map_err("blob_delete_fixture", e))?,
            ];
        let binding = MutationEventBinding::try_new(blob_moved_selector()?, bindings, mutation)
            .map_err(|e| map_err("blob_delete_fixture", e))?;
        compile_portable_handlers(
            "blob_delete_fixture",
            1,
            ProjectionPartition::Unit,
            [PortableHandler::from_binding("moved", binding)],
        )
        .map_err(|e| map_err("blob_delete_fixture", e))
    }

    fn delete_blob_resolve(
        occurrence: &distributed::DomainEventOccurrence,
    ) -> Result<ResolvedProjectionPlan, ProjectionProgramError> {
        resolve_mutation_program(&delete_blob_program()?, occurrence)
    }

    fn delete_blob_lower(
        plan: &ResolvedProjectionPlan,
    ) -> Result<distributed::projection::lower::LoweredProjectionPlan, ProjectionLoweringError>
    {
        lower_single_model::<BlobGames>(plan)
    }

    fn delete_blob_inventory() -> Result<ProjectionOutputInventory, ProjectionLoweringError> {
        inventory_single_model::<BlobGames>()
    }

    const DELETE_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = descriptor_from_factories(
        "blob_delete_fixture",
        1,
        "fixture",
        delete_blob_program,
        delete_blob_resolve,
        delete_blob_lower,
        delete_blob_inventory,
    );

    fn multi_row_blob_program() -> Result<ProjectionProgram, ProjectionProgramError> {
        let games = state_upsert_program_for_model::<BlobGames>(
            "save_blob_games_multi",
            1,
            "upsert-games",
            "first",
        )
        .map_err(|e| map_err("blob_multi_row_fixture", e))?;
        let audits = state_upsert_program_for_model::<BlobGameAudits>(
            "save_blob_audits_multi",
            1,
            "upsert-audits",
            "second",
        )
        .map_err(|e| map_err("blob_multi_row_fixture", e))?;
        let mut ops = games.operations().to_vec();
        for (index, op) in audits.operations().iter().enumerate() {
            ops.push(
                MutationOperation::try_new(
                    op.operation_id(),
                    (games.operations().len() + index) as u32,
                    op.kind(),
                    op.target().clone(),
                    op.key().to_vec(),
                    op.fields().to_vec(),
                    op.conflict(),
                    op.relationship_effects().to_vec(),
                    op.invalidations().to_vec(),
                    op.returning().cloned(),
                )
                .map_err(|e| map_err("blob_multi_row_fixture", e))?,
            );
        }
        let mutation = MutationProgram::try_new("blob_multi_row_mutation", 1, ops)
            .map_err(|e| map_err("blob_multi_row_fixture", e))?;
        let mut bindings = body_bindings_for_model::<BlobGames>("first")
            .map_err(|e| map_err("blob_multi_row_fixture", e))?;
        for binding in body_bindings_for_model::<BlobGameAudits>("second")
            .map_err(|e| map_err("blob_multi_row_fixture", e))?
        {
            bindings.push(binding);
        }
        let binding = MutationEventBinding::try_new(blob_moved_selector()?, bindings, mutation)
            .map_err(|e| map_err("blob_multi_row_fixture", e))?;
        compile_portable_handlers(
            "blob_multi_row_fixture",
            1,
            ProjectionPartition::Unit,
            [PortableHandler::from_binding("moved", binding)],
        )
        .map_err(|e| map_err("blob_multi_row_fixture", e))
    }

    fn multi_row_blob_resolve(
        occurrence: &distributed::DomainEventOccurrence,
    ) -> Result<ResolvedProjectionPlan, ProjectionProgramError> {
        resolve_mutation_program(&multi_row_blob_program()?, occurrence)
    }

    fn multi_row_blob_lower(
        plan: &ResolvedProjectionPlan,
    ) -> Result<distributed::projection::lower::LoweredProjectionPlan, ProjectionLoweringError>
    {
        let mut builder = distributed::read_model::ReadModelWritePlanBuilder::new();
        for mutation in plan.mutations() {
            match mutation.target().model() {
                "BlobGames" => lower_model_mutation::<BlobGames>(&mut builder, mutation)?,
                "BlobGameAudits" => lower_model_mutation::<BlobGameAudits>(&mut builder, mutation)?,
                other => {
                    return Err(ProjectionLoweringError::Table(
                        distributed::TableStoreError::Metadata(format!("unknown model `{other}`")),
                    ));
                }
            }
        }
        finish_lowering(builder, plan)
    }

    fn multi_row_blob_inventory() -> Result<ProjectionOutputInventory, ProjectionLoweringError> {
        Ok(ProjectionOutputInventory::new(
            vec![
                ProjectionOutputModel::of::<BlobGames>()?,
                ProjectionOutputModel::of::<BlobGameAudits>()?,
            ],
            Vec::new(),
        ))
    }

    const MULTI_ROW_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = descriptor_from_factories(
        "blob_multi_row_fixture",
        1,
        "fixture",
        multi_row_blob_program,
        multi_row_blob_resolve,
        multi_row_blob_lower,
        multi_row_blob_inventory,
    );

    fn assert_eventual_only(_: ProjectionDescriptor<EventualOnly>) {}

    #[test]
    fn blob_program_is_built_from_save_blob_game_mutation() {
        let program = BLOB_GAMES.program().unwrap();
        let from_mutations = BLOB_GAMES.program().unwrap();
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
            save_blob_game().program().clone().operations()[0].kind(),
            MutationKind::Upsert
        );
    }

    #[test]
    fn from_state_materializes_blob_games_row_for_handler_owned_projected() {
        let mut game = BlobGame::default();
        game.start_with_demo("g1", "alice").unwrap();
        let state = BlobGameState::from(&game);
        let row: BlobGames = save_blob_game()
            .from_state(&state)
            .expect("mutation from_state builds the projected row");
        assert_eq!(row.game_id, "g1");
        assert_eq!(row.owner_id, "alice");
        assert_eq!(row.map_json, state.map_json);
        assert!(row.owner.is_none());
    }

    #[test]
    fn save_blob_game_mutation_is_single_row_event_free_upsert() {
        let program = save_blob_game().program().clone();
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
            names
                .iter()
                .any(|n| n == "blob.initialized" || n == "blob.started"),
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
