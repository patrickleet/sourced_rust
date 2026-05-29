//! Knative / CloudEvents HTTP ingress.
//!
//! Knative is *endpoint-driven*: the platform invokes an HTTP route, so this is
//! a separate ingress shape from the pull-based [`AsyncMessageSource`]. It is
//! NOT modeled as a polling source. The route parses a CloudEvent (binary or
//! structured HTTP mode), maps it to the canonical [`Message`], and calls the
//! same [`Service::dispatch_message`] boundary the direct-transport runner uses.
//!
//! Acknowledgement is the HTTP response:
//!
//! - handler success → `200 OK` (the only ack);
//! - retryable failure → `503` so Knative redelivers per its Delivery config;
//! - permanent failure → `422` so Knative stops retrying / dead-letters per its
//!   Delivery config.
//!
//! Retry, backoff, and dead-lettering are **platform-managed** by Knative
//! Eventing here — unlike direct transports, where this crate's
//! [`FailurePolicy`](super::FailurePolicy) owns them.
//!
//! Requires the `http` feature.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{json, Value};

use super::TransportError;
use crate::microsvc::{Message, MessageKind, Service, SubscriptionPlan};

const STRUCTURED_CONTENT_TYPE: &str = "application/cloudevents+json";

/// Build an axum router exposing a CloudEvents ingress at `POST /` and the
/// per-type route `POST /cloudevent/:type`.
///
/// Both dispatch by the parsed CloudEvent `type` (the path segment is for routing
/// alignment only), so a Knative Trigger can target either a single shared `ref`
/// (`/`) or the per-type subscriber URI [`KnativeBus`](super::KnativeBus) emits
/// (`/cloudevent/<type>`). Compose it with other routes or serve it directly.
pub fn cloud_events_router<D: Send + Sync + 'static>(service: Arc<Service<D>>) -> Router {
    Router::new()
        .route("/", axum::routing::post(ingress_handler))
        .route("/cloudevent/:type", axum::routing::post(ingress_handler))
        .with_state(service)
}

async fn ingress_handler<D: Send + Sync + 'static>(
    State(service): State<Arc<Service<D>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let message = match parse_cloud_event(&headers, &body) {
        Ok(message) => message,
        Err(reason) => return (StatusCode::BAD_REQUEST, reason).into_response(),
    };

    match service.dispatch_message(&message) {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => {
            // Map our retryable/permanent classification onto HTTP so Knative's
            // platform-managed retry/DLQ does the right thing.
            let status = if TransportError::classify_handler_error(&err).is_retryable() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
            (status, Json(json!({ "error": err.to_string() }))).into_response()
        }
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Parse a CloudEvent in binary or structured HTTP mode into a [`Message`].
fn parse_cloud_event(headers: &HeaderMap, body: &Bytes) -> Result<Message, String> {
    let content_type = header(headers, "content-type").unwrap_or("");
    if content_type.starts_with(STRUCTURED_CONTENT_TYPE) {
        parse_structured(body)
    } else {
        parse_binary(headers, body)
    }
}

/// Binary mode: attributes are `ce-*` headers, the body is the data.
fn parse_binary(headers: &HeaderMap, body: &Bytes) -> Result<Message, String> {
    let id = header(headers, "ce-id").ok_or("missing ce-id header")?;
    let name = header(headers, "ce-type").ok_or("missing ce-type header")?;
    let content_type = header(headers, "content-type")
        .unwrap_or("application/json")
        .to_string();

    let mut metadata = Vec::new();
    for (key, value) in headers.iter() {
        let key = key.as_str();
        if let Some(attr) = key.strip_prefix("ce-") {
            if attr == "id" || attr == "type" {
                continue;
            }
            if let Ok(value) = value.to_str() {
                metadata.push((attr.to_string(), value.to_string()));
            }
        }
    }

    Ok(Message {
        id: Some(id.to_string()),
        name: name.to_string(),
        kind: MessageKind::Event,
        payload: body.to_vec(),
        content_type,
        metadata,
    })
}

/// Structured mode: a single `application/cloudevents+json` document.
fn parse_structured(body: &Bytes) -> Result<Message, String> {
    let event: Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid cloudevents+json: {e}"))?;
    let object = event
        .as_object()
        .ok_or("cloudevent must be a JSON object")?;

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or("missing cloudevent id")?
        .to_string();
    let name = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or("missing cloudevent type")?
        .to_string();
    let content_type = object
        .get("datacontenttype")
        .and_then(Value::as_str)
        .unwrap_or("application/json")
        .to_string();

    // Data: `data` (any JSON; objects/arrays re-serialized to bytes) or
    // `data_base64` (binary).
    let payload = if let Some(data) = object.get("data") {
        match data {
            Value::String(s) => s.clone().into_bytes(),
            other => serde_json::to_vec(other).map_err(|e| format!("invalid data: {e}"))?,
        }
    } else if let Some(Value::String(b64)) = object.get("data_base64") {
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("invalid data_base64: {e}"))?
    } else {
        Vec::new()
    };

    // Remaining attributes (source, subject, extensions) become metadata.
    let reserved = [
        "specversion",
        "id",
        "type",
        "datacontenttype",
        "data",
        "data_base64",
    ];
    let mut metadata = Vec::new();
    for (key, value) in object {
        if reserved.contains(&key.as_str()) {
            continue;
        }
        let value = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        metadata.push((key.clone(), value));
    }

    Ok(Message {
        id: Some(id),
        name,
        kind: MessageKind::Event,
        payload,
        content_type,
        metadata,
    })
}

