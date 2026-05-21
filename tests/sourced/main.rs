mod aggregate;

use aggregate::{Todo, TodoEvent};
use serde::ser::Error as _;
use serde::Serialize;
use sourced_rust::{
    Aggregate, AggregateBuilder, Entity, EventRecord, EventRecordError, HashMapRepository,
    Queueable,
};

#[derive(Clone)]
struct FailingSerialize;

impl Serialize for FailingSerialize {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(S::Error::custom("intentional serialization failure"))
    }
}

#[derive(Default)]
struct SafeRecorder {
    entity: Entity,
    applied: bool,
}

impl SafeRecorder {
    #[sourced_rust::digest("Recorded")]
    fn record(&mut self, _payload: FailingSerialize) {
        self.applied = true;
    }

    #[sourced_rust::digest("Recorded", version = 2)]
    fn record_ok(&mut self, payload: String) {
        self.applied = true;
        assert_eq!(payload, "ok");
    }

    #[sourced_rust::digest("TailChecked")]
    fn record_after_tail_check(&mut self) {
        self.tail_check()?
    }

    fn tail_check(&self) -> sourced_rust::SourcedResult {
        Ok(())
    }
}

#[derive(Debug)]
enum ValidationError {
    EmptyTitle,
    EventRecord(EventRecordError),
}

impl From<EventRecordError> for ValidationError {
    fn from(err: EventRecordError) -> Self {
        Self::EventRecord(err)
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyTitle => write!(f, "title cannot be empty"),
            Self::EventRecord(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ValidationError {}

#[derive(Default)]
struct ExplicitlyValidatedRecorder {
    entity: Entity,
    title: String,
}

impl ExplicitlyValidatedRecorder {
    fn rename(&mut self, title: String) -> Result<(), ValidationError> {
        if title.trim().is_empty() {
            return Err(ValidationError::EmptyTitle);
        }

        self.entity.digest("Renamed", &(title.clone(),))?;
        self.title = title;
        Ok(())
    }
}

#[test]
fn enum_variants_exist_and_compile() {
    let init = TodoEvent::Initialized {
        id: "1".into(),
        user_id: "alice".into(),
        task: "Buy milk".into(),
    };
    let completed = TodoEvent::Completed;

    // Debug and Clone derives work
    let _ = format!("{:?}", init);
    let _ = init.clone();
    let _ = format!("{:?}", completed);
}

#[test]
fn digest_macro_returns_payload_errors_without_running_body() {
    let mut recorder = SafeRecorder::default();

    let err = recorder.record(FailingSerialize).unwrap_err();

    assert!(err.message.contains("intentional serialization failure"));
    assert!(!recorder.applied);
    assert!(recorder.entity.events().is_empty());
}

#[test]
fn explicit_digest_validates_before_recording_events() {
    let mut recorder = ExplicitlyValidatedRecorder::default();

    let err = recorder.rename("   ".to_string()).unwrap_err();

    assert!(matches!(err, ValidationError::EmptyTitle));
    assert!(recorder.entity.events().is_empty());
    assert!(recorder.title.is_empty());
}

#[test]
fn digest_macro_records_successful_versioned_events() {
    let mut recorder = SafeRecorder::default();

    recorder.record_ok("ok".to_string()).unwrap();

    assert!(recorder.applied);
    assert_eq!(recorder.entity.events().len(), 1);
    assert_eq!(recorder.entity.events()[0].event_name, "Recorded");
    assert_eq!(recorder.entity.events()[0].event_version, 2);
}

#[test]
fn digest_macro_accepts_inferred_tail_try_expression() {
    let mut recorder = SafeRecorder::default();

    recorder.record_after_tail_check().unwrap();

    assert_eq!(recorder.entity.events().len(), 1);
    assert_eq!(recorder.entity.events()[0].event_name, "TailChecked");
}

#[test]
fn event_name_returns_correct_strings() {
    let init = TodoEvent::Initialized {
        id: "1".into(),
        user_id: "alice".into(),
        task: "Buy milk".into(),
    };
    assert_eq!(init.event_name(), "Initialized");

    let completed = TodoEvent::Completed;
    assert_eq!(completed.event_name(), "Completed");
}

#[test]
fn try_from_event_record_initialized() {
    let payload = bitcode::serialize(&(
        "t1".to_string(),
        "alice".to_string(),
        "Buy milk".to_string(),
    ))
    .unwrap();
    let record = EventRecord::new("Initialized", payload, 1);
    let event = TodoEvent::try_from(&record).unwrap();
    match event {
        TodoEvent::Initialized { id, user_id, task } => {
            assert_eq!(id, "t1");
            assert_eq!(user_id, "alice");
            assert_eq!(task, "Buy milk");
        }
        _ => panic!("Expected Initialized variant"),
    }
}

#[test]
fn try_from_event_record_completed() {
    let record = EventRecord::new("Completed", vec![], 2);
    let event = TodoEvent::try_from(&record).unwrap();
    assert_eq!(event, TodoEvent::Completed);
}

#[test]
fn try_from_unknown_event_returns_error() {
    let record = EventRecord::new("Unknown", vec![], 1);
    let result = TodoEvent::try_from(&record);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown event"));
}

#[test]
fn aggregate_hydration_roundtrip() {
    let repo = HashMapRepository::new().queued().aggregate::<Todo>();

    let mut todo = Todo::default();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into())
        .unwrap();
    todo.complete().unwrap();

    repo.commit(&mut todo).unwrap();

    let loaded = repo.get("t1").unwrap().unwrap();
    assert_eq!(loaded.snapshot().id, "t1");
    assert_eq!(loaded.snapshot().user_id, "alice");
    assert_eq!(loaded.snapshot().task, "Buy milk");
    assert!(loaded.snapshot().completed);
}

#[test]
fn guard_condition_works() {
    let mut todo = Todo::default();
    todo.initialize("t1".into(), "alice".into(), "Test".into())
        .unwrap();
    todo.complete().unwrap();
    // Second complete should be no-op (guard: !self.completed)
    todo.complete().unwrap();

    assert_eq!(todo.entity.version(), 2); // only Initialized + Completed
}

#[test]
fn sourced_event_macro_accepts_inferred_tail_try_expression() {
    let mut todo = Todo::default();

    todo.validate_tail().unwrap();

    assert_eq!(todo.entity.events().len(), 1);
    assert_eq!(todo.entity.events()[0].event_name, "TailValidated");
}

#[test]
fn non_event_methods_pass_through() {
    let mut todo = Todo::default();
    todo.initialize("t1".into(), "alice".into(), "Test".into())
        .unwrap();
    let snap = todo.snapshot();
    assert_eq!(snap.id, "t1");
    assert_eq!(snap.user_id, "alice");
}

#[test]
fn partial_equality_on_enum() {
    let a = TodoEvent::Initialized {
        id: "1".into(),
        user_id: "alice".into(),
        task: "Test".into(),
    };
    let b = TodoEvent::Initialized {
        id: "1".into(),
        user_id: "alice".into(),
        task: "Test".into(),
    };
    let c = TodoEvent::Completed;

    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn no_upcasters_by_default() {
    assert!(Todo::upcasters().is_empty());
}
