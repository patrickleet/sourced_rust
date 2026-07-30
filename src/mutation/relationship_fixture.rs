//! Multi-model relationship mutation fixture (WP8 / DMP-REQ-008).
//!
//! Framework-owned proof that one ordered mutation program can atomically
//! target parent, child, join, and summary models, and that the handler
//! catalog enforces per-owner binding uniqueness for multi-owner fan-out.

use crate::projection::{
    ProjectionEventSelector, ProjectionPartition, ProjectionTarget, ProjectionValueType,
};
use crate::{DomainEventBodyKind, DOMAIN_EVENT_OCCURRENCE_VERSION};

use super::bind::{body_field_binding, MutationEventBinding};
use super::expression::{MutationAssignment, MutationExpression};
use super::handler::{
    MutationHandlerCatalog, MutationHandlerPlacement, MutationHandlerRegistration,
};
use super::program::{
    MutationConflictTarget, MutationField, MutationKeyField, MutationKind, MutationOperation,
    MutationProgram,
};
use super::MutationProgramError;

const FP: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn string_input(path: &[&str]) -> MutationExpression {
    MutationExpression::input_path(
        ProjectionValueType::String,
        path.iter().map(|segment| (*segment).to_owned()),
    )
    .expect("input path")
}

fn u64_input(path: &[&str]) -> MutationExpression {
    MutationExpression::input_path(
        ProjectionValueType::U64,
        path.iter().map(|segment| (*segment).to_owned()),
    )
    .expect("input path")
}

/// Build the multi-model ACQUIRE-style mutation program used by the fixture.
pub fn multi_model_acquire_program() -> Result<MutationProgram, MutationProgramError> {
    let player = MutationOperation::try_new(
        "upsert-player",
        0,
        MutationKind::Upsert,
        ProjectionTarget::try_new("Players", "players")?,
        vec![MutationKeyField::try_new(
            0,
            "player_id",
            string_input(&["player", "player_id"]),
        )?],
        vec![
            MutationField::try_new(
                0,
                "player_id",
                MutationAssignment::set(string_input(&["player", "player_id"])),
            )?,
            MutationField::try_new(
                1,
                "name",
                MutationAssignment::set(string_input(&["player", "name"])),
            )?,
        ],
        Some(MutationConflictTarget::PrimaryKey),
        Vec::new(),
        Vec::new(),
        None,
    )?;
    let weapon = MutationOperation::try_new(
        "upsert-weapon",
        1,
        MutationKind::Upsert,
        ProjectionTarget::try_new("PlayerWeapons", "player_weapons")?,
        vec![
            MutationKeyField::try_new(0, "player_id", string_input(&["weapon", "player_id"]))?,
            MutationKeyField::try_new(1, "weapon_id", string_input(&["weapon", "weapon_id"]))?,
        ],
        vec![
            MutationField::try_new(
                0,
                "player_id",
                MutationAssignment::set(string_input(&["weapon", "player_id"])),
            )?,
            MutationField::try_new(
                1,
                "weapon_id",
                MutationAssignment::set(string_input(&["weapon", "weapon_id"])),
            )?,
            MutationField::try_new(
                2,
                "name",
                MutationAssignment::set(string_input(&["weapon", "name"])),
            )?,
        ],
        Some(MutationConflictTarget::PrimaryKey),
        Vec::new(),
        Vec::new(),
        None,
    )?;
    let summary = MutationOperation::try_new(
        "patch-summary",
        2,
        MutationKind::Patch,
        ProjectionTarget::try_new("AccountSummary", "account_summaries")?,
        vec![MutationKeyField::try_new(
            0,
            "account_id",
            string_input(&["account_id"]),
        )?],
        vec![MutationField::try_new(
            0,
            "equipped_weapon_count",
            MutationAssignment::set(u64_input(&["equipped_weapon_count"])),
        )?],
        None,
        Vec::new(),
        Vec::new(),
        None,
    )?;
    MutationProgram::try_new("acquire_weapon", 1, vec![player, weapon, summary])
}

fn selector() -> ProjectionEventSelector {
    ProjectionEventSelector::try_new(
        DOMAIN_EVENT_OCCURRENCE_VERSION,
        "inventory.weapon-acquired",
        1,
        DomainEventBodyKind::State,
        "WeaponAcquired",
        1,
        "urn:distributed:test:weapon-acquired:v1",
        FP,
        "distributed-json",
        1,
    )
    .expect("selector")
}

