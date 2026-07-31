//! Chat: mutation + portable handler (room-partitioned).

use distributed::domain_event::DomainEventContract;
use distributed::mutation;
use distributed::mutation_projector;
use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed::{
    bind_state_body_to_mutation, compile_portable_handlers, Mutation, ProjectionExpression,
    ProjectionPartition, ProjectionProgram, ProjectionProgramError, ProjectionValueType,
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

fn chat_handlers() -> Result<ProjectionProgram, ProjectionProgramError> {
    let handler = bind_state_body_to_mutation::<ChatMessages>(
        &ChatMessagePostedDomainEvent::descriptor(),
        save_chat_message().program().clone(),
        "message",
    )
    .map_err(|e| ProjectionProgramError::InvalidOperation {
        operation: "project_chat_messages".into(),
        reason: e.to_string(),
    })?;
    let partition = ProjectionPartition::Expression(
        ProjectionExpression::body_path(ProjectionValueType::String, ["room_id"]).map_err(
            |e| ProjectionProgramError::InvalidOperation {
                operation: "chat partition".into(),
                reason: e.to_string(),
            },
        )?,
    );
    compile_portable_handlers("project_chat_messages", 1, partition, [handler])
}

mutation_projector! {
    pub const CHAT_MESSAGES: ProjectionDescriptor<DirectCandidate> = {
        name: "project_chat_messages",
        version: 1,
        epoch: "e2e-ui-chat-v2",
        model: ChatMessages,
        program: chat_handlers,
    };
}

#[cfg(test)]
mod tests {
    use distributed::domain_event::DomainEventContract;
    use distributed::{MutationKind, RelationalReadModel, RowValue, TableMutation};

    use super::*;
    use chat_domain::{ChatMessage, ChatMessagePostedDomainEvent};

    #[test]
    fn posted_applies_save_chat_message() {
        let mut message = ChatMessage::default();
        message.post("m1", "lobby", "alice", "hello", "1").unwrap();
        let occurrence = message.entity.pending_domain_events().last().unwrap();
        assert_eq!(
            occurrence.descriptor(),
            &ChatMessagePostedDomainEvent::descriptor()
        );
        let lowered = CHAT_MESSAGES.server_executor().unwrap().plan(occurrence).unwrap();
        let TableMutation::UpsertRow(row) = &lowered.write_plan.mutations[0] else {
            panic!("expected upsert");
        };
        assert_eq!(row.values.get("body"), Some(&RowValue::String("hello".into())));
        assert_eq!(
            save_chat_message().program().operations()[0].kind(),
            MutationKind::Upsert
        );
        assert_eq!(ChatMessages::schema().table_name, "chat_messages");
    }

    #[test]
    fn mutation_is_event_free() {
        let json = serde_json::to_value(save_chat_message().program())
            .unwrap()
            .to_string();
        assert!(!json.contains("event_name"));
    }
}
