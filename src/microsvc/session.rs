//! Session variables from the request context (e.g., Hasura session variables).

use std::collections::HashMap;

/// Parsed session variables from the incoming request.
///
/// In a Hasura + Knative setup, these come from the JWT claims forwarded
/// by Hasura as `session_variables` in the action payload:
///
/// ```json
/// {
///   "x-hasura-user-id": "user-42",
///   "x-hasura-role": "customer"
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Session {
    variables: HashMap<String, String>,
}

impl Session {
    /// Create an empty session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a session from a map of variables.
    pub fn from_map(variables: HashMap<String, String>) -> Self {
        Self { variables }
    }

    /// Get the user ID (`x-hasura-user-id`).
    pub fn user_id(&self) -> Option<&str> {
        self.get("x-hasura-user-id")
    }

    /// Get the user role (`x-hasura-role`).
    pub fn role(&self) -> Option<&str> {
        self.get("x-hasura-role")
    }

    /// Get a session variable by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.variables.get(key).map(|v| v.as_str())
    }

    /// Set a session variable.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(key.into(), value.into());
    }

    /// Check if a session variable exists.
    pub fn has(&self, key: &str) -> bool {
        self.variables.contains_key(key)
    }

    /// Get all session variables.
    pub fn variables(&self) -> &HashMap<String, String> {
        &self.variables
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_session() {
        let session = Session::new();
        assert_eq!(session.user_id(), None);
        assert_eq!(session.role(), None);
        assert!(!session.has("anything"));
    }

    #[test]
    fn hasura_variables() {
        let mut vars = HashMap::new();
        vars.insert("x-hasura-user-id".to_string(), "user-42".to_string());
        vars.insert("x-hasura-role".to_string(), "customer".to_string());
        let session = Session::from_map(vars);

        assert_eq!(session.user_id(), Some("user-42"));
        assert_eq!(session.role(), Some("customer"));
        assert!(session.has("x-hasura-user-id"));
        assert!(!session.has("x-hasura-admin-secret"));
    }

    #[test]
    fn set_and_get() {
        let mut session = Session::new();
        session.set("x-hasura-role", "admin");
        assert_eq!(session.get("x-hasura-role"), Some("admin"));
        assert_eq!(session.role(), Some("admin"));
    }
}
