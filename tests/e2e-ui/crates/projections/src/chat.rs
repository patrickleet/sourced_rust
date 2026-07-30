//! Chat domain-event projections via **mutation IR** (`SAVE_CHAT_MESSAGE`).

use distributed::domain_event::DomainEventContract;
use distributed::mutation;
use distributed::projection::lower::{
    DirectCandidate, ProjectionDescriptor, ProjectionLoweringError, ProjectionOutputInventory,
};
use distributed::{
    body_bindings_for_model, descriptor_from_factories, inventory_single_model, lower_single_model,
    program_from_mutation_arms, resolve_mutation_program, Mutation, MutationEventBinding,
    MutationProgram, MutationProjectionArm, ProjectionExpression, ProjectionPartition,
    ProjectionProgram, ProjectionProgramError, ProjectionValueType, ResolvedProjectionPlan,
};
use distributed::DomainEventOccurrence;

use chat_domain::ChatMessagePostedDomainEvent;
use e2e_readmodels::ChatMessages;

/// Event-independent complete-row upsert for chat messages.
pub fn save_chat_message() -> Mutation<()> {
    mutation! {
        name: "save_chat_message";
        version: 1;
        upsert ChatMessages from input.message;
    }
}

/// Canonical SAVE_CHAT_MESSAGE mutation program.
pub fn save_chat_message_program() -> MutationProgram {
    save_chat_message().program().clone()
}

fn chat_field_bindings(
) -> Result<Vec<distributed::MutationInputBinding>, distributed::MutationProgramError> {
    body_bindings_for_model::<ChatMessages>("message")
}

fn chat_partition() -> Result<ProjectionPartition, ProjectionProgramError> {
    Ok(ProjectionPartition::Expression(
        ProjectionExpression::body_path(ProjectionValueType::String, ["room_id"]).map_err(
            |e| ProjectionProgramError::InvalidOperation {
                operation: "chat partition".into(),
                reason: e.to_string(),
            },
        )?,
    ))
}

fn chat_arm() -> Result<MutationProjectionArm, distributed::MutationProgramError> {
    let selector = distributed::ProjectionEventSelector::try_from_descriptor(
        &ChatMessagePostedDomainEvent::descriptor(),
    )
    .map_err(distributed::MutationProgramError::from)?;
    let binding =
        MutationEventBinding::try_new(selector, chat_field_bindings()?, save_chat_message_program())?;
    Ok(MutationProjectionArm {
        arm_id: "chat-posted",
        binding,
    })
}

/// Build dual-path projection program from SAVE_CHAT_MESSAGE.
pub fn chat_mutation_projection_program() -> Result<ProjectionProgram, ProjectionProgramError> {
    let arms = vec![chat_arm().map_err(|e| ProjectionProgramError::InvalidOperation {
        operation: "chat mutation arm".into(),
        reason: e.to_string(),
    })?];
    program_from_mutation_arms("project_chat_messages", 1, chat_partition()?, &arms).map_err(
        |e| ProjectionProgramError::InvalidOperation {
            operation: "project_chat_messages".into(),
            reason: e.to_string(),
        },
    )
}

fn chat_program_factory() -> Result<ProjectionProgram, ProjectionProgramError> {
    chat_mutation_projection_program()
}

fn chat_resolve(
    occurrence: &DomainEventOccurrence,
) -> Result<ResolvedProjectionPlan, ProjectionProgramError> {
    resolve_mutation_program(&chat_mutation_projection_program()?, occurrence)
}

fn chat_lower(
    plan: &ResolvedProjectionPlan,
) -> Result<distributed::projection::lower::LoweredProjectionPlan, ProjectionLoweringError> {
    lower_single_model::<ChatMessages>(plan)
}

fn chat_inventory() -> Result<ProjectionOutputInventory, ProjectionLoweringError> {
    inventory_single_model::<ChatMessages>()
}

/// Mutation-backed Chat projector (DirectCandidate for completeness of mount type).
pub const CHAT_MESSAGES: ProjectionDescriptor<DirectCandidate> = descriptor_from_factories(
    "project_chat_messages",
    1,
    "e2e-ui-chat-v2",
    chat_program_factory,
    chat_resolve,
    chat_lower,
    chat_inventory,
);

#[cfg(test)]
mod tests {
    use distributed::domain_event::DomainEventContract;
    use distributed::projection::placement::ProjectionExecutionClass;
    use distributed::{MutationKind, RelationalReadModel, RowValue, TableMutation};

    use super::*;
    use chat_domain::{ChatMessage, ChatMessagePostedDomainEvent};

    #[test]
    fn chat_program_comes_from_save_chat_message_mutation() {
        let program = CHAT_MESSAGES.program().unwrap();
        let from_mutations = chat_mutation_projection_program().unwrap();
        assert_eq!(
            program.canonical_bytes().unwrap(),
            from_mutations.canonical_bytes().unwrap()
        );
        assert_eq!(program.arms().len(), 1);
        assert_eq!(
            program.arms()[0].operations()[0].kind(),
            distributed::ProjectionMutationKind::Upsert
        );
        assert_eq!(
            save_chat_message_program().operations()[0].kind(),
            MutationKind::Upsert
        );
    }

    #[test]
    fn posted_state_preserves_semantic_name_and_lowers_to_complete_upsert() {
        let mut message = ChatMessage::default();
        message.post("m1", "lobby", "alice", "hello", "1").unwrap();
        let occurrence = message.entity.pending_domain_events().last().unwrap();
        assert_eq!(
            occurrence.descriptor(),
            &ChatMessagePostedDomainEvent::descriptor()
        );

        let lowered = CHAT_MESSAGES
            .server_executor()
            .unwrap()
            .plan(occurrence)
            .unwrap();
        let TableMutation::UpsertRow(row) = &lowered.write_plan.mutations[0] else {
            panic!("posted state must lower to upsert");
        };
        assert_eq!(
            row.values.get("body"),
            Some(&RowValue::String("hello".into()))
        );
        assert_eq!(ChatMessages::schema().table_name, "chat_messages");
        assert_eq!(
            CHAT_MESSAGES.eventual().execution_class(),
            ProjectionExecutionClass::Causal
        );
    }

    #[test]
    fn projection_has_one_insert_shaped_arm_and_stable_output_inventory() {
        let program = CHAT_MESSAGES.program().unwrap();
        let inventory = CHAT_MESSAGES.output_inventory().unwrap();

        assert_eq!(program.arms().len(), 1);
        assert_eq!(inventory.models.len(), 1);
        assert_eq!(inventory.models[0].model, "ChatMessages");
        assert_eq!(inventory.models[0].storage, "chat_messages");
    }

    #[test]
    fn save_chat_message_mutation_is_event_free_complete_upsert() {
        let program = save_chat_message_program();
        assert_eq!(program.operations().len(), 1);
        assert_eq!(program.operations()[0].kind(), MutationKind::Upsert);
        assert_eq!(program.operations()[0].target().model(), "ChatMessages");
        let json = serde_json::to_value(&program).unwrap().to_string();
        assert!(!json.contains("event_name"));
        assert!(!json.contains("chat_message.posted"));
    }
}
