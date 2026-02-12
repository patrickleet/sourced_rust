//! gRPC transport integration tests.
//!
//! Starts a tonic gRPC server and exercises it with the generated client.

use std::sync::Arc;

use serde_json::json;
use sourced_rust::microsvc::grpc::{
    CommandServiceClient, GrpcRequest, HealthRequest,
};
use sourced_rust::microsvc::Service;
use sourced_rust::{AggregateBuilder, HashMapRepository, Queueable};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

use crate::handlers;
use crate::handlers::Repo;
use crate::models::counter::Counter;

fn counter_service() -> Arc<Service<Repo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(HashMapRepository::new().queued().aggregate::<Counter>()),
        handlers::counter_create,
        handlers::counter_increment,
        handlers::whoami,
    ))
}

/// Bind to port 0, spawn the gRPC server, and return a connected client.
async fn start_server(
    service: Arc<Service<Repo>>,
) -> CommandServiceClient<tonic::transport::Channel> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let grpc_svc = sourced_rust::microsvc::grpc_server(service);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(grpc_svc)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    // Connect client
    let endpoint = format!("http://{addr}");
    CommandServiceClient::connect(endpoint).await.unwrap()
}

#[tokio::test]
async fn health_check() {
    let service = counter_service();
    let mut client = start_server(service).await;

    let resp = client
        .health(HealthRequest {})
        .await
        .unwrap()
        .into_inner();

    assert!(resp.ok);
    assert!(resp.commands.iter().any(|c| c == "counter.create"));
    assert!(resp.commands.iter().any(|c| c == "counter.increment"));
}

#[tokio::test]
async fn create_counter() {
    let service = counter_service();
    let mut client = start_server(service).await;

    let resp = client
        .dispatch(GrpcRequest {
            command: "counter.create".into(),
            input: json!({ "id": "c1" }).to_string(),
            session_variables: Default::default(),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body, json!({ "id": "c1" }));
}

#[tokio::test]
async fn create_and_increment_counter() {
    let service = counter_service();
    let mut client = start_server(service).await;

    // Create
    let resp = client
        .dispatch(GrpcRequest {
            command: "counter.create".into(),
            input: json!({ "id": "c1" }).to_string(),
            session_variables: Default::default(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.status, 200);

    // Increment
    let resp = client
        .dispatch(GrpcRequest {
            command: "counter.increment".into(),
            input: json!({ "id": "c1", "amount": 5 }).to_string(),
            session_variables: Default::default(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body, json!({ "id": "c1", "value": 5 }));

    // Increment again
    let resp = client
        .dispatch(GrpcRequest {
            command: "counter.increment".into(),
            input: json!({ "id": "c1", "amount": 3 }).to_string(),
            session_variables: Default::default(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body, json!({ "id": "c1", "value": 8 }));
}

#[tokio::test]
async fn unknown_command_returns_404_status() {
    let service = counter_service();
    let mut client = start_server(service).await;

    let resp = client
        .dispatch(GrpcRequest {
            command: "nonexistent".into(),
            input: json!({}).to_string(),
            session_variables: Default::default(),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(resp.status, 404);
}

#[tokio::test]
async fn metadata_flows_to_session() {
    let service = counter_service();
    let mut client = start_server(service).await;

    let mut request = tonic::Request::new(GrpcRequest {
        command: "whoami".into(),
        input: json!({}).to_string(),
        session_variables: Default::default(),
    });
    request
        .metadata_mut()
        .insert("x-hasura-user-id", "user-42".parse().unwrap());

    let resp = client.dispatch(request).await.unwrap().into_inner();

    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body, json!({ "user_id": "user-42" }));
}

#[tokio::test]
async fn missing_session_returns_401_status() {
    let service = counter_service();
    let mut client = start_server(service).await;

    let resp = client
        .dispatch(GrpcRequest {
            command: "whoami".into(),
            input: json!({}).to_string(),
            session_variables: Default::default(),
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(resp.status, 401);
}
