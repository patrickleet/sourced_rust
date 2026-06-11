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

// Note: The `aggregate!` macro is now provided as a proc-macro from distributed_macros.
// It generates the event enum, TryFrom impl, apply method, and Aggregate trait impl.
// Use: aggregate!(MyAggregate, entity_field { "EventName"(args) => method_name, ... });

/// Hydrate an aggregate from an entity by replaying its events.
pub fn hydrate<A: Aggregate>(entity: Entity) -> Result<A, RepositoryError> {
    let mut agg = A::new_empty();
    *agg.entity_mut() = entity;

    // Take the events out of the entity so we can iterate them while holding a
    // mutable borrow of `agg` during replay. `hydrate` is only ever called with
    // a full-history entity, so `prefix_version == 0` and restoring via
    // `load_from_history` reproduces the exact loaded version/committed_version.
    let history = agg.entity_mut().take_events();

    let upcasters = A::upcasters();
    let events = if upcasters.is_empty() {
        // Replay directly from `history`; it is restored verbatim below, so no
        // clone of the stream is needed on the common (no-upcaster) path.
        replay_into(&mut agg, &history)?;
        history
    } else {
        // Upcasters may rewrite events for replay, but the durable history is
        // unchanged: replay from the upcasted view, then restore the originals.
        let upcasted = upcast_events(history.clone(), upcasters)
            .map_err(|err| RepositoryError::Replay(err.to_string()))?;
        replay_into(&mut agg, &upcasted)?;
        history
    };

    agg.entity_mut().load_from_history(events);

    Ok(agg)
}

fn replay_into<A: Aggregate>(agg: &mut A, events: &[EventRecord]) -> Result<(), RepositoryError> {
    agg.entity_mut().set_replaying(true);
    for event in events {
        if let Err(err) = agg.replay_event(event) {
            agg.entity_mut().set_replaying(false);
            return Err(RepositoryError::Replay(err.to_string()));
        }
    }
    agg.entity_mut().set_replaying(false);
    Ok(())
}
