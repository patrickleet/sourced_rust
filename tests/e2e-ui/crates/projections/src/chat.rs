//! Chat domain-event projections into query models.

use distributed::projection;
use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};

use chat_domain::ChatMessageState;
use e2e_readmodels::ChatMessages;

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
}