fn binding_for(program: MutationProgram) -> MutationEventBinding {
    let inputs = vec![
        body_field_binding(
            ["player", "player_id"],
            ["player_id"],
            ProjectionValueType::String,
        )
        .unwrap(),
        body_field_binding(
            ["player", "name"],
            ["player_name"],
            ProjectionValueType::String,
        )
        .unwrap(),
        body_field_binding(
            ["weapon", "player_id"],
            ["player_id"],
            ProjectionValueType::String,
        )
        .unwrap(),
        body_field_binding(
            ["weapon", "weapon_id"],
            ["weapon_id"],
            ProjectionValueType::String,
        )
        .unwrap(),
        body_field_binding(
            ["weapon", "name"],
            ["weapon_name"],
            ProjectionValueType::String,
        )
        .unwrap(),
        body_field_binding(["account_id"], ["account_id"], ProjectionValueType::String).unwrap(),
        body_field_binding(
            ["equipped_weapon_count"],
            ["equipped_weapon_count"],
            ProjectionValueType::U64,
        )
        .unwrap(),
    ];
    MutationEventBinding::try_new(selector(), inputs, program).expect("binding")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutation::cache::{
        lower_mutation_cache, MutationCacheEffect, MutationCacheVisibility,
    };
    use crate::mutation::preview::{causal_scopes, compose_event_preview};

    #[test]
    fn multi_model_program_is_ordered_and_event_free() {
        let program = multi_model_acquire_program().unwrap();
        assert_eq!(program.operations().len(), 3);
        assert_eq!(program.operations()[0].target().model(), "Players");
        assert_eq!(program.operations()[1].target().model(), "PlayerWeapons");
        assert_eq!(program.operations()[2].target().model(), "AccountSummary");
        let json = serde_json::to_value(&program).unwrap().to_string();
        assert!(!json.contains("event_name"));
        assert!(!json.contains("inventory.weapon-acquired"));
    }

    #[test]
    fn multi_model_cache_lowering_is_atomic_program() {
        let program = multi_model_acquire_program().unwrap();
        let cache = lower_mutation_cache(&program, &MutationCacheVisibility::full()).unwrap();
        assert_eq!(cache.effects().len(), 3);
        assert!(matches!(
            &cache.effects()[0],
            MutationCacheEffect::Upsert { .. }
        ));
        assert!(matches!(
            &cache.effects()[1],
            MutationCacheEffect::Upsert { .. }
        ));
        assert!(matches!(
            &cache.effects()[2],
            MutationCacheEffect::Patch { .. }
        ));
    }

    #[test]
    fn multi_owner_catalog_allows_disjoint_targets_rejects_same_model_writers() {
        let program = multi_model_acquire_program().unwrap();
        let inventory_only = {
            // Owner A takes Players + PlayerWeapons only.
            MutationProgram::try_new("inventory", 1, program.operations()[0..2].to_vec()).unwrap()
        };
        let summary_only = {
            // Rebuild summary at staging ordinal 0 for a standalone program.
            let op = &program.operations()[2];
            let rebuilt = MutationOperation::try_new(
                op.operation_id(),
                0,
                op.kind(),
                op.target().clone(),
                op.key().to_vec(),
                op.fields().to_vec(),
                op.conflict(),
                op.relationship_effects().to_vec(),
                op.invalidations().to_vec(),
                op.returning().cloned(),
            )
            .unwrap();
            MutationProgram::try_new("summary", 1, vec![rebuilt]).unwrap()
        };
        let mut catalog = MutationHandlerCatalog::new();
        catalog
            .register(
                MutationHandlerRegistration::try_new(
                    "inventory-handler",
                    1,
                    "inventory-owner",
                    "epoch-1",
                    MutationHandlerPlacement::EventualLocal,
                    ProjectionPartition::Unit,
                    binding_for(inventory_only),
                )
                .unwrap(),
            )
            .unwrap();
        catalog
            .register(
                MutationHandlerRegistration::try_new(
                    "summary-handler",
                    1,
                    "summary-owner",
                    "epoch-1",
                    MutationHandlerPlacement::EventualLocal,
                    ProjectionPartition::Unit,
                    binding_for(summary_only),
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(catalog.registrations().len(), 2);

        // Same model dual writer rejected.
        let dual = multi_model_acquire_program().unwrap();
        let err = catalog.register(
            MutationHandlerRegistration::try_new(
                "dual",
                1,
                "other-owner",
                "epoch-1",
                MutationHandlerPlacement::EventualLocal,
                ProjectionPartition::Unit,
                binding_for(dual),
            )
            .unwrap(),
        );
        assert!(err.is_err());
    }

    #[test]
    fn multi_owner_preview_retains_all_causal_scopes() {
        let program = multi_model_acquire_program().unwrap();
        let inventory_only =
            MutationProgram::try_new("inventory", 1, program.operations()[0..2].to_vec()).unwrap();
        let summary_only = {
            let op = &program.operations()[2];
            let rebuilt = MutationOperation::try_new(
                op.operation_id(),
                0,
                op.kind(),
                op.target().clone(),
                op.key().to_vec(),
                op.fields().to_vec(),
                op.conflict(),
                op.relationship_effects().to_vec(),
                op.invalidations().to_vec(),
                op.returning().cloned(),
            )
            .unwrap();
            MutationProgram::try_new("summary", 1, vec![rebuilt]).unwrap()
        };
        let a = MutationHandlerRegistration::try_new(
            "inventory-handler",
            1,
            "inventory-owner",
            "epoch-1",
            MutationHandlerPlacement::EventualLocal,
            ProjectionPartition::Unit,
            binding_for(inventory_only),
        )
        .unwrap();
        let b = MutationHandlerRegistration::try_new(
            "summary-handler",
            1,
            "summary-owner",
            "epoch-1",
            MutationHandlerPlacement::EventualLocal,
            ProjectionPartition::Unit,
            binding_for(summary_only),
        )
        .unwrap();
        let layer = compose_event_preview(&[&a, &b], &selector(), &MutationCacheVisibility::full())
            .unwrap();
        assert_eq!(layer.contributions.len(), 2);
        let scopes = causal_scopes(&layer);
        assert_eq!(scopes.len(), 3);
        let models: Vec<_> = scopes.iter().map(|scope| scope.model.as_str()).collect();
        assert!(models.contains(&"Players"));
        assert!(models.contains(&"PlayerWeapons"));
        assert!(models.contains(&"AccountSummary"));
    }

    #[test]
    fn rewrite_produces_three_projection_operations() {
        let program = multi_model_acquire_program().unwrap();
        let binding = binding_for(program);
        let arm = binding.to_projection_arm("acquired").unwrap();
        assert_eq!(arm.operations().len(), 3);
        assert_eq!(arm.operations()[0].staging_ordinal(), 0);
        assert_eq!(arm.operations()[1].staging_ordinal(), 1);
        assert_eq!(arm.operations()[2].staging_ordinal(), 2);
    }
}
