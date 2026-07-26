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
//! use distributed::{microsvc, InMemoryRepository};
//!
//! let service = Arc::new(
//!     microsvc::Service::new().routes(
//!         microsvc::Routes::new()
//!             .with_repo(InMemoryRepository::new().queued().aggregate::<Counter>())
//!             .command("counter.create")
//!             .handle(|ctx| { /* ... */ })
//!     )
//! );
//!
//! // Get the server to compose with other tonic routes
//! let grpc_svc = microsvc::grpc_server(service.clone());
//!
//! // Or serve directly
//! microsvc::serve_grpc(service, "[::1]:50051").await?;
//! ```

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::net::{AddrParseError, SocketAddr};
use std::sync::Arc;

use serde_json::json;
use tonic::{Request, Response, Status};

use super::service::Service;
use super::session::Session;
use crate::bus::MessageKind;
use crate::failure_log::{
    FailureAction, FailureCategory, FailureComponent, FailureMessageFields, FailureOperation,
    FailureRecord, FailureRetryClass,
};

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

/// Error returned when serving the gRPC transport fails.
#[derive(Debug)]
pub enum GrpcServeError {
    /// The supplied bind address could not be parsed as a socket address.
    InvalidAddress {
        /// Original bind address string supplied by the caller.
        addr: String,
        /// Address parser error.
        source: AddrParseError,
    },
    /// The tonic transport server failed while serving.
    Transport(tonic::transport::Error),
}

impl fmt::Display for GrpcServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrpcServeError::InvalidAddress { addr, source } => {
                write!(f, "invalid gRPC bind address `{addr}`: {source}")
            }
            GrpcServeError::Transport(source) => write!(f, "gRPC transport error: {source}"),
        }
    }
}

impl Error for GrpcServeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            GrpcServeError::InvalidAddress { source, .. } => Some(source),
            GrpcServeError::Transport(source) => Some(source),
        }
    }
}

impl From<tonic::transport::Error> for GrpcServeError {
    fn from(source: tonic::transport::Error) -> Self {
        GrpcServeError::Transport(source)
    }
}

// ---------------------------------------------------------------------------
// Handler implementation
// ---------------------------------------------------------------------------

/// gRPC handler that wraps a `Service` and implements the generated
/// `CommandService` trait. Mirrors the HTTP transport pattern.
pub struct GrpcHandler {
    service: Arc<Service>,
}

impl GrpcHandler {
    pub fn new(service: Arc<Service>) -> Self {
        Self { service }
    }
}

#[tonic::async_trait]
impl CommandService for GrpcHandler {
    async fn dispatch(
        &self,
        request: Request<GrpcRequest>,
    ) -> Result<Response<GrpcResponse>, Status> {
        let metadata = request.metadata().clone();
        let req = request.into_inner();

        // Parse JSON input (strict; invalid payloads return 400)
        let input: serde_json::Value = match serde_json::from_str(&req.input) {
            Ok(value) => value,
            Err(e) => {
                FailureRecord::new(
                    FailureComponent::Grpc,
                    FailureOperation::Ingress,
                    FailureCategory::Decode,
                    FailureAction::Return400,
                    FailureRetryClass::Permanent,
                    "serde_json::Error",
                    format!("invalid JSON input: {e}"),
                )
                .with_service(self.service.name())
                .with_message(FailureMessageFields::for_name(
                    MessageKind::Command,
                    &req.command,
                ))
                .with_grpc_status(400)
                .emit();
                return Ok(Response::new(GrpcResponse {
                    status: 400,
                    body: json!({ "error": format!("invalid JSON input: {e}") }).to_string(),
                }));
            }
        };

        // Build session: payload values first, then transport metadata wins.
        // Transport metadata is trusted (injected by a trusted proxy); the
        // payload is client-controlled and MUST NOT override it. See
        // [`build_session`] and the `Session` trust-boundary docs.
        let session = build_session(&metadata, req.session_variables);
        let failure_metadata = session_metadata(&session);

        match self.service.dispatch(&req.command, input, session).await {
            Ok(value) => Ok(Response::new(GrpcResponse {
                status: 200,
                body: value.to_string(),
            })),
            Err(e) => {
                // Mirror the HTTP transport: never leak internal error detail
                // (SQL/driver text) to clients. Server-internal failures are
                // masked; client-fault errors keep their descriptive message.
                let status = e.status_code();
                let message = if matches!(e, super::error::HandlerError::UnknownCommand(_)) {
                    FailureMessageFields::unknown()
                } else {
                    FailureMessageFields::for_name_with_metadata(
                        MessageKind::Command,
                        &req.command,
                        &failure_metadata,
                    )
                };
                FailureRecord::from_handler_error(
                    FailureComponent::Grpc,
                    FailureOperation::Ingress,
                    &e,
                )
                .with_service(self.service.name())
                .with_message(message)
                .with_grpc_status(status)
                .emit();
                Ok(Response::new(GrpcResponse {
                    status: status as u32,
                    body: json!({ "error": e.client_facing_message() }).to_string(),
                }))
            }
        }
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let commands: Vec<String> = self
            .service
            .command_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        Ok(Response::new(HealthResponse { ok: true, commands }))
    }
}

