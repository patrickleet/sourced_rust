pub mod account;
mod handlers;

use std::sync::Arc;

use sourced_rust::microsvc::Service;
use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use account::{Account, DepositMoney, OpenAccount};

pub type AccountRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Account>;

pub fn model_service(repo: AccountRepo) -> Arc<Service<AccountRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::account_open,
        handlers::account_deposit,
    ))
}

#[cfg(feature = "http")]
pub async fn start_http_service(service: Arc<Service<AccountRepo>>) -> String {
    let app = sourced_rust::microsvc::router(service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("HTTP test server should bind");
    let addr = listener
        .local_addr()
        .expect("HTTP test server should expose local address");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("HTTP test server should serve");
    });

    format!("http://{addr}")
}

#[cfg(feature = "grpc")]
pub async fn start_grpc_service(
    service: Arc<Service<AccountRepo>>,
) -> sourced_rust::microsvc::grpc::CommandServiceClient<tonic::transport::Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gRPC test server should bind");
    let addr = listener
        .local_addr()
        .expect("gRPC test server should expose local address");
    let grpc_svc = sourced_rust::microsvc::grpc_server(service);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(grpc_svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .expect("gRPC test server should serve");
    });

    sourced_rust::microsvc::grpc::CommandServiceClient::connect(format!("http://{addr}"))
        .await
        .expect("gRPC test client should connect")
}
