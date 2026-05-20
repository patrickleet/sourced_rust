use std::collections::HashSet;
use std::fmt;

use super::EventRecord;

/// A stateless, pure transformation that converts an event payload from one version to another.
///
/// Upcasters are plain structs with function pointers — no traits, no boxing, no dynamic dispatch.
/// They are returned as static slices (`&'static [EventUpcaster]`) for zero allocation overhead.
pub struct EventUpcaster {
    pub event_type: &'static str,
    pub from_version: u64,
    pub to_version: u64,
    pub transform: fn(payload: &[u8]) -> Vec<u8>,
}

/// Error returned when an upcaster chain cannot make safe forward progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpcastError {
    SameVersionTransition { event_type: String, version: u64 },
    CycleDetected { event_type: String, version: u64 },
}

impl fmt::Display for UpcastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpcastError::SameVersionTransition {
                event_type,
                version,
            } => write!(
                f,
                "upcaster for event {event_type} does not advance version {version}"
            ),
            UpcastError::CycleDetected {
                event_type,
                version,
            } => write!(
                f,
                "upcaster chain for event {event_type} cycles back to version {version}"
            ),
        }
    }
}

impl std::error::Error for UpcastError {}

/// Apply upcasters to a list of events. Chains automatically (v1->v2->v3).
///
/// This compatibility helper panics when invalid upcaster configuration is
/// detected. Hydration paths use [`try_upcast_events`] so repository reads can
/// return the error instead.
pub fn upcast_events(events: Vec<EventRecord>, upcasters: &[EventUpcaster]) -> Vec<EventRecord> {
    try_upcast_events(events, upcasters).expect("invalid upcaster chain")
}

/// Fallible form of [`upcast_events`].
pub fn try_upcast_events(
    events: Vec<EventRecord>,
    upcasters: &[EventUpcaster],
) -> Result<Vec<EventRecord>, UpcastError> {
    events
        .into_iter()
        .map(|event| upcast_one(event, upcasters))
        .collect()
}

fn upcast_one(
    mut event: EventRecord,
    upcasters: &[EventUpcaster],
) -> Result<EventRecord, UpcastError> {
    let mut seen_versions = HashSet::new();
    seen_versions.insert(event.event_version);

    loop {
        let mut applied = false;
        for u in upcasters {
            if u.event_type == event.event_name && u.from_version == event.event_version {
                if u.to_version == event.event_version {
                    return Err(UpcastError::SameVersionTransition {
                        event_type: event.event_name,
                        version: event.event_version,
                    });
                }

                let next_version = u.to_version;
                event.payload = (u.transform)(&event.payload);
                event.event_version = next_version;
                if !seen_versions.insert(next_version) {
                    return Err(UpcastError::CycleDetected {
                        event_type: event.event_name,
                        version: next_version,
                    });
                }
                applied = true;
                break; // restart loop to handle chaining
            }
        }
        if !applied {
            break;
        }
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_upcasters_leaves_events_unchanged() {
        let event = EventRecord::new("TestEvent", vec![1, 2, 3], 1);
        let events = upcast_events(vec![event.clone()], &[]);
        assert_eq!(events[0].payload, vec![1, 2, 3]);
        assert_eq!(events[0].event_version, 1);
    }

    #[test]
    fn single_upcaster_transforms_matching_event() {
        let event = EventRecord::new("TestEvent", vec![1, 2], 1);
        let upcasters = [EventUpcaster {
            event_type: "TestEvent",
            from_version: 1,
            to_version: 2,
            transform: |payload| {
                let mut new = payload.to_vec();
                new.push(99);
                new
            },
        }];
        let events = upcast_events(vec![event], &upcasters);
        assert_eq!(events[0].payload, vec![1, 2, 99]);
        assert_eq!(events[0].event_version, 2);
    }

    #[test]
    fn upcaster_does_not_affect_non_matching_events() {
        let event = EventRecord::new("OtherEvent", vec![1, 2], 1);
        let upcasters = [EventUpcaster {
            event_type: "TestEvent",
            from_version: 1,
            to_version: 2,
            transform: |_| vec![99],
        }];
        let events = upcast_events(vec![event], &upcasters);
        assert_eq!(events[0].payload, vec![1, 2]);
        assert_eq!(events[0].event_version, 1);
    }

    #[test]
    fn chained_upcasters_v1_to_v3() {
        let event = EventRecord::new("TestEvent", vec![1], 1);
        let upcasters = [
            EventUpcaster {
                event_type: "TestEvent",
                from_version: 1,
                to_version: 2,
                transform: |payload| {
                    let mut new = payload.to_vec();
                    new.push(2);
                    new
                },
            },
            EventUpcaster {
                event_type: "TestEvent",
                from_version: 2,
                to_version: 3,
                transform: |payload| {
                    let mut new = payload.to_vec();
                    new.push(3);
                    new
                },
            },
        ];
        let events = upcast_events(vec![event], &upcasters);
        assert_eq!(events[0].payload, vec![1, 2, 3]);
        assert_eq!(events[0].event_version, 3);
    }

    #[test]
    fn mixed_events_some_upcasted_some_not() {
        let events = vec![
            EventRecord::new("A", vec![10], 1),
            EventRecord::new("B", vec![20], 1),
            EventRecord::new_versioned("A", vec![10, 99], 3, 2),
        ];
        let upcasters = [EventUpcaster {
            event_type: "A",
            from_version: 1,
            to_version: 2,
            transform: |payload| {
                let mut new = payload.to_vec();
                new.push(99);
                new
            },
        }];
        let result = upcast_events(events, &upcasters);
        // First A: upcasted from v1 to v2
        assert_eq!(result[0].payload, vec![10, 99]);
        assert_eq!(result[0].event_version, 2);
        // B: untouched
        assert_eq!(result[1].payload, vec![20]);
        assert_eq!(result[1].event_version, 1);
        // Second A already at v2: untouched
        assert_eq!(result[2].payload, vec![10, 99]);
        assert_eq!(result[2].event_version, 2);
    }

    #[test]
    fn try_upcast_events_rejects_same_version_transition() {
        let event = EventRecord::new("A", vec![10], 1);
        let upcasters = [EventUpcaster {
            event_type: "A",
            from_version: 1,
            to_version: 1,
            transform: |payload| payload.to_vec(),
        }];

        let err = try_upcast_events(vec![event], &upcasters).unwrap_err();

        assert_eq!(
            err,
            UpcastError::SameVersionTransition {
                event_type: "A".to_string(),
                version: 1
            }
        );
    }

    #[test]
    fn try_upcast_events_rejects_cycles() {
        let event = EventRecord::new("A", vec![10], 1);
        let upcasters = [
            EventUpcaster {
                event_type: "A",
                from_version: 1,
                to_version: 2,
                transform: |payload| payload.to_vec(),
            },
            EventUpcaster {
                event_type: "A",
                from_version: 2,
                to_version: 1,
                transform: |payload| payload.to_vec(),
            },
        ];

        let err = try_upcast_events(vec![event], &upcasters).unwrap_err();

        assert_eq!(
            err,
            UpcastError::CycleDetected {
                event_type: "A".to_string(),
                version: 1
            }
        );
    }
}
