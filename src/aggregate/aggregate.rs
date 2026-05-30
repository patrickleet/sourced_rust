use std::fmt;

use crate::entity::{upcast_events, Entity, EventRecord, EventUpcaster};
use crate::repository::RepositoryError;

/// Trait for domain aggregates that can be event-sourced.
pub trait Aggregate: Sized + Default {
    type ReplayError: fmt::Display;

    /// Stable aggregate type used by stream-aware persistence.
    ///
    /// The default is a development fallback based on Rust's type name so
    /// existing examples keep working. Production persistence should override
    /// this with an explicit, durable application name.
    fn aggregate_type() -> &'static str {
        std::any::type_name::<Self>()
    }

    fn new_empty() -> Self {
        Self::default()
    }
    fn entity(&self) -> &Entity;
    fn entity_mut(&mut self) -> &mut Entity;
    fn replay_event(&mut self, event: &EventRecord) -> Result<(), Self::ReplayError>;

    /// Override to register upcasters for this aggregate's events.
    /// Upcasters are configuration, not state — this is a static method.
    fn upcasters() -> &'static [EventUpcaster] {
        &[]
    }
}

#[macro_export]
macro_rules! impl_aggregate {
    ($ty:ty, $entity:ident, $replay:ident) => {
        $crate::impl_aggregate!($ty, $entity, $replay, String);
    };
    ($ty:ty, $entity:ident, $replay:ident, aggregate_type = $aggregate_type:literal) => {
        $crate::impl_aggregate!(
            $ty,
            $entity,
            $replay,
            String,
            aggregate_type = $aggregate_type
        );
    };
    ($ty:ty, $entity:ident, $replay:ident, $err:ty) => {
        impl $crate::Aggregate for $ty {
            type ReplayError = $err;

            fn entity(&self) -> &$crate::Entity {
                &self.$entity
            }

            fn entity_mut(&mut self) -> &mut $crate::Entity {
                &mut self.$entity
            }

            fn replay_event(
                &mut self,
                event: &$crate::EventRecord,
            ) -> Result<(), Self::ReplayError> {
                Self::$replay(self, event)
            }
        }
    };
    ($ty:ty, $entity:ident, $replay:ident, $err:ty, aggregate_type = $aggregate_type:literal) => {
        impl $crate::Aggregate for $ty {
            type ReplayError = $err;

            fn aggregate_type() -> &'static str {
                $aggregate_type
            }

            fn entity(&self) -> &$crate::Entity {
                &self.$entity
            }

            fn entity_mut(&mut self) -> &mut $crate::Entity {
                &mut self.$entity
            }

            fn replay_event(
                &mut self,
                event: &$crate::EventRecord,
            ) -> Result<(), Self::ReplayError> {
                Self::$replay(self, event)
            }
        }
    };
}

// Note: The `aggregate!` macro is now provided as a proc-macro from sourced_rust_macros.
// It generates the event enum, TryFrom impl, apply method, and Aggregate trait impl.
// Use: aggregate!(MyAggregate, entity_field { "EventName"(args) => method_name, ... });

/// Hydrate an aggregate from an entity by replaying its events.
pub fn hydrate<A: Aggregate>(entity: Entity) -> Result<A, RepositoryError> {
    let mut agg = A::new_empty();
    *agg.entity_mut() = entity;

    let upcasters = A::upcasters();
    let events = if upcasters.is_empty() {
        agg.entity().events().to_vec()
    } else {
        upcast_events(agg.entity().events().to_vec(), upcasters)
            .map_err(|err| RepositoryError::Replay(err.to_string()))?
    };

    agg.entity_mut().set_replaying(true);
    for event in &events {
        if let Err(err) = agg.replay_event(event) {
            agg.entity_mut().set_replaying(false);
            return Err(RepositoryError::Replay(err.to_string()));
        }
    }
    agg.entity_mut().set_replaying(false);

    Ok(agg)
}
