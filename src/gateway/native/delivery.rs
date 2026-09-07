use super::{
    graphql::GraphqlBinding, response, Body, GatewayError, NativeInner, Request, RequestContext,
    Response, StatusCode,
};
use crate::gateway::{delivery::*, DeliveryCapabilities, GraphqlExecutor};
use axum::{
    body::Bytes,
    http::{header, request::Parts},
    response::IntoResponse,
};
use futures_util::StreamExt;
use std::sync::Mutex;
use tokio::sync::OwnedSemaphorePermit;

/// Bounded native coordinator. Snapshot storage is allocated only when this
/// explicit resource is mounted; every lookup first visits authenticated origin
/// validation. Query-flight and live-sharing mounts remain independent.
pub struct NativeDelivery {
    snapshots: Mutex<SnapshotCache>,
    entry_bytes: usize,
}
impl NativeDelivery {
    /// Allocate a bounded origin-validated snapshot cache.
    pub fn snapshots(limits: SnapshotLimits) -> Result<Self, GatewayError> {
        let snapshots =
            SnapshotCache::new(limits).map_err(|_| GatewayError("invalid snapshot limits"))?;
        Ok(Self {
            snapshots: Mutex::new(snapshots),
            entry_bytes: limits.entry_bytes,
        })
    }
    pub(super) fn capabilities(&self) -> DeliveryCapabilities {
        DeliveryCapabilities {
            snapshots: true,
            coalescing: false,
            live_sharing: false,
        }
    }
    /// Invalidate on a known lost feed, rebuild or coordinator reset. Private
    /// lookups still validate at primary even without a pushed invalidation.
    pub fn invalidate_all(&self) {
        if let Ok(mut cache) = self.snapshots.lock() {
            cache.invalidate_all();
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute(
        &self,
        binding: &GraphqlBinding,
        inner: &NativeInner,
        executor: &GraphqlExecutor,
        context: RequestContext,
        parts: Parts,
        value: serde_json::Value,
        permit: OwnedSemaphorePermit,
    ) -> Response {
        let freshness = match value["extensions"].get("gatewayFreshness") {
            Some(value) => match FreshnessContext::parse(value) {
                Ok(value) => Some(value),
                Err(_) => return response(StatusCode::BAD_REQUEST),
            },
            None => None,
        };
        let admission = match validate(binding, inner, executor, &context, &parts, &value).await {
            AdmissionResult::Eligible(admission) => admission,
            AdmissionResult::Bypass => {
                return binding
                    .execute_http(
                        inner,
                        executor,
                        context,
                        request(&parts, value),
                        Some(permit),
                    )
                    .await
            }
            AdmissionResult::Error(error) => return error,
        };
        let ticket = {
            let Ok(mut cache) = self.snapshots.lock() else {
                return response(StatusCode::SERVICE_UNAVAILABLE);
            };
            match cache.lookup(&admission, freshness.as_ref(), super::now()) {
                Ok(Some(hit)) => return cached_response(hit),
                Err(_) => return response(StatusCode::SERVICE_UNAVAILABLE),
                Ok(None) => {}
            }
            match cache.begin_fill(&admission, super::now()) {
                Ok(ticket) => ticket,
                Err(_) => return response(StatusCode::SERVICE_UNAVAILABLE),
            }
        };
        let mut execution = value.clone();
        mark(&mut execution, "snapshot");
        let result = binding
            .execute_http(
                inner,
                executor,
                context.clone(),
                request(&parts, execution),
                None,
            )
            .await;
        let (response_parts, body) = result.into_parts();
        let captured = match tokio::time::timeout(
            inner.options.limits.read_timeout,
            capture(body, self.entry_bytes),
        )
        .await
        {
            Ok(captured) => captured,
            Err(_) => return response(StatusCode::GATEWAY_TIMEOUT),
        };
        let body = match captured {
            Captured::Bytes(body) => body,
            Captured::Streaming(body) => {
                // An oversized/streaming result bypasses cache without truncation.
                let stream = body.into_data_stream().map(move |chunk| {
                    let _ = &permit;
                    chunk
                });
                return Response::from_parts(response_parts, Body::from_stream(stream));
            }
        };
        let headers = response_parts
            .headers
            .iter()
            .map(|(name, value)| {
                value
                    .to_str()
                    .map(|value| (name.to_string(), value.to_owned()))
            })
            .collect::<Result<Vec<_>, _>>();
        let Ok(headers) = headers else {
            return Response::from_parts(response_parts, Body::from(body));
        };
        let snapshot = SnapshotResponse {
            status: response_parts.status.as_u16(),
            headers,
            body: body.to_vec(),
        };
        if !snapshot.satisfies(&admission, freshness.as_ref()) {
            return Response::from_parts(response_parts, Body::from(body));
        }
        // Recheck the actual fill's vector and authorization after result SQL.
        // A delayed fill cannot install behind a newer primary commit, even if
        // the invalidation feed was delayed, dropped or never connected.
        match validate(binding, inner, executor, &context, &parts, &value).await {
            AdmissionResult::Eligible(current) => {
                if let Ok(mut cache) = self.snapshots.lock() {
                    if cache
                        .install(ticket, current, snapshot, super::now())
                        .is_err()
                    {
                        return response(StatusCode::SERVICE_UNAVAILABLE);
                    }
                }
            }
            AdmissionResult::Error(error) => return error,
            AdmissionResult::Bypass => {}
        }
        Response::from_parts(response_parts, Body::from(body))
    }
}

enum AdmissionResult {
    Eligible(OriginAdmission),
    Bypass,
    Error(Response),
}
async fn validate(
    binding: &GraphqlBinding,
    inner: &NativeInner,
    executor: &GraphqlExecutor,
    context: &RequestContext,
    parts: &Parts,
    value: &serde_json::Value,
) -> AdmissionResult {
    let mut validation = value.clone();
    mark(&mut validation, "validate");
    let pending = binding.execute_http(
        inner,
        executor,
        context.clone(),
        request(parts, validation),
        None,
    );
    let result =
        match tokio::time::timeout(inner.options.limits.response_header_timeout, pending).await {
            Ok(result) => result,
            Err(_) => return AdmissionResult::Error(response(StatusCode::GATEWAY_TIMEOUT)),
        };
    let (parts, body) = result.into_parts();
    let body = match tokio::time::timeout(
        inner.options.limits.read_timeout,
        axum::body::to_bytes(body, 65536),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => return AdmissionResult::Error(response(StatusCode::BAD_GATEWAY)),
        Err(_) => return AdmissionResult::Error(response(StatusCode::GATEWAY_TIMEOUT)),
    };
    if parts.status != StatusCode::OK {
        return AdmissionResult::Error(Response::from_parts(parts, Body::from(body)));
    }
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return AdmissionResult::Error(response(StatusCode::BAD_GATEWAY)),
    };
    if parsed
        .get("errors")
        .is_some_and(|e| e.as_array().is_none_or(|e| !e.is_empty()))
    {
        return AdmissionResult::Error(Response::from_parts(parts, Body::from(body)));
    }
    let delivery = &parsed["extensions"]["gatewayDelivery"];
    if delivery["eligible"] != true {
        return AdmissionResult::Bypass;
    }
    // Cookie-bearing validation is never a reusable authorization grant.
    if parts.headers.contains_key(header::SET_COOKIE) {
        return AdmissionResult::Bypass;
    }
    let admission: OriginAdmission = match serde_json::from_value(delivery["admission"].clone()) {
        Ok(value) => value,
        Err(_) => return AdmissionResult::Error(response(StatusCode::BAD_GATEWAY)),
    };
    if admission.bind(value, super::now()).is_err() {
        return AdmissionResult::Error(response(StatusCode::SERVICE_UNAVAILABLE));
    }
    AdmissionResult::Eligible(admission)
}
fn mark(value: &mut serde_json::Value, action: &str) {
    if !value["extensions"].is_object() {
        value["extensions"] = serde_json::json!({});
    }
    value["extensions"]["gatewayDelivery"] = serde_json::json!({"action":action});
}
fn request(parts: &Parts, value: serde_json::Value) -> Request<Body> {
    let mut request = Request::new(Body::from(value.to_string()));
    *request.method_mut() = parts.method.clone();
    *request.uri_mut() = parts.uri.clone();
    *request.version_mut() = parts.version;
    *request.headers_mut() = parts.headers.clone();
    *request.extensions_mut() = parts.extensions.clone();
    request.headers_mut().remove(header::CONTENT_LENGTH);
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    request
}
fn cached_response(snapshot: SnapshotResponse) -> Response {
    let mut result = snapshot.body.into_response();
    *result.status_mut() = StatusCode::from_u16(snapshot.status).expect("validated status");
    result.headers_mut().clear();
    for (name, value) in snapshot.headers {
        if let (Ok(name), Ok(value)) = (
            name.parse::<header::HeaderName>(),
            value.parse::<axum::http::HeaderValue>(),
        ) {
            result.headers_mut().append(name, value);
        }
    }
    result
}
enum Captured {
    Bytes(Bytes),
    Streaming(Body),
}
async fn capture(body: Body, limit: usize) -> Captured {
    let mut stream = body.into_data_stream();
    let mut chunks = Vec::new();
    let mut bytes = 0;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => {
                bytes += chunk.len();
                chunks.push(Ok(chunk));
            }
            Err(error) => {
                chunks.push(Err(error));
                return Captured::Streaming(Body::from_stream(
                    futures_util::stream::iter(chunks).chain(stream),
                ));
            }
        }
        if bytes > limit {
            return Captured::Streaming(Body::from_stream(
                futures_util::stream::iter(chunks).chain(stream),
            ));
        }
    }
    let mut result = Vec::with_capacity(bytes);
    for chunk in chunks {
        result.extend_from_slice(&chunk.expect("successful captured chunks"));
    }
    Captured::Bytes(result.into())
}
