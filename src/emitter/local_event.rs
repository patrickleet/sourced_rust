/// An in-process event queued on an [`EntityEmitter`](super::EntityEmitter)
/// and emitted after a successful commit.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalEvent {
    pub event_type: String,
    pub data: String,
}
