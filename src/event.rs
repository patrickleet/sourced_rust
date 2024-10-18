pub trait Event: Send + Sync {
  fn event_type(&self) -> &str;
  fn get_data(&self) -> &str;
}