//! gRPC transport for microsvc — maps gRPC requests to command dispatch.
//!
//! Requires the `grpc` feature. Uses tonic for the gRPC server and prost
//! for message serialization (standard protobuf wire format, no `.proto` file).
//!
//! ## RPCs
//!
//! - `Dispatch` — dispatch a command. Input = `GrpcRequest`, output = `GrpcResponse`.
//! - `Health` — health check returning available commands.
//!
//! ## Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use sourced_rust::{microsvc, HashMapRepository};
//!
//! let service = Arc::new(
//!     microsvc::Service::new(HashMapRepository::new())
//!         .command("counter.create", |ctx| { /* ... */ })
//! );
//!
//! // Get the server to compose with other tonic routes
//! let grpc_svc = microsvc::grpc_server(service.clone());
//!
//! // Or serve directly
//! microsvc::serve_grpc(service, "[::1]:50051").await?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;
use tonic::{Request, Response, Status};

use super::service::Service;
use super::session::Session;

// ---------------------------------------------------------------------------
// Message types (prost — standard protobuf wire format)
// ---------------------------------------------------------------------------

#[derive(Clone, prost::Message)]
pub struct GrpcRequest {
    #[prost(string, tag = "1")]
    pub command: String,
    #[prost(string, tag = "2")]
    pub input: String, // JSON string
    #[prost(map = "string, string", tag = "3")]
    pub session_variables: HashMap<String, String>,
}

#[derive(Clone, prost::Message)]
pub struct GrpcResponse {
    #[prost(uint32, tag = "1")]
    pub status: u32,
    #[prost(string, tag = "2")]
    pub body: String, // JSON string
}

#[derive(Clone, prost::Message)]
pub struct HealthRequest {}

#[derive(Clone, prost::Message)]
pub struct HealthResponse {
    #[prost(bool, tag = "1")]
    pub ok: bool,
    #[prost(string, repeated, tag = "2")]
    pub commands: Vec<String>,
}

// ---------------------------------------------------------------------------
// Generated service trait + server/client
// ---------------------------------------------------------------------------

include!(concat!(
    env!("OUT_DIR"),
    "/sourced.microsvc.CommandService.rs"
));

pub use command_service_client::CommandServiceClient;
pub use command_service_server::{CommandService, CommandServiceServer};

// ---------------------------------------------------------------------------
// Handler implementation
// ---------------------------------------------------------------------------

/// gRPC handler that wraps a `Service<R>` and implements the generated
/// `CommandService` trait. Mirrors the HTTP transport pattern.
pub struct GrpcHandler<R> {
    service: Arc<Service<R>>,
}

impl<R> GrpcHandler<R> {
    pub fn new(service: Arc<Service<R>>) -> Self {
        Self { service }
    }
}

#[tonic::async_trait]
impl<R: Send + Sync + 'static> CommandService for GrpcHandler<R> {
    async fn dispatch(
        &self,
        request: Request<GrpcRequest>,
    ) -> Result<Response<GrpcResponse>, Status> {
        let metadata = request.metadata().clone();
        let req = request.into_inner();

        // Parse JSON input
        let input: serde_json::Value = serde_json::from_str(&req.input).unwrap_or_default();

        // Build session: start with metadata headers, then overlay payload values
        let session = build_session(&metadata, req.session_variables);

        match self.service.dispatch(&req.command, input, session) {
            Ok(value) => Ok(Response::new(GrpcResponse {
                status: 200,
                body: value.to_string(),
            })),
            Err(e) => Ok(Response::new(GrpcResponse {
                status: e.status_code() as u32,
                body: json!({ "error": e.to_string() }).to_string(),
            })),
        }
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let commands: Vec<String> = self
            .service
            .commands()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        Ok(Response::new(HealthResponse {
            ok: true,
            commands,
        }))
    }
}

// ---------------------------------------------------------------------------
// Session building (same pattern as HTTP transport)
// ---------------------------------------------------------------------------

/// Build a session from gRPC metadata and payload session variables.
///
/// 1. Start with gRPC metadata headers (lowercased key → value)
/// 2. Overlay payload `session_variables` (payload takes precedence)
fn build_session(
    metadata: &tonic::metadata::MetadataMap,
    payload_vars: HashMap<String, String>,
) -> Session {
    let mut vars = HashMap::new();

    // Metadata headers
    for kv in metadata.iter() {
        if let tonic::metadata::KeyAndValueRef::Ascii(key, value) = kv {
            if let Ok(v) = value.to_str() {
                vars.insert(key.as_str().to_string(), v.to_string());
            }
        }
    }

    // Payload overrides
    for (k, v) in payload_vars {
        vars.insert(k, v);
    }

    Session::from_map(vars)
}

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

/// Create a `CommandServiceServer` from a shared `Service<R>`.
pub fn grpc_server<R: Send + Sync + 'static>(
    service: Arc<Service<R>>,
) -> CommandServiceServer<GrpcHandler<R>> {
    CommandServiceServer::new(GrpcHandler::new(service))
}

/// Bind and serve the gRPC transport at the given address (e.g. `"[::1]:50051"`).
pub async fn serve_grpc<R: Send + Sync + 'static>(
    service: Arc<Service<R>>,
    addr: &str,
) -> Result<(), tonic::transport::Error> {
    let addr = addr.parse().expect("invalid gRPC address");
    tonic::transport::Server::builder()
        .add_service(grpc_server(service))
        .serve(addr)
        .await
}
