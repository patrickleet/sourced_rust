//! Structured failure event schema and sanitization.

use std::error::Error;

use crate::bus::{Message, MessageKind, TransportError, TransportErrorKind};
use crate::lock::RetryClass;
use crate::microsvc::HandlerError;
use crate::repository::RepositoryError;
use crate::trace_context::{is_valid_traceparent, TraceContext};

pub(crate) const SCHEMA_VERSION: u64 = 1;

const UNKNOWN: &str = "unknown";
const ID_LIMIT: usize = 128;
const MESSAGE_LIMIT: usize = 128;
const ERROR_LIMIT: usize = 512;
const TYPE_LIMIT: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    dead_code,
    reason = "schema vocabulary includes repository even when current emitters report through their owning boundary"
)]
pub(crate) enum FailureComponent {
    Microsvc,
    Http,
    Grpc,
    Knative,
    Transport,
    Outbox,
    Repository,
}

impl FailureComponent {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Microsvc => "microsvc",
            Self::Http => "http",
            Self::Grpc => "grpc",
            Self::Knative => "knative",
            Self::Transport => "transport",
            Self::Outbox => "outbox",
            Self::Repository => "repository",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    dead_code,
    reason = "schema vocabulary reserves handler and commit operations for framework failure boundaries"
)]
pub(crate) enum FailureOperation {
    Dispatch,
    Handler,
    Ingress,
    Receive,
    Settle,
    Publish,
    Claim,
    Commit,
}

impl FailureOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::Handler => "handler",
            Self::Ingress => "ingress",
            Self::Receive => "receive",
            Self::Settle => "settle",
            Self::Publish => "publish",
            Self::Claim => "claim",
            Self::Commit => "commit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    dead_code,
    reason = "schema vocabulary reserves unknown for future unmapped framework failures"
)]
pub(crate) enum FailureCategory {
    Routing,
    Decode,
    Validation,
    Auth,
    Guard,
    Repository,
    Storage,
    Transport,
    OutboxPublish,
    OutboxSettle,
    Handler,
    Unknown,
}

impl FailureCategory {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Routing => "routing",
            Self::Decode => "decode",
            Self::Validation => "validation",
            Self::Auth => "auth",
            Self::Guard => "guard",
            Self::Repository => "repository",
            Self::Storage => "storage",
            Self::Transport => "transport",
            Self::OutboxPublish => "outbox_publish",
            Self::OutboxSettle => "outbox_settle",
            Self::Handler => "handler",
            Self::Unknown => UNKNOWN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureAction {
    Nack,
    DeadLetter,
    Park,
    LogAndAck,
    Stop,
    Release,
    Fail,
    Return400,
    Return401,
    Return404,
    Return422,
    Return500,
    Return503,
    RecvError,
    SettleAck,
    SettleNack,
    SettleDeadLetter,
    SettlePark,
}

impl FailureAction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Nack => "nack",
            Self::DeadLetter => "dead_letter",
            Self::Park => "park",
            Self::LogAndAck => "log_and_ack",
            Self::Stop => "stop",
            Self::Release => "release",
            Self::Fail => "fail",
            Self::Return400 => "return_400",
            Self::Return401 => "return_401",
            Self::Return404 => "return_404",
            Self::Return422 => "return_422",
            Self::Return500 => "return_500",
            Self::Return503 => "return_503",
            Self::RecvError => "recv_error",
            Self::SettleAck => "settle_ack",
            Self::SettleNack => "settle_nack",
            Self::SettleDeadLetter => "settle_dead_letter",
            Self::SettlePark => "settle_park",
        }
    }

    pub(crate) const fn for_status(status: u16) -> Self {
        match status {
            400 => Self::Return400,
            401 => Self::Return401,
            404 => Self::Return404,
            422 => Self::Return422,
            503 => Self::Return503,
            _ => Self::Return500,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    dead_code,
    reason = "schema vocabulary reserves unknown when a future source cannot classify retryability"
)]
pub(crate) enum FailureRetryClass {
    Retryable,
    Permanent,
    Unknown,
}

