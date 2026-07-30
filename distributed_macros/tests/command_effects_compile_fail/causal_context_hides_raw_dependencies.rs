use distributed::microsvc::CausalCommandContext;
use distributed::{Aggregate, Entity, EventRecord};

#[derive(Default)]
struct FixtureAggregate {
    entity: Entity,
}

impl Aggregate for FixtureAggregate {
    type ReplayError = String;

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

fn handler(context: &CausalCommandContext<'_, FixtureAggregate>) {
    let _ = context.dependencies();
    let _ = context.read_model_store();
}

fn main() {}
