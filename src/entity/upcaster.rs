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

/// Apply upcasters to a list of events. Chains automatically (v1->v2->v3).
pub fn upcast_events(events: Vec<EventRecord>, upcasters: &[EventUpcaster]) -> Vec<EventRecord> {
    events.into_iter()
        .map(|event| upcast_one(event, upcasters))
        .collect()
}

fn upcast_one(mut event: EventRecord, upcasters: &[EventUpcaster]) -> EventRecord {
    loop {
        let mut applied = false;
        for u in upcasters {
            if u.event_type == event.event_name && u.from_version == event.event_version {
                event.payload = (u.transform)(&event.payload);
                event.event_version = u.to_version;
                applied = true;
                break; // restart loop to handle chaining
            }
        }
        if !applied { break; }
    }
    event
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
}
