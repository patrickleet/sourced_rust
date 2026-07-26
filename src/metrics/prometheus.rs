use crate::telemetry::metric_labels;

use super::registry::{MetricFamily, MetricSampleValue, MetricsSnapshot};

pub(super) fn render_prometheus(snapshot: &MetricsSnapshot) -> String {
    let mut output = String::new();
    for family_snapshot in snapshot.families() {
        write_family_header(&mut output, family_snapshot.family);
        for sample in &family_snapshot.samples {
            match &sample.value {
                MetricSampleValue::Counter(value) => {
                    push_metric(
                        &mut output,
                        family_snapshot.family.name,
                        &sample.labels,
                        &value.to_string(),
                    );
                }
                MetricSampleValue::Gauge(value) => {
                    push_metric(
                        &mut output,
                        family_snapshot.family.name,
                        &sample.labels,
                        &format_float(*value),
                    );
                }
                MetricSampleValue::Histogram(histogram) => {
                    let bucket_name = format!("{}_bucket", family_snapshot.family.name);
                    for bucket in &histogram.buckets {
                        let mut labels = sample.labels.clone();
                        labels.push((
                            metric_labels::LE.to_string(),
                            bucket.upper_bound.to_string(),
                        ));
                        push_metric(
                            &mut output,
                            &bucket_name,
                            &labels,
                            &bucket.count.to_string(),
                        );
                    }
                    let mut labels = sample.labels.clone();
                    labels.push((metric_labels::LE.to_string(), "+Inf".to_string()));
                    push_metric(
                        &mut output,
                        &bucket_name,
                        &labels,
                        &histogram.count.to_string(),
                    );
                    push_metric(
                        &mut output,
                        &format!("{}_sum", family_snapshot.family.name),
                        &sample.labels,
                        &format_float(histogram.sum),
                    );
                    push_metric(
                        &mut output,
                        &format!("{}_count", family_snapshot.family.name),
                        &sample.labels,
                        &histogram.count.to_string(),
                    );
                }
            }
        }
    }
    output.push_str("# EOF\n");
    output
}

fn write_family_header(output: &mut String, family: MetricFamily) {
    output.push_str("# HELP ");
    output.push_str(family.name);
    output.push(' ');
    output.push_str(family.help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(family.name);
    output.push(' ');
    output.push_str(family.kind.as_prometheus_type());
    output.push('\n');
}

fn push_metric(output: &mut String, name: &str, labels: &[(String, String)], value: &str) {
    output.push_str(name);
    if !labels.is_empty() {
        output.push('{');
        for (index, (key, value)) in labels.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(key.as_str());
            output.push_str("=\"");
            push_escaped_label_value(output, value);
            output.push('"');
        }
        output.push('}');
    }
    output.push(' ');
    output.push_str(value);
    output.push('\n');
}

fn push_escaped_label_value(output: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            _ => output.push(ch),
        }
    }
}

fn format_float(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}
