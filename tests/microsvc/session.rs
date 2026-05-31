//! Session integration tests — exercises session variables through dispatch.

use distributed::microsvc::{Context, HandlerError, Service, Session};
use serde_json::json;
use std::collections::HashMap;

#[tokio::test]
async fn handler_accesses_user_id() {
    let service = Service::new(())
        .command("whoami")
        .handle(|ctx: &Context<()>| {
            let user_id = ctx.user_id().map(|id| id.to_string());
            async move {
                let user_id = user_id?;
                Ok(json!({ "user_id": user_id }))
            }
        });

    let mut vars = HashMap::new();
    vars.insert("x-hasura-user-id".to_string(), "user-42".to_string());
    let session = Session::from_map(vars);

    let result = service
        .dispatch("whoami", json!({}), session)
        .await
        .unwrap();
    assert_eq!(result, json!({ "user_id": "user-42" }));
}

#[tokio::test]
async fn missing_user_id_returns_unauthorized() {
    let service = Service::new(())
        .command("whoami")
        .handle(|ctx: &Context<()>| {
            let user_id = ctx.user_id().map(|id| id.to_string());
            async move {
                let _user_id = user_id?;
                Ok(json!({}))
            }
        });

    let result = service.dispatch("whoami", json!({}), Session::new()).await;
    assert!(matches!(result, Err(HandlerError::Unauthorized(_))));
}
