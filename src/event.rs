pub trait Event: Send + Sync {
  fn event_type(&self) -> &str;
  fn get_data(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_trait() {
        // This is a placeholder test to ensure the Event trait is defined.
        // Since the Event trait is a marker trait, we can't directly test its methods.
        // However, we can test that a struct implementing the trait compiles correctly.
        struct TestEvent;

        impl Event for TestEvent {
            fn event_type(&self) -> &str {
                "test_event"
            }

            fn get_data(&self) -> &str {
                "test_data"
            }
        }

        let event = TestEvent;
        assert_eq!(event.event_type(), "test_event");
        assert_eq!(event.get_data(), "test_data");
    }
}
