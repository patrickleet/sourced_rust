//! Outbox leaf for identity ingress. No domain commands; AuthUsers is a read model.

use distributed::{Aggregate, Entity, EventRecord};

/// Persistence leaf so Zitadel ingress can publish provider messages.
#[derive(Default)]
pub struct Identity {
    entity: Entity,
}

impl Aggregate for Identity {
    type ReplayError = std::convert::Infallible;

    fn aggregate_type() -> &'static str {
        "identity"
    }

    fn entity(&self) -> &Entity {
        &self.entity
    }

    fn entity_mut(&mut self) -> &mut Entity {
        &mut self.entity
    }

    fn replay_event(&mut self, _event: &EventRecord) -> Result<(), Self::ReplayError> {
        Ok(())
    }
}
