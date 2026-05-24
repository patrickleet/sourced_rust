use std::sync::Arc;

use sourced_rust::microsvc::Service;

use super::{handlers, OrderRepo};

pub fn service(repo: OrderRepo) -> Arc<Service<OrderRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::order_place,
        handlers::order_add_line,
        handlers::order_change_quantity,
        handlers::order_remove_line,
        handlers::order_submit,
        handlers::order_confirm,
        handlers::order_cancel,
    ))
}

#[cfg(feature = "http")]
pub async fn start_http_service(service: Arc<Service<OrderRepo>>) -> String {
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
    service: Arc<Service<OrderRepo>>,
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
