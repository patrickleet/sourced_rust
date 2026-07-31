//! Chat projections: mutation + room-partitioned portable bind.

use distributed::domain_event::DomainEventContract;
use distributed::mutation;
use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed::{
    arm_state_upsert_for_model, build_mutation_projector_program, mutation_projector, Mutation,
    MutationProgram, ProjectionExpression, ProjectionPartition, ProjectionProgram,
    ProjectionProgramError, ProjectionValueType,
};
use chat_domain::ChatMessagePostedDomainEvent;
use e2e_readmodels::ChatMessages;

/// Complete-row upsert for chat messages.
pub fn save_chat_message() -> Mutation<()> {
    mutation! {
        name: "save_chat_message";
        version: 1;
        upsert ChatMessages from input.message;
    }
}

/// Canonical SAVE_CHAT_MESSAGE program.
pub fn save_chat_message_program() -> MutationProgram {
    save_chat_message().program().clone()
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

/// Projector program: chat.posted → SAVE_CHAT_MESSAGE.
pub fn chat_mutation_projection_program() -> Result<ProjectionProgram, ProjectionProgramError> {
    let arm = arm_state_upsert_for_model::<ChatMessages>(
        "chat-posted",
        &ChatMessagePostedDomainEvent::descriptor(),
        save_chat_message_program(),
        "message",
    )
    .map_err(|e| ProjectionProgramError::InvalidOperation {
        operation: "project_chat_messages".into(),
        reason: e.to_string(),
    })?;
    build_mutation_projector_program("project_chat_messages", 1, chat_partition()?, [arm])
}

mutation_projector! {
    pub const CHAT_MESSAGES: ProjectionDescriptor<DirectCandidate> = {
        name: "project_chat_messages",
        version: 1,
        epoch: "e2e-ui-chat-v2",
        model: ChatMessages,
        program: chat_mutation_projection_program,
    };
}

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
