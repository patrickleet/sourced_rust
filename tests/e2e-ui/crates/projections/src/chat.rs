//! Chat: mutation + portable handler (room-partitioned).
//!
//! Partition is custom (body.room_id), so handlers are compiled with
//! [`compile_portable_handlers`] and mounted via [`mutation_projector!`].

use distributed::domain_event::DomainEventContract;
use distributed::mutation;
use distributed::mutation_projector;
use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed::{
    bind_state_body_to_mutation, compile_portable_handlers, Mutation, MutationProgram,
    ProjectionExpression, ProjectionPartition, ProjectionProgram, ProjectionProgramError,
    ProjectionValueType,
};
use chat_domain::ChatMessagePostedDomainEvent;
use e2e_readmodels::ChatMessages;

/// Mutation: complete-row upsert for chat messages.
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

/// Portable handler: chat.posted → apply [`save_chat_message`] (body → input.message).
pub fn chat_mutation_projection_program() -> Result<ProjectionProgram, ProjectionProgramError> {
    let handler = bind_state_body_to_mutation::<ChatMessages>(
        &ChatMessagePostedDomainEvent::descriptor(),
        save_chat_message_program(),
        "message",
    )
    .map_err(|e| ProjectionProgramError::InvalidOperation {
        operation: "project_chat_messages".into(),
        reason: e.to_string(),
    })?;
    compile_portable_handlers(
        "project_chat_messages",
        1,
        chat_partition()?,
        [handler],
    )
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
        assert_eq!(program.arms().len(), 1);
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
        assert_eq!(
            CHAT_MESSAGES.eventual().execution_class(),
            ProjectionExecutionClass::Causal
        );
        assert_eq!(ChatMessages::schema().table_name, "chat_messages");
    }

    #[test]
    fn save_chat_message_mutation_is_event_free() {
        let json = serde_json::to_value(&save_chat_message_program())
            .unwrap()
            .to_string();
        assert!(!json.contains("event_name"));
        assert!(!json.contains("chat_message.posted"));
    }
}
