//! Session integration tests — exercises session variables through dispatch.

use distributed::microsvc::{Context, HandlerError, Routes, Service, Session};
use serde_json::json;
use std::collections::HashMap;

#[tokio::test]
async fn handler_accesses_user_id() {
    let service = Service::new().routes(
        Routes::new()
            .with_dependencies(())
            .command("session.identify")
            .handle(|ctx: &Context<()>| {
                let user_id = ctx.user_id().map(|id| id.to_string());
                async move {
                    let user_id = user_id?;
                    Ok(json!({ "user_id": user_id }))
                }
            }),
    );

    let mut vars = HashMap::new();
    vars.insert(
        distributed::microsvc::USER_ID_KEY.to_string(),
        "user-42".to_string(),
    );
    let session = Session::from_map(vars);

    let result = service
        .dispatch("session.identify", json!({}), session)
        .await
        .unwrap();
    assert_eq!(result, json!({ "user_id": "user-42" }));
}

#[tokio::test]
async fn missing_user_id_returns_unauthorized() {
    let service = Service::new().routes(
        Routes::new()
            .with_dependencies(())
            .command("session.identify")
            .handle(|ctx: &Context<()>| {
                let user_id = ctx.user_id().map(|id| id.to_string());
                async move {
                    let _user_id = user_id?;
                    Ok(json!({}))
                }
            }),
    );

    let result = service
        .dispatch("session.identify", json!({}), Session::new())
        .await;
    assert!(matches!(result, Err(HandlerError::Unauthorized(_))));
}
