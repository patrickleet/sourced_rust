//! Session integration tests — exercises session variables through dispatch.

use serde_json::json;
use std::collections::HashMap;
use sourced_rust::microsvc::{HandlerError, Service, Session};
use sourced_rust::HashMapRepository;

#[test]
fn handler_accesses_user_id() {
    let service = Service::new(HashMapRepository::new()).command("whoami", |ctx| {
        let user_id = ctx.user_id()?;
        Ok(json!({ "user_id": user_id }))
    });

    let mut vars = HashMap::new();
    vars.insert("x-hasura-user-id".to_string(), "user-42".to_string());
    let session = Session::from_map(vars);

    let result = service.dispatch("whoami", json!({}), session).unwrap();
    assert_eq!(result, json!({ "user_id": "user-42" }));
}

#[test]
fn missing_user_id_returns_unauthorized() {
    let service = Service::new(HashMapRepository::new()).command("whoami", |ctx| {
        let _user_id = ctx.user_id()?;
        Ok(json!({}))
    });

    let result = service.dispatch("whoami", json!({}), Session::new());
    assert!(matches!(result, Err(HandlerError::Unauthorized(_))));
}
