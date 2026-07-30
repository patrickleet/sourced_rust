//! Chat domain-event projections into query models.
//!
//! `SAVE_CHAT_MESSAGE` is the event-independent mutation; `CHAT_MESSAGES`
//! remains the dual-path runtime mount until cutover.

use distributed::mutation;
use distributed::projection;
use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed::{Mutation, MutationProgram};

use chat_domain::ChatMessageState;
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

/// Portable insert-shaped state transfer for chat.
pub const CHAT_MESSAGES: ProjectionDescriptor<DirectCandidate> = projection! {
    name: "project_chat_messages";
    version: 1;
    epoch: "e2e-ui-chat-v2";
    partition: state.room_id;

    on "chat_message.posted" version 1 (state: ChatMessageState) {
        upsert ChatMessages from state as message;
    }
};

#[cfg(test)]
mod tests {
    use distributed::domain_event::DomainEventContract;
    use distributed::projection::placement::ProjectionExecutionClass;
    use distributed::{RelationalReadModel, RowValue, TableMutation};

    use super::*;
    use chat_domain::{ChatMessage, ChatMessagePostedDomainEvent};

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
        use distributed::MutationKind;

        let program = save_chat_message_program();
        assert_eq!(program.operations().len(), 1);
        assert_eq!(program.operations()[0].kind(), MutationKind::Upsert);
        assert_eq!(program.operations()[0].target().model(), "ChatMessages");
        assert_eq!(program.operations()[0].target().storage(), "chat_messages");
        let json = serde_json::to_value(&program).unwrap().to_string();
        assert!(!json.contains("event_name"));
        assert!(!json.contains("chat_message.posted"));
    }
}
