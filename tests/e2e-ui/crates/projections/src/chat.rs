//! Chat: mutation + portable handler (room-partitioned).

use distributed::domain_event::DomainEventContract;
use distributed::mutation_file;
use distributed::mutation_projector;
use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed::{
    bind_state_body_to_mutation, compile_portable_handlers, Mutation, ProjectionPartition,
    ProjectionProgram, ProjectionProgramError,
};
use chat_domain::ChatMessagePostedDomainEvent;
use e2e_readmodels::ChatMessages;

/// Mutation: complete-row upsert for chat messages.
///
/// Authored as GraphQL-looking syntax-only IR (not a public GraphQL field):
/// `src/mutations/save_chat_message.mutation.graphql`.
pub fn save_chat_message() -> Mutation<()> {
    mutation_file!("src/mutations/save_chat_message.mutation.graphql")
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
    // Unit partition so the lobby @live subscription can advertise resumable
    // index evidence (`live.supported = true`). Room isolation stays on the
    // GraphQL document (`where: { room_id: { _eq: "lobby" } }`). Expression
    // partitions are correct for multi-room worker sharding, but they make
    // live indexes incomparable and the client falls back to Idle.
    compile_portable_handlers(
        "project_chat_messages",
        1,
        ProjectionPartition::Unit,
        [handler],
    )
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