/// Render Knative `Trigger` YAML for each event a service subscribes to, derived
/// from its [`SubscriptionPlan`]. Each Trigger filters on the CloudEvent `type`
/// and routes to `subscriber_service` on `broker`.
pub fn knative_triggers(plan: &SubscriptionPlan, broker: &str, subscriber_service: &str) -> String {
    let mut out = String::new();
    for event in &plan.events {
        let trigger_name = format!("{subscriber_service}-{}", event.replace('.', "-"));
        out.push_str(&format!(
            "apiVersion: eventing.knative.dev/v1\n\
             kind: Trigger\n\
             metadata:\n\
             \x20 name: {trigger_name}\n\
             spec:\n\
             \x20 broker: {broker}\n\
             \x20 filter:\n\
             \x20   attributes:\n\
             \x20     type: {event}\n\
             \x20 subscriber:\n\
             \x20   ref:\n\
             \x20     apiVersion: serving.knative.dev/v1\n\
             \x20     kind: Service\n\
             \x20     name: {subscriber_service}\n\
             ---\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        map
    }

    #[test]
    fn parses_binary_cloud_event() {
        let h = headers(&[
            ("ce-id", "evt-1"),
            ("ce-type", "order.created"),
            ("ce-source", "/orders"),
            ("content-type", "application/json"),
        ]);
        let body = Bytes::from_static(br#"{"order":"o1"}"#);
        let message = parse_cloud_event(&h, &body).unwrap();
        assert_eq!(message.id(), Some("evt-1"));
        assert_eq!(message.name(), "order.created");
        assert_eq!(message.payload(), br#"{"order":"o1"}"#);
        assert_eq!(message.metadata("source"), Some("/orders"));
    }

    #[test]
    fn parses_structured_cloud_event() {
        let h = headers(&[("content-type", "application/cloudevents+json")]);
        let body = Bytes::from(
            json!({
                "specversion": "1.0",
                "id": "evt-2",
                "type": "order.created",
                "source": "/orders",
                "datacontenttype": "application/json",
                "data": {"order": "o2"},
            })
            .to_string(),
        );
        let message = parse_cloud_event(&h, &body).unwrap();
        assert_eq!(message.id(), Some("evt-2"));
        assert_eq!(message.name(), "order.created");
        assert_eq!(message.payload(), br#"{"order":"o2"}"#);
        assert_eq!(message.metadata("source"), Some("/orders"));
    }

    #[test]
    fn missing_id_is_rejected() {
        let h = headers(&[("ce-type", "order.created")]);
        assert!(parse_cloud_event(&h, &Bytes::new()).is_err());
    }

    #[test]
    fn triggers_render_from_subscription_plan() {
        let plan = SubscriptionPlan {
            commands: vec![],
            events: vec!["seat.reserved".to_string()],
        };
        let yaml = knative_triggers(&plan, "default", "checkout-projection");
        assert!(yaml.contains("kind: Trigger"));
        assert!(yaml.contains("type: seat.reserved"));
        assert!(yaml.contains("name: checkout-projection-seat-reserved"));
        assert!(yaml.contains("broker: default"));
    }
}
