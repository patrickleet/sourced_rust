use crate::event::Event;

#[derive(Clone, Debug, PartialEq)]
pub struct LocalEvent {
    pub event_type: String,
    pub data: String,
}

impl Event for LocalEvent {
    fn event_type(&self) -> &str {
        &self.event_type
    }

    fn get_data(&self) -> &str {
        &self.data
    }
}