// ---------------------------------------------------------------------------
// Session building (same pattern as HTTP transport)
// ---------------------------------------------------------------------------

/// Build a session from gRPC metadata and payload session variables.
///
/// **Trust boundary (security-critical):** transport metadata is TRUSTED —
/// it is injected by a trusted proxy/gateway that authenticates the caller
/// and strips any client-supplied identity headers. The request payload
/// `session_variables` are CLIENT-CONTROLLED and therefore UNTRUSTED.
///
/// Precedence: **metadata wins.** Payload values are applied first, then
/// metadata overwrites any colliding key. This prevents a client from
/// spoofing identity via the request body when behind a trusted gateway.
/// See the [`Session`] docs for the framework-wide trust model.
///
/// Payload-only keys (not present in metadata) still pass through. That
/// supports gateway action/webhook shapes where verified claims arrive only
/// in the body (e.g. a query-layer action payload) and no transport metadata
/// is injected.
fn build_session(
    metadata: &tonic::metadata::MetadataMap,
    payload_vars: HashMap<String, String>,
) -> Session {
    let mut vars = HashMap::new();

    // Untrusted payload first.
    for (k, v) in payload_vars {
        vars.insert(k, v);
    }

    // Trusted metadata wins — overwrites any payload-supplied key.
    for kv in metadata.iter() {
        if let tonic::metadata::KeyAndValueRef::Ascii(key, value) = kv {
            if let Ok(v) = value.to_str() {
                vars.insert(key.as_str().to_string(), v.to_string());
            }
        }
    }

    Session::from_map(vars)
}

fn session_metadata(session: &Session) -> Vec<(String, String)> {
    session
        .variables()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

/// Create a `CommandServiceServer` from a shared `Service`.
pub fn grpc_server(service: Arc<Service>) -> CommandServiceServer<GrpcHandler> {
    CommandServiceServer::new(GrpcHandler::new(service))
}

/// Bind and serve the gRPC transport at the given address (e.g. `"[::1]:50051"`).
///
/// Returns [`GrpcServeError::InvalidAddress`] when `addr` is not a valid socket
/// address, and [`GrpcServeError::Transport`] for errors returned by tonic while
/// serving.
pub async fn serve_grpc(service: Arc<Service>, addr: &str) -> Result<(), GrpcServeError> {
    let addr: SocketAddr = addr
        .parse()
        .map_err(|source| GrpcServeError::InvalidAddress {
            addr: addr.to_string(),
            source,
        })?;
    tonic::transport::Server::builder()
        .add_service(grpc_server(service))
        .serve(addr)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "failure-logs")]
    #[tokio::test]
    async fn dispatch_masks_5xx_and_emits_sanitized_failure_event() {
        use crate::microsvc::{Context, HandlerError, Routes};
        use crate::RepositoryError;

        let service = Arc::new(
            Service::new().named("orders-grpc").routes(
                Routes::new()
                    .with_dependencies(())
                    .command("orders.create")
                    .handle(|_: &Context<()>| async move {
                        Err(HandlerError::Repository(RepositoryError::Model(
                            "storage failed token=raw-token mysql://user:pass@db/app".into(),
                        )))
                    }),
            ),
        );
        let handler = GrpcHandler::new(service);
        let mut payload_vars = HashMap::new();
        payload_vars.insert("x-hasura-user-id".to_string(), "user-secret".to_string());
        let mut request = Request::new(GrpcRequest {
            command: "orders.create".to_string(),
            input: json!({ "request_body": "payload-secret" }).to_string(),
            session_variables: payload_vars,
        });
        request
            .metadata_mut()
            .insert("authorization", "Bearer raw-auth".parse().unwrap());
        request.metadata_mut().insert(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
                .parse()
                .unwrap(),
        );
        request
            .metadata_mut()
            .insert("tracestate", "vendor=raw-state".parse().unwrap());
        let capture = crate::failure_log::testing::capture_failures();

        let response = handler.dispatch(request).await.unwrap().into_inner();

        assert_eq!(response.status, 500);
        let body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(body, json!({ "error": "Internal server error" }));

        let events = capture.failure_events();
        let grpc = events
            .iter()
            .find(|event| event.field("distributed.component") == Some("grpc"))
            .expect("grpc failure event");
        assert_eq!(grpc.field("distributed.failure.action"), Some("return_500"));
        assert_eq!(grpc.field("rpc.grpc.status_code"), Some("500"));
        assert_eq!(
            grpc.field("trace.trace_id"),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );

        let rendered = format!("{events:?}");
        for forbidden in [
            "payload-secret",
            "raw-token",
            "mysql://user:pass@db/app",
            "user-secret",
            "raw-auth",
            "raw-state",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "failure event leaked `{forbidden}`: {rendered}"
            );
        }
    }
}
