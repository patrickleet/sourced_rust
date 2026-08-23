//! HTTP/gRPC causal wait-path and Bus send-has-no-reply.
#![cfg(all(feature = "graphql", feature = "http"))]

use std::sync::Arc;

use distributed::bus::{Bus, BusConsumer, InMemoryBus, TransportError};
use distributed::graphql::{
    typed_command, GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField, Succeeded,
};
use distributed::microsvc::{router, Routes, Service, ROLE_KEY, USER_ID_KEY};
use distributed::{Aggregate, AggregateBuilder, Entity, InMemoryRepository, Snapshot};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Snapshot)]
struct WaitAgg {
    entity: Entity,
}

impl WaitAgg {
    fn record(&mut self, id: String) -> distributed::SourcedResult {
        self.entity.set_id(id);
        self.entity.digest_empty("wait.recorded")
    }
}

impl Aggregate for WaitAgg {
    type ReplayError = std::convert::Infallible;

    fn aggregate_type() -> &'static str {
        "causal-wait-path"
    }

    fn entity(&self) -> &Entity {
        &self.entity
    }

    fn entity_mut(&mut self) -> &mut Entity {
        &mut self.entity
    }

    fn replay_event(&mut self, _event: &distributed::EventRecord) -> Result<(), Self::ReplayError> {
        Ok(())
    }
}

#[derive(Deserialize)]
struct IdInput {
    id: String,
}

impl GraphqlInputType for IdInput {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "IdInput",
            vec![GraphqlTypeField {
                name: "id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        )
        .with_type_id(std::any::TypeId::of::<Self>())
    }
}

#[derive(Serialize)]
struct IdPayload {
    id: String,
}

impl GraphqlOutputType for IdPayload {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "IdPayload",
            vec![GraphqlTypeField {
                name: "id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        )
        .with_type_id(std::any::TypeId::of::<Self>())
    }
}

fn wait_service() -> Arc<Service> {
    let causal = Routes::new()
        .with_repo(InMemoryRepository::new().aggregate::<WaitAgg>())
        .typed_command(
            typed_command::<IdInput, Succeeded<IdPayload>>("todo.create").roles(["user"]),
        )
        .create()
        .invoke(|aggregate, input, _owner| {
            aggregate.record(input.id.clone())?;
            Ok::<_, distributed::EventRecordError>(())
        })
        .succeeded(|aggregate| IdPayload {
            id: aggregate.entity().id().to_string(),
        })
        .typed_command(
            typed_command::<IdInput, Succeeded<IdPayload>>("todo.admin_only").roles(["admin"]),
        )
        .create()
        .invoke(|aggregate, input, _owner| {
            aggregate.record(input.id.clone())?;
            Ok::<_, distributed::EventRecordError>(())
        })
        .succeeded(|aggregate| IdPayload {
            id: aggregate.entity().id().to_string(),
        });
    let ping = Routes::new()
        .with_dependencies(())
        .command("ping")
        .handle(|_ctx: &distributed::microsvc::Context<'_, ()>| async {
            Ok(json!({ "pong": true }))
        });
    Arc::new(
        Service::new()
            .named("causal-wait-path")
            .with_http_command_routes()
            .routes(causal)
            .routes(ping),
    )
}

async fn start_http(service: Arc<Service>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(service);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn http_wait_path_returns_command_id_and_receipt() {
    let base = start_http(wait_service()).await;
    let client = reqwest::Client::new();
    let command_id = "0190a000-0000-7000-8000-000000000101";
    let resp = client
        .post(format!("{base}/todo.create"))
        .header(USER_ID_KEY, "alice")
        .header(ROLE_KEY, "user")
        .json(&json!({
            "commandId": command_id,
            "input": { "id": "todo-wait-1" },
            "session_variables": { "x-roles": "admin" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["payload"], json!({ "id": "todo-wait-1" }));
    assert_eq!(body["receipt"]["commandId"], command_id);
    assert_eq!(body["receipt"]["state"], "succeeded");
    assert!(body["receipt"]["causationId"].as_str().unwrap().len() > 0);
}

#[tokio::test]
async fn http_wait_path_ignores_spoofed_body_roles() {
    let base = start_http(wait_service()).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/todo.admin_only"))
        .header(USER_ID_KEY, "alice")
        .header(ROLE_KEY, "user")
        .json(&json!({
            "commandId": "0190a000-0000-7000-8000-000000000102",
            "input": { "id": "todo-admin" },
            "session_variables": { "x-roles": "admin" },
            "roles": "admin"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "{}", resp.text().await.unwrap());
}

#[tokio::test]
async fn bus_send_has_no_reply_value() {
    let bus = InMemoryBus::new();
    let result: Result<(), TransportError> = bus.send("ping", b"{}".to_vec()).await;
    result.expect("send is fire-and-forget");
}

#[tokio::test]
async fn same_host_listen_ping_and_http_wait_path() {
    let bus = InMemoryBus::new();
    let service = Arc::new(
        Service::new()
            .named("causal-wait-path")
            .with_http_command_routes()
            .routes(
                Routes::new()
                    .with_repo(InMemoryRepository::new().aggregate::<WaitAgg>())
                    .typed_command(
                        typed_command::<IdInput, Succeeded<IdPayload>>("todo.create")
                            .roles(["user"]),
                    )
                    .create()
                    .invoke(|aggregate, input, _owner| {
                        aggregate.record(input.id.clone())?;
                        Ok::<_, distributed::EventRecordError>(())
                    })
                    .succeeded(|aggregate| IdPayload {
                        id: aggregate.entity().id().to_string(),
                    }),
            )
            .routes(
                Routes::new()
                    .with_dependencies(())
                    .command("ping")
                    .handle(|_ctx: &distributed::microsvc::Context<'_, ()>| async {
                        Ok(json!({ "pong": true }))
                    }),
            )
            .with_bus(bus.clone()),
    );
    {
        let bus = bus.clone();
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            let _ = bus
                .listen(service, distributed::bus::RunOptions::default())
                .await;
        });
    }
    bus.send("ping", b"{}".to_vec()).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let base = start_http(Arc::clone(&service)).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/todo.create"))
        .header(USER_ID_KEY, "alice")
        .header(ROLE_KEY, "user")
        .json(&json!({
            "commandId": "0190a000-0000-7000-8000-000000000103",
            "input": { "id": "todo-host-1" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
}

#[cfg(feature = "grpc")]
#[tokio::test]
async fn grpc_wait_path_returns_command_id_and_receipt() {
    use distributed::microsvc::grpc::{CommandServiceClient, GrpcRequest};
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;

    let service = wait_service();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let grpc_svc = distributed::microsvc::grpc_server(service);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(grpc_svc)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let mut client = CommandServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let command_id = "0190a000-0000-7000-8000-000000000104";
    let mut request = tonic::Request::new(GrpcRequest {
        command: "todo.create".into(),
        input: json!({
            "commandId": command_id,
            "input": { "id": "todo-grpc-1" },
            "session_variables": { "x-roles": "admin" }
        })
        .to_string(),
        session_variables: Default::default(),
    });
    request
        .metadata_mut()
        .insert(USER_ID_KEY, "alice".parse().unwrap());
    request
        .metadata_mut()
        .insert(ROLE_KEY, "user".parse().unwrap());
    let resp = client.dispatch(request).await.unwrap().into_inner();
    assert_eq!(resp.status, 200, "{}", resp.body);
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body["payload"], json!({ "id": "todo-grpc-1" }));
    assert_eq!(body["receipt"]["commandId"], command_id);
    assert_eq!(body["receipt"]["state"], "succeeded");
}
