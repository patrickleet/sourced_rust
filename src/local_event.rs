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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_event_type_and_data() {
        let event = LocalEvent {
            event_type: String::from("test_event"),
            data: String::from("test_data"),
        };
        assert_eq!(event.event_type(), "test_event");
        assert_eq!(event.get_data(), "test_data");
    }
}
