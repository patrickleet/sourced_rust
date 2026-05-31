//! Failure handling policy for the async transport runner.
//!
//! The runner classifies each handler/transport failure with
//! [`TransportError`](super::TransportError). Retryable failures are always
//! redelivered. For *permanent* failures the runner consults a
//! [`FailurePolicy`] and performs the resolved [`FailureAction`]. The default
//! never silently acknowledges a handler error.

use super::TransportError;

/// What the runner should do with a message after a failure.
///
/// This is the decision; the runner (a later subtask) performs the side effect
/// through the transport adapter's ack/nack/dead-letter primitives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FailureAction {
    /// Negative-acknowledge (or leave unacked) so the transport redelivers the
    /// message later, subject to its retry/backoff/max-attempt rules.
    Nack,
    /// Route the message to a dead-letter destination, then acknowledge it so it
    /// leaves the main flow.
    DeadLetter,
    /// Hold the message for manual intervention without acknowledging it and
    /// without automatic redelivery. Adapters that cannot park should surface a
    /// clear error rather than silently dropping the message.
    Park,
    /// Log the error and acknowledge the message, dropping it. This is the only
    /// action that discards a failed message, so it must be opt-in.
    LogAndAck,
    /// Stop the runner loop and surface the error to the caller.
    Stop,
}

/// Policy for handling *permanent* (non-retryable) failures.
///
/// Retryable failures bypass the policy and are always nacked for redelivery;
/// the policy decides what happens once an error is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FailurePolicy {
    /// Redeliver everything, even permanent failures. Bounded only by the
    /// transport's retry/backoff/max-attempt configuration.
    Retry,
    /// Send permanent failures to a dead-letter destination, then acknowledge.
    DeadLetter,
    /// Park permanent failures for manual intervention.
    Park,
    /// Log permanent failures and acknowledge, dropping the message.
    LogAndAck,
    /// Stop the runner on a permanent failure.
    Stop,
}

impl Default for FailurePolicy {
    /// Dead-letter: a permanent failure is moved aside for inspection rather
    /// than dropped or redelivered forever. Adapters without a dead-letter
    /// destination should treat this as [`FailureAction::Park`] and report a
    /// clear error instead of silently acknowledging.
    fn default() -> Self {
        FailurePolicy::DeadLetter
    }
}

impl FailurePolicy {
    /// Resolve the action the runner should take for a classified error.
    ///
    /// Retryable errors always resolve to [`FailureAction::Nack`] regardless of
    /// policy, because "retry later" is the correct response to a transient
    /// failure on every transport. Only permanent errors are routed through the
    /// configured policy.
    pub fn resolve(self, error: &TransportError) -> FailureAction {
        if error.is_retryable() {
            return FailureAction::Nack;
        }
        match self {
            FailurePolicy::Retry => FailureAction::Nack,
            FailurePolicy::DeadLetter => FailureAction::DeadLetter,
            FailurePolicy::Park => FailureAction::Park,
            FailurePolicy::LogAndAck => FailureAction::LogAndAck,
            FailurePolicy::Stop => FailureAction::Stop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_dead_letters_permanent_failures() {
        assert_eq!(FailurePolicy::default(), FailurePolicy::DeadLetter);
        let action = FailurePolicy::default().resolve(&TransportError::permanent("bad"));
        assert_eq!(action, FailureAction::DeadLetter);
    }

    #[test]
    fn retryable_errors_always_nack_regardless_of_policy() {
        let retry = TransportError::retryable("transient");
        for policy in [
            FailurePolicy::Retry,
            FailurePolicy::DeadLetter,
            FailurePolicy::Park,
            FailurePolicy::LogAndAck,
            FailurePolicy::Stop,
        ] {
            assert_eq!(
                policy.resolve(&retry),
                FailureAction::Nack,
                "{policy:?} should nack a retryable error"
            );
        }
    }

    #[test]
    fn permanent_failures_follow_the_configured_policy() {
        let permanent = TransportError::permanent("terminal");
        assert_eq!(
            FailurePolicy::Retry.resolve(&permanent),
            FailureAction::Nack
        );
        assert_eq!(
            FailurePolicy::DeadLetter.resolve(&permanent),
            FailureAction::DeadLetter
        );
        assert_eq!(FailurePolicy::Park.resolve(&permanent), FailureAction::Park);
        assert_eq!(
            FailurePolicy::LogAndAck.resolve(&permanent),
            FailureAction::LogAndAck
        );
        assert_eq!(FailurePolicy::Stop.resolve(&permanent), FailureAction::Stop);
    }

    #[test]
    fn only_log_and_ack_discards_a_failed_message() {
        let permanent = TransportError::permanent("terminal");
        let discards =
            |policy: FailurePolicy| policy.resolve(&permanent) == FailureAction::LogAndAck;
        assert!(discards(FailurePolicy::LogAndAck));
        assert!(!discards(FailurePolicy::Retry));
        assert!(!discards(FailurePolicy::DeadLetter));
        assert!(!discards(FailurePolicy::Park));
        assert!(!discards(FailurePolicy::Stop));
    }
}
