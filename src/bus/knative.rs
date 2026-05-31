//! Knative manifest helpers: render `Trigger` YAML and sanitize Kubernetes names.
//!
//! The CloudEvents HTTP *ingress* (the consume side) is `Service`-coupled and
//! lives on the microsvc side (`microsvc::cloud_events_router`); this module keeps
//! only the produce/manifest helpers, which need just the bus vocabulary
//! ([`SubscriptionPlan`]). Used by [`KnativeBus`](super::KnativeBus).
//!
//! Requires the `http` feature.

use super::SubscriptionPlan;

/// Sanitize a string into an RFC 1123 DNS label usable as a Kubernetes resource
/// name: lowercase, ASCII-alphanumeric or `-`, no leading/trailing `-`, ≤63
/// chars. Used for generated `Trigger` names, whose CloudEvent-type segment can
/// contain dots/uppercase/other characters that are invalid in k8s names.
pub(super) fn sanitize_k8s_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() {
                c
            } else {
                '-'
            }
        })
        .collect();
    let capped: String = mapped.trim_matches('-').chars().take(63).collect();
    let trimmed = capped.trim_end_matches('-');
    if trimmed.is_empty() {
        "x".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Render Knative `Trigger` YAML for each event a service subscribes to, derived
/// from its [`SubscriptionPlan`]. Each Trigger filters on the CloudEvent `type`
/// and routes to `subscriber_service` on `broker`.
pub fn knative_triggers(plan: &SubscriptionPlan, broker: &str, subscriber_service: &str) -> String {
    let mut out = String::new();
    for event in &plan.events {
        let trigger_name = sanitize_k8s_name(&format!("{subscriber_service}-{event}"));
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

    #[test]
    fn sanitize_k8s_name_enforces_rfc1123() {
        // Valid names pass through unchanged.
        assert_eq!(
            sanitize_k8s_name("checkout-projection-seat-reserved"),
            "checkout-projection-seat-reserved"
        );
        // Dots, uppercase, and other characters become '-' and lowercase.
        assert_eq!(sanitize_k8s_name("Order.Created!"), "order-created");
        // No leading/trailing dashes, capped at 63 chars.
        let long = "a".repeat(80);
        let out = sanitize_k8s_name(&format!(".{long}."));
        assert_eq!(out.len(), 63);
        assert!(!out.starts_with('-') && !out.ends_with('-'));
        // All-invalid degrades to a safe placeholder.
        assert_eq!(sanitize_k8s_name("..."), "x");
    }

    #[test]
    fn trigger_name_is_sanitized_for_messy_event_types() {
        let plan = SubscriptionPlan {
            commands: vec![],
            events: vec!["Order.Created".to_string()],
        };
        let yaml = knative_triggers(&plan, "default", "checkout-projection");
        // The CloudEvent type filter keeps the raw type; the resource name is sanitized.
        assert!(yaml.contains("type: Order.Created"));
        assert!(yaml.contains("name: checkout-projection-order-created"));
    }
}
