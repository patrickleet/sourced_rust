use super::api::snapshot;
use super::registry::MetricSampleValue;
use super::*;
use crate::bus::MessageKind;
use crate::telemetry::{
    dispatch_status, failure_action, failure_class, metric_labels, metric_names, outbox_outcome,
    transport_outcome,
};
use std::collections::BTreeSet;
use std::time::Duration;

#[test]
fn prometheus_text_escapes_label_values() {
    let _guard = lock_for_tests();
    reset_for_tests();

    record_microsvc_dispatch(
        Some("svc\"one"),
        MessageKind::Command,
        "line\nbreak",
        dispatch_status::SUCCESS,
        Duration::from_millis(2),
    );

    let text = prometheus_text();

    assert!(text.contains(
            "distributed_microsvc_dispatch_total{service=\"svc\\\"one\",message_kind=\"command\",message=\"line\\nbreak\",status=\"success\"} 1"
        ));
}

#[test]
fn histogram_records_cumulative_buckets() {
    let _guard = lock_for_tests();
    reset_for_tests();

    record_microsvc_dispatch(
        Some("orders"),
        MessageKind::Event,
        "orders.created",
        dispatch_status::SUCCESS,
        Duration::from_millis(7),
    );

    let text = prometheus_text();

    assert!(text.contains(
            "distributed_microsvc_dispatch_duration_seconds_bucket{service=\"orders\",message_kind=\"event\",message=\"orders.created\",status=\"success\",le=\"0.005\"} 0"
        ));
    assert!(text.contains(
            "distributed_microsvc_dispatch_duration_seconds_bucket{service=\"orders\",message_kind=\"event\",message=\"orders.created\",status=\"success\",le=\"0.01\"} 1"
        ));
    assert!(text.contains(
            "distributed_microsvc_dispatch_duration_seconds_count{service=\"orders\",message_kind=\"event\",message=\"orders.created\",status=\"success\"} 1"
        ));
}

#[test]
fn snapshot_exposes_bounded_family_shape() {
    let _guard = lock_for_tests();
    reset_for_tests();

    record_microsvc_dispatch(
        Some("orders"),
        MessageKind::Event,
        "orders.created",
        dispatch_status::SUCCESS,
        Duration::from_millis(7),
    );
    record_transport_message(
        Some("orders"),
        "nats",
        MessageKind::Event,
        transport_outcome::ACK,
    );
    record_transport_failure(
        Some("orders"),
        "nats",
        failure_class::RETRYABLE,
        failure_action::NACK,
    );
    record_outbox_messages(Some("orders"), outbox_outcome::PUBLISHED, 2);
    set_outbox_backlog(Some("orders"), 3, Some(Duration::from_secs(4)));

    let snapshot = snapshot();
    let family_names = snapshot
        .families()
        .iter()
        .map(|family| family.family.name)
        .collect::<Vec<_>>();

    assert_eq!(
        family_names,
        vec![
            metric_names::SERVICE_INFO,
            metric_names::MICROSVC_DISPATCH_TOTAL,
            metric_names::MICROSVC_DISPATCH_DURATION_SECONDS,
            metric_names::TRANSPORT_MESSAGES_TOTAL,
            metric_names::TRANSPORT_FAILURES_TOTAL,
            metric_names::OUTBOX_MESSAGES_TOTAL,
            metric_names::OUTBOX_PENDING_MESSAGES,
            metric_names::OUTBOX_OLDEST_PENDING_AGE_SECONDS,
            metric_names::GRAPHQL_REQUEST_TOTAL,
            metric_names::GRAPHQL_REQUEST_DURATION_SECONDS,
        ]
    );

    let dispatch_family = snapshot
        .families()
        .iter()
        .find(|family| family.family.name == metric_names::MICROSVC_DISPATCH_TOTAL)
        .expect("dispatch family is present");
    let dispatch_sample = dispatch_family
        .samples
        .iter()
        .find(|sample| {
            label_value(&sample.labels, metric_labels::MESSAGE) == Some("orders.created")
        })
        .expect("dispatch sample is present");
    assert!(matches!(
        &dispatch_sample.value,
        MetricSampleValue::Counter(1)
    ));

    let duration_family = snapshot
        .families()
        .iter()
        .find(|family| family.family.name == metric_names::MICROSVC_DISPATCH_DURATION_SECONDS)
        .expect("duration family is present");
    assert!(matches!(
        &duration_family.samples[0].value,
        MetricSampleValue::Histogram(_)
    ));
}

#[test]
fn rendered_metrics_use_only_allowed_low_cardinality_labels() {
    let _guard = lock_for_tests();
    reset_for_tests();

    describe_service(Some("orders"));
    record_microsvc_dispatch(
        Some("orders"),
        MessageKind::Command,
        "orders.create",
        dispatch_status::SUCCESS,
        Duration::from_millis(2),
    );
    record_transport_message(
        Some("orders"),
        "rabbitmq",
        MessageKind::Command,
        transport_outcome::ACK,
    );
    record_transport_failure(
        Some("orders"),
        "rabbitmq",
        failure_class::PERMANENT,
        failure_action::DEAD_LETTER,
    );
    record_outbox_messages(Some("orders"), outbox_outcome::RELEASED, 1);
    set_outbox_backlog(Some("orders"), 1, Some(Duration::from_secs(30)));

    let text = prometheus_text();
    let label_names = rendered_label_names(&text);

    for forbidden in crate::telemetry::privacy_policy::FORBIDDEN_METRIC_LABELS {
        assert!(
            !label_names.contains(*forbidden),
            "forbidden label `{forbidden}` appeared in metrics:\n{text}"
        );
    }
    for label in &label_names {
        assert!(
            crate::telemetry::privacy_policy::ALLOWED_METRIC_LABELS.contains(&label.as_str()),
            "unexpected metric label `{label}` appeared in metrics:\n{text}"
        );
    }
}

fn label_value<'a>(labels: &'a [(String, String)], name: &str) -> Option<&'a str> {
    labels
        .iter()
        .find(|(label, _)| label == name)
        .map(|(_, value)| value.as_str())
}

fn rendered_label_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in text.lines().filter(|line| !line.starts_with('#')) {
        let Some(open) = line.find('{') else {
            continue;
        };
        let Some(close_offset) = line[open + 1..].find('}') else {
            continue;
        };
        let labels = &line[open + 1..open + 1 + close_offset];
        let bytes = labels.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let name_start = index;
            while index < bytes.len() && bytes[index] != b'=' {
                index += 1;
            }
            if index >= bytes.len() {
                break;
            }
            names.insert(labels[name_start..index].to_string());
            index += 1;

            if bytes.get(index) == Some(&b'"') {
                index += 1;
            }
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index = (index + 2).min(bytes.len()),
                    b'"' => {
                        index += 1;
                        break;
                    }
                    _ => index += 1,
                }
            }
            if bytes.get(index) == Some(&b',') {
                index += 1;
            }
        }
    }
    names
}