impl FailureRetryClass {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::Permanent => "permanent",
            Self::Unknown => UNKNOWN,
        }
    }
}

impl From<TransportErrorKind> for FailureRetryClass {
    fn from(kind: TransportErrorKind) -> Self {
        match kind {
            TransportErrorKind::Retryable => Self::Retryable,
            TransportErrorKind::Permanent => Self::Permanent,
        }
    }
}

impl From<RetryClass> for FailureRetryClass {
    fn from(kind: RetryClass) -> Self {
        match kind {
            RetryClass::Retryable => Self::Retryable,
            RetryClass::Permanent => Self::Permanent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HandlerFailureMapping {
    pub(crate) category: FailureCategory,
    pub(crate) status: u16,
    pub(crate) retry_class: FailureRetryClass,
    pub(crate) action: FailureAction,
    pub(crate) error_type: &'static str,
}

pub(crate) fn handler_failure_mapping(error: &HandlerError) -> HandlerFailureMapping {
    let status = error.status_code();
    let (category, retry_class, error_type) = match error {
        HandlerError::UnknownCommand(_) => (
            FailureCategory::Routing,
            FailureRetryClass::Permanent,
            "HandlerError::UnknownCommand",
        ),
        HandlerError::DecodeFailed(_) => (
            FailureCategory::Decode,
            FailureRetryClass::Permanent,
            "HandlerError::DecodeFailed",
        ),
        HandlerError::Rejected(_) => (
            FailureCategory::Validation,
            FailureRetryClass::Permanent,
            "HandlerError::Rejected",
        ),
        HandlerError::NotFound(_) => (
            FailureCategory::Repository,
            FailureRetryClass::Retryable,
            "HandlerError::NotFound",
        ),
        HandlerError::Unauthorized(_) => (
            FailureCategory::Auth,
            FailureRetryClass::Permanent,
            "HandlerError::Unauthorized",
        ),
        HandlerError::Repository(err) => (
            repository_failure_category(err),
            FailureRetryClass::from(err.kind()),
            repository_error_type(err),
        ),
        HandlerError::GuardRejected(_) => (
            FailureCategory::Guard,
            FailureRetryClass::Permanent,
            "HandlerError::GuardRejected",
        ),
        HandlerError::Other(_) => (
            FailureCategory::Handler,
            FailureRetryClass::Retryable,
            "HandlerError::Other",
        ),
    };

    HandlerFailureMapping {
        category,
        status,
        retry_class,
        action: FailureAction::for_status(status),
        error_type,
    }
}

pub(crate) fn repository_failure_category(error: &RepositoryError) -> FailureCategory {
    match error {
        RepositoryError::LockPoisoned(_)
        | RepositoryError::Lock(_)
        | RepositoryError::Storage { .. } => FailureCategory::Storage,
        RepositoryError::ConcurrentWrite { .. }
        | RepositoryError::DuplicateStreamInBatch { .. }
        | RepositoryError::DuplicateOutboxMessageInBatch { .. }
        | RepositoryError::DuplicateInboxReceipt { .. }
        | RepositoryError::InvalidInboxReceipt { .. }
        | RepositoryError::InvalidStreamIdentity { .. }
        | RepositoryError::NotFound { .. }
        | RepositoryError::InvalidState { .. }
        | RepositoryError::Replay(_)
        | RepositoryError::Model(_) => FailureCategory::Repository,
    }
}

pub(crate) fn repository_error_type(error: &RepositoryError) -> &'static str {
    match error {
        RepositoryError::LockPoisoned(_) => "RepositoryError::LockPoisoned",
        RepositoryError::Lock(_) => "RepositoryError::Lock",
        RepositoryError::ConcurrentWrite { .. } => "RepositoryError::ConcurrentWrite",
        RepositoryError::DuplicateStreamInBatch { .. } => "RepositoryError::DuplicateStreamInBatch",
        RepositoryError::DuplicateOutboxMessageInBatch { .. } => {
            "RepositoryError::DuplicateOutboxMessageInBatch"
        }
        RepositoryError::DuplicateInboxReceipt { .. } => "RepositoryError::DuplicateInboxReceipt",
        RepositoryError::InvalidInboxReceipt { .. } => "RepositoryError::InvalidInboxReceipt",
        RepositoryError::InvalidStreamIdentity { .. } => "RepositoryError::InvalidStreamIdentity",
        RepositoryError::NotFound { .. } => "RepositoryError::NotFound",
        RepositoryError::InvalidState { .. } => "RepositoryError::InvalidState",
        RepositoryError::Replay(_) => "RepositoryError::Replay",
        RepositoryError::Model(_) => "RepositoryError::Model",
        RepositoryError::Storage { retryable, .. } => {
            if *retryable {
                "RepositoryError::StorageRetryable"
            } else {
                "RepositoryError::StoragePermanent"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FailureMessageFields {
    pub(crate) kind: Option<MessageKind>,
    pub(crate) name: String,
    pub(crate) id_hash: String,
    pub(crate) correlation_id: String,
    pub(crate) causation_id: String,
    pub(crate) trace: FailureTraceFields,
    pub(crate) payload_size_bytes: Option<usize>,
    pub(crate) payload_content_type: String,
    pub(crate) payload_codec: String,
}

impl FailureMessageFields {
    pub(crate) fn unknown() -> Self {
        Self {
            name: UNKNOWN.to_string(),
            ..Self::default()
        }
    }

    #[cfg(any(feature = "http", feature = "grpc"))]
    pub(crate) fn for_name(kind: MessageKind, name: &str) -> Self {
        Self {
            kind: Some(kind),
            name: cap(name, MESSAGE_LIMIT),
            ..Self::default()
        }
    }

    #[cfg(any(feature = "http", feature = "grpc"))]
    pub(crate) fn for_name_with_metadata(
        kind: MessageKind,
        name: &str,
        metadata: &[(String, String)],
    ) -> Self {
        let mut fields = Self::for_name(kind, name);
        fields.correlation_id = metadata_value(metadata, crate::trace_context::CORRELATION_ID)
            .map(cap_id)
            .unwrap_or_default();
        fields.causation_id = metadata_value(metadata, crate::trace_context::CAUSATION_ID)
            .map(cap_id)
            .unwrap_or_default();
        fields.trace = trace_fields_from_metadata(metadata);
        fields
    }

    pub(crate) fn from_message(message: &Message) -> Self {
        let trace = trace_fields_from_metadata(&message.metadata);
        Self {
            kind: Some(message.kind),
            name: cap(message.name(), MESSAGE_LIMIT),
            id_hash: message.id().map(hash_identifier).unwrap_or_default(),
            correlation_id: message.correlation_id().map(cap_id).unwrap_or_default(),
            causation_id: message.causation_id().map(cap_id).unwrap_or_default(),
            trace,
            payload_size_bytes: Some(message.payload().len()),
            payload_content_type: sanitize_label(&message.content_type),
            payload_codec: message
                .metadata("x-sourced-payload-codec")
                .map(sanitize_label)
                .unwrap_or_default(),
        }
    }
}

#[cfg(any(feature = "http", feature = "grpc"))]
fn metadata_value<'a>(metadata: &'a [(String, String)], key: &str) -> Option<&'a str> {
    metadata
        .iter()
        .find(|(existing, _)| existing.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct FailureTraceFields {
    pub(crate) trace_id: String,
    pub(crate) parent_span_id: String,
    pub(crate) trace_flags: String,
    pub(crate) traceparent_valid: bool,
    pub(crate) tracestate_present: bool,
}

fn trace_fields_from_metadata(metadata: &[(String, String)]) -> FailureTraceFields {
    let context = TraceContext::from_metadata(metadata.iter().map(|(key, value)| (key, value)));
    let tracestate_present = context.tracestate.is_some();
    let Some(traceparent) = context.traceparent.as_deref() else {
        return FailureTraceFields {
            tracestate_present,
            ..FailureTraceFields::default()
        };
    };
    if !is_valid_traceparent(traceparent) {
        return FailureTraceFields {
            traceparent_valid: false,
            tracestate_present,
            ..FailureTraceFields::default()
        };
    }

    let mut parts = traceparent.split('-');
    let _version = parts.next();
    let trace_id = parts.next().unwrap_or_default().to_string();
    let parent_span_id = parts.next().unwrap_or_default().to_string();
    let trace_flags = parts.next().unwrap_or_default().to_string();

    FailureTraceFields {
        trace_id,
        parent_span_id,
        trace_flags,
        traceparent_valid: true,
        tracestate_present,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailureRecord {
    pub(crate) service_name: String,
    pub(crate) component: FailureComponent,
    pub(crate) operation: FailureOperation,
    pub(crate) category: FailureCategory,
    pub(crate) action: FailureAction,
    pub(crate) retry_class: FailureRetryClass,
    pub(crate) message: FailureMessageFields,
    pub(crate) error_type: &'static str,
    pub(crate) error_message: String,
    pub(crate) http_status_code: Option<u16>,
    pub(crate) grpc_status_code: Option<u16>,
    pub(crate) transport: String,
    pub(crate) outbox_attempt: Option<u32>,
    pub(crate) outbox_max_attempts: Option<u32>,
    pub(crate) outbox_status: String,
    pub(crate) outbox_source_aggregate_type: String,
    pub(crate) outbox_source_sequence: String,
}

impl FailureRecord {
    pub(crate) fn new(
        component: FailureComponent,
        operation: FailureOperation,
        category: FailureCategory,
        action: FailureAction,
        retry_class: FailureRetryClass,
        error_type: &'static str,
        error_message: impl AsRef<str>,
    ) -> Self {
        Self {
            service_name: String::new(),
            component,
            operation,
            category,
            action,
            retry_class,
            message: FailureMessageFields::unknown(),
            error_type,
            error_message: sanitize_error_message(error_message.as_ref()),
            http_status_code: None,
            grpc_status_code: None,
            transport: String::new(),
            outbox_attempt: None,
            outbox_max_attempts: None,
            outbox_status: String::new(),
            outbox_source_aggregate_type: String::new(),
            outbox_source_sequence: String::new(),
        }
    }

    pub(crate) fn from_handler_error(
        component: FailureComponent,
        operation: FailureOperation,
        error: &HandlerError,
    ) -> Self {
        let mapping = handler_failure_mapping(error);
        let error_message = handler_error_summary(error);
        Self::new(
            component,
            operation,
            mapping.category,
            mapping.action,
            mapping.retry_class,
            mapping.error_type,
            error_message,
        )
        .with_http_status(mapping.status)
    }

    pub(crate) fn from_transport_error(
        component: FailureComponent,
        operation: FailureOperation,
        action: FailureAction,
        error: &TransportError,
    ) -> Self {
        let (category, error_type) = classify_transport_error(error);
        Self::new(
            component,
            operation,
            category,
            action,
            FailureRetryClass::from(error.kind()),
            error_type,
            error.message(),
        )
    }

    pub(crate) fn from_repository_error(
        component: FailureComponent,
        operation: FailureOperation,
        action: FailureAction,
        error: &RepositoryError,
    ) -> Self {
        Self::new(
            component,
            operation,
            repository_failure_category(error),
            action,
            FailureRetryClass::from(error.kind()),
            repository_error_type(error),
            error.to_string(),
        )
    }

    pub(crate) fn with_service(mut self, service_name: Option<&str>) -> Self {
        self.service_name = service_name
            .map(|name| cap(name, TYPE_LIMIT))
            .unwrap_or_default();
        self
    }

    pub(crate) fn with_message(mut self, message: FailureMessageFields) -> Self {
        self.message = message;
        self
    }

    pub(crate) fn with_http_status(mut self, status: u16) -> Self {
        self.http_status_code = Some(status);
        self
    }

    #[cfg(feature = "http")]
    pub(crate) fn with_action(mut self, action: FailureAction) -> Self {
        self.action = action;
        self
    }

    pub(crate) fn with_category(mut self, category: FailureCategory) -> Self {
        self.category = category;
        self
    }

    #[cfg(feature = "grpc")]
    pub(crate) fn with_grpc_status(mut self, status: u16) -> Self {
        self.grpc_status_code = Some(status);
        self
    }

    pub(crate) fn with_transport(mut self, transport: &str) -> Self {
        self.transport = cap(transport, TYPE_LIMIT);
        self
    }

    pub(crate) fn with_outbox_fields(
        mut self,
        attempt: u32,
        max_attempts: u32,
        status: &'static str,
        source_aggregate_type: String,
        source_sequence: String,
    ) -> Self {
        self.outbox_attempt = Some(attempt);
        self.outbox_max_attempts = Some(max_attempts);
        self.outbox_status = status.to_string();
        self.outbox_source_aggregate_type = sanitize_label(&source_aggregate_type);
        self.outbox_source_sequence = sanitize_label(&source_sequence);
        self
    }

    pub(crate) fn emit(&self) {
        emit_failure(self);
    }
}

fn classify_transport_error(error: &TransportError) -> (FailureCategory, &'static str) {
    if let Some(source) = error.source() {
        if let Some(handler) = source.downcast_ref::<HandlerError>() {
            let mapping = handler_failure_mapping(handler);
            return (mapping.category, mapping.error_type);
        }
        if let Some(repository) = source.downcast_ref::<RepositoryError>() {
            return (
                repository_failure_category(repository),
                repository_error_type(repository),
            );
        }
    }

    let error_type = match error.kind() {
        TransportErrorKind::Retryable => "TransportError::Retryable",
        TransportErrorKind::Permanent => "TransportError::Permanent",
    };
    (FailureCategory::Transport, error_type)
}

fn handler_error_summary(error: &HandlerError) -> String {
    match error {
        HandlerError::UnknownCommand(_) => "unknown command".to_string(),
        HandlerError::DecodeFailed(message) => format!("decode failed: {message}"),
        HandlerError::Rejected(message) => format!("rejected: {message}"),
        HandlerError::NotFound(_) => "not found".to_string(),
        HandlerError::Unauthorized(_) => "unauthorized".to_string(),
        HandlerError::Repository(error) => error.to_string(),
        HandlerError::GuardRejected(_) => "guard rejected command".to_string(),
        HandlerError::Other(error) => error.to_string(),
    }
}

pub(crate) fn sanitize_error_message(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if looks_like_sql(&lower) {
        return "sql statement redacted".to_string();
    }

    let mut redacted = Vec::new();
    let mut redact_next = false;
    for token in value.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        let sensitive = redact_next || is_sensitive_token(&lower);
        if sensitive {
            redacted.push("[redacted]".to_string());
            redact_next = redacts_following_value(&lower);
        } else if lower.contains("://") {
            redacted.push("[redacted-url]".to_string());
            redact_next = false;
        } else {
            redacted.push(token.to_string());
            redact_next = false;
        }
    }

    cap(&redacted.join(" "), ERROR_LIMIT)
}

fn looks_like_sql(lower: &str) -> bool {
    lower.contains("select ")
        || lower.contains("insert ")
        || lower.contains("update ")
        || lower.contains("delete ")
        || lower.contains(" where ")
        || lower.contains(" from ")
}

fn is_sensitive_token(lower: &str) -> bool {
    lower.contains("authorization")
        || lower == "bearer"
        || lower.contains("cookie")
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("passwd")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("access_key")
        || lower.contains("session_variables")
        || lower.contains("session:")
        || lower.contains("x-hasura-")
        || lower.contains("://")
}

fn redacts_following_value(lower: &str) -> bool {
    lower.ends_with(':')
        || lower.ends_with('=')
        || matches!(
            lower,
            "authorization"
                | "bearer"
                | "cookie"
                | "token"
                | "secret"
                | "password"
                | "passwd"
                | "api_key"
                | "apikey"
                | "access_key"
                | "session"
                | "session_variables"
        )
}

fn cap_id(value: &str) -> String {
    cap(value, ID_LIMIT)
}

fn sanitize_label(value: &str) -> String {
    cap(&sanitize_error_message(value), TYPE_LIMIT)
}

fn cap(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn hash_identifier(value: &str) -> String {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001B3;

    let mut hash = OFFSET;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(feature = "failure-logs")]
fn emit_failure(record: &FailureRecord) {
    let message_kind = record.message.kind.map(|kind| kind.as_str()).unwrap_or("");
    let payload_size = record.message.payload_size_bytes.unwrap_or_default();
    let http_status = record.http_status_code.unwrap_or_default();
    let grpc_status = record.grpc_status_code.unwrap_or_default();
    let outbox_attempt = record.outbox_attempt.unwrap_or_default();
    let outbox_max_attempts = record.outbox_max_attempts.unwrap_or_default();

    tracing::event!(
        target: "distributed.failure",
        tracing::Level::ERROR,
        event.name = "distributed.failure",
        distributed.failure.schema_version = SCHEMA_VERSION,
        distributed.service.name = %record.service_name,
        distributed.component = record.component.as_str(),
        distributed.operation = record.operation.as_str(),
        distributed.message.kind = message_kind,
        distributed.message.name = %record.message.name,
        messaging.message.id_hash = %record.message.id_hash,
        distributed.correlation_id = %record.message.correlation_id,
        distributed.causation_id = %record.message.causation_id,
        trace.trace_id = %record.message.trace.trace_id,
        trace.parent_span_id = %record.message.trace.parent_span_id,
        trace.trace_flags = %record.message.trace.trace_flags,
        trace.traceparent_valid = record.message.trace.traceparent_valid,
        trace.tracestate_present = record.message.trace.tracestate_present,
        error.type = record.error_type,
        error.category = record.category.as_str(),
        error.retry_class = record.retry_class.as_str(),
        error.message = %record.error_message,
        distributed.failure.action = record.action.as_str(),
        http.response.status_code = http_status,
        rpc.grpc.status_code = grpc_status,
        messaging.system = %record.transport,
        outbox.attempt = outbox_attempt,
        outbox.max_attempts = outbox_max_attempts,
        outbox.status = %record.outbox_status,
        outbox.source_aggregate_type = %record.outbox_source_aggregate_type,
        outbox.source_sequence = %record.outbox_source_sequence,
        payload.size_bytes = payload_size,
        payload.content_type = %record.message.payload_content_type,
        payload.codec = %record.message.payload_codec,
    );
}

#[cfg(not(feature = "failure-logs"))]
fn emit_failure(record: &FailureRecord) {
    let _ = (
        SCHEMA_VERSION,
        record.component.as_str(),
        record.operation.as_str(),
        record.category.as_str(),
        record.retry_class.as_str(),
    );
}

#[cfg(all(test, feature = "failure-logs"))]
pub(crate) mod testing {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

    use tracing::field::{Field, Visit};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{Layer, Registry};

    static EVENTS: OnceLock<Arc<Mutex<Vec<CapturedEvent>>>> = OnceLock::new();
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub(crate) struct CapturedEvent {
        pub(crate) target: String,
        pub(crate) fields: BTreeMap<String, String>,
    }

    impl CapturedEvent {
        pub(crate) fn field(&self, name: &str) -> Option<&str> {
            self.fields.get(name).map(String::as_str)
        }
    }

    pub(crate) struct CaptureGuard {
        _guard: MutexGuard<'static, ()>,
        events: Arc<Mutex<Vec<CapturedEvent>>>,
    }

    impl CaptureGuard {
        pub(crate) fn events(&self) -> Vec<CapturedEvent> {
            self.events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        pub(crate) fn failure_events(&self) -> Vec<CapturedEvent> {
            self.events()
                .into_iter()
                .filter(|event| event.target == "distributed.failure")
                .collect()
        }
    }

    pub(crate) fn capture_failures() -> CaptureGuard {
        let events = EVENTS
            .get_or_init(|| {
                let events = Arc::new(Mutex::new(Vec::new()));
                let subscriber = Registry::default().with(FailureCaptureLayer {
                    events: events.clone(),
                });
                let _ = tracing::subscriber::set_global_default(subscriber);
                events
            })
            .clone();

        let guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        CaptureGuard {
            _guard: guard,
            events,
        }
    }

    struct FailureCaptureLayer {
        events: Arc<Mutex<Vec<CapturedEvent>>>,
    }

    impl<S> Layer<S> for FailureCaptureLayer
    where
        S: Subscriber,
    {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            if event.metadata().target() != "distributed.failure" {
                return;
            }
            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            self.events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(CapturedEvent {
                    target: event.metadata().target().to_string(),
                    fields: visitor.fields,
                });
        }
    }

    #[derive(Default)]
    struct FieldVisitor {
        fields: BTreeMap<String, String>,
    }

    impl Visit for FieldVisitor {
        fn record_bool(&mut self, field: &Field, value: bool) {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }

        fn record_i64(&mut self, field: &Field, value: i64) {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.fields
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn handler_error_mapping_covers_all_variants() {
        let cases = [
            (
                HandlerError::UnknownCommand("raw-path".into()),
                FailureCategory::Routing,
                404,
                FailureRetryClass::Permanent,
                FailureAction::Return404,
            ),
            (
                HandlerError::DecodeFailed("bad json".into()),
                FailureCategory::Decode,
                400,
                FailureRetryClass::Permanent,
                FailureAction::Return400,
            ),
            (
                HandlerError::Rejected("invalid".into()),
                FailureCategory::Validation,
                422,
                FailureRetryClass::Permanent,
                FailureAction::Return422,
            ),
            (
                HandlerError::NotFound("aggregate-1".into()),
                FailureCategory::Repository,
                404,
                FailureRetryClass::Retryable,
                FailureAction::Return404,
            ),
            (
                HandlerError::Unauthorized("missing user".into()),
                FailureCategory::Auth,
                401,
                FailureRetryClass::Permanent,
                FailureAction::Return401,
            ),
            (
                HandlerError::Repository(RepositoryError::Model("bad row".into())),
                FailureCategory::Repository,
                500,
                FailureRetryClass::Permanent,
                FailureAction::Return500,
            ),
            (
                HandlerError::GuardRejected("order.create".into()),
                FailureCategory::Guard,
                400,
                FailureRetryClass::Permanent,
                FailureAction::Return400,
            ),
            (
                HandlerError::Other(Box::<dyn Error + Send + Sync>::from("io failed")),
                FailureCategory::Handler,
                500,
                FailureRetryClass::Retryable,
                FailureAction::Return500,
            ),
        ];

        for (error, category, status, retry_class, action) in cases {
            let mapping = handler_failure_mapping(&error);
            assert_eq!(
                (
                    mapping.category,
                    mapping.status,
                    mapping.retry_class,
                    mapping.action
                ),
                (category, status, retry_class, action),
                "{error}"
            );
        }
    }

    #[test]
    fn repository_and_transport_retry_classification_is_reused() {
        let retryable_repo = RepositoryError::retryable_storage(
            "load stream",
            std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"),
        );
        let permanent_repo = RepositoryError::permanent_storage(
            "insert row",
            std::io::Error::new(std::io::ErrorKind::InvalidData, "constraint"),
        );
        assert_eq!(
            FailureRetryClass::from(retryable_repo.kind()),
            FailureRetryClass::Retryable
        );
        assert_eq!(
            FailureRetryClass::from(permanent_repo.kind()),
            FailureRetryClass::Permanent
        );

        let retryable = TransportError::retryable("broker timed out");
        let permanent = TransportError::permanent("decode failed");
        assert_eq!(
            FailureRecord::from_transport_error(
                FailureComponent::Transport,
                FailureOperation::Receive,
                FailureAction::RecvError,
                &retryable,
            )
            .retry_class,
            FailureRetryClass::Retryable
        );
        assert_eq!(
            FailureRecord::from_transport_error(
                FailureComponent::Transport,
                FailureOperation::Receive,
                FailureAction::RecvError,
                &permanent,
            )
            .retry_class,
            FailureRetryClass::Permanent
        );
    }

    #[test]
    fn sanitizer_removes_sensitive_values_and_db_urls() {
        let raw = "authorization: Bearer abc123 cookie: session=xyz password hunter2 token tok secret=s postgres://user:pass@db/app";

        let sanitized = sanitize_error_message(raw);

        for forbidden in [
            "abc123",
            "session=xyz",
            "hunter2",
            "tok",
            "secret=s",
            "postgres://user:pass@db/app",
        ] {
            assert!(
                !sanitized.contains(forbidden),
                "sanitized message leaked `{forbidden}`: {sanitized}"
            );
        }
    }

    #[test]
    fn sanitizer_removes_raw_sql() {
        let sanitized =
            sanitize_error_message("SELECT * FROM users WHERE password = 'raw-password'");

        assert_eq!(sanitized, "sql statement redacted");
    }

    #[test]
    fn record_excludes_payload_metadata_session_values_and_raw_tracestate() {
        let mut message = Message::new(
            "orders.created",
            MessageKind::Event,
            br#"{"request_body":"payload-secret"}"#.to_vec(),
        )
        .with_id("tenant-user-message-id")
        .with_metadata("authorization", "Bearer raw-auth")
        .with_metadata("cookie", "raw-cookie")
        .with_metadata("x-hasura-user-id", "user-42")
        .with_metadata("arbitrary", "metadata-secret")
        .with_metadata("x-sourced-payload-codec", "metadata-secret")
        .with_metadata("correlation_id", "corr-1")
        .with_metadata("causation_id", "cause-1")
        .with_metadata("traceparent", TRACEPARENT)
        .with_metadata("tracestate", "vendor=raw-state");
        message.content_type = "application/json; password=hunter2".to_string();
        let error = HandlerError::Repository(RepositoryError::Model(
            "failed with token=raw-token password=raw-password".into(),
        ));

        let record = FailureRecord::from_handler_error(
            FailureComponent::Microsvc,
            FailureOperation::Dispatch,
            &error,
        )
        .with_message(FailureMessageFields::from_message(&message));
        let rendered = format!("{record:?}");

        for forbidden in [
            "payload-secret",
            "raw-auth",
            "raw-cookie",
            "user-42",
            "metadata-secret",
            "raw-state",
            "raw-token",
            "raw-password",
            "hunter2",
            "tenant-user-message-id",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "failure record leaked `{forbidden}`: {rendered}"
            );
        }
        assert!(rendered.contains("corr-1"));
        assert!(rendered.contains("cause-1"));
        assert!(record.message.trace.tracestate_present);
        assert_eq!(
            record.message.trace.trace_id,
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
    }
}
