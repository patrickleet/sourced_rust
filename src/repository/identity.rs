use std::fmt;

use super::RepositoryError;

/// Stable event-stream identity used by persistence backends.
///
/// Durable stores should key aggregate streams by both aggregate type and
/// aggregate id. ID-only repository APIs remain available for the synchronous
/// in-memory path, but production adapters should prefer this type.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct StreamIdentity {
    aggregate_type: String,
    aggregate_id: String,
}

impl StreamIdentity {
    /// Create a validated stream identity.
    pub fn new(
        aggregate_type: impl Into<String>,
        aggregate_id: impl Into<String>,
    ) -> Result<Self, RepositoryError> {
        let aggregate_type = aggregate_type.into();
        let aggregate_id = aggregate_id.into();

        if aggregate_type.trim().is_empty() {
            return Err(RepositoryError::InvalidStreamIdentity {
                aggregate_type,
                aggregate_id,
                reason: "aggregate type must not be empty".into(),
            });
        }

        if aggregate_id.trim().is_empty() {
            return Err(RepositoryError::InvalidStreamIdentity {
                aggregate_type,
                aggregate_id,
                reason: "aggregate id must not be empty".into(),
            });
        }

        Ok(Self {
            aggregate_type,
            aggregate_id,
        })
    }

    /// Stable aggregate type component.
    pub fn aggregate_type(&self) -> &str {
        &self.aggregate_type
    }

    /// Aggregate id component.
    pub fn aggregate_id(&self) -> &str {
        &self.aggregate_id
    }

    pub(crate) fn storage_key(&self) -> String {
        format!("{}\u{1f}{}", self.aggregate_type, self.aggregate_id)
    }
}

impl fmt::Display for StreamIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.aggregate_type, self.aggregate_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_non_empty_type_and_id() {
        let identity = StreamIdentity::new("orders", "order-1").unwrap();

        assert_eq!(identity.aggregate_type(), "orders");
        assert_eq!(identity.aggregate_id(), "order-1");
    }

    #[test]
    fn rejects_empty_aggregate_type() {
        let err = StreamIdentity::new(" ", "order-1").unwrap_err();

        assert!(matches!(
            err,
            RepositoryError::InvalidStreamIdentity { reason, .. }
                if reason == "aggregate type must not be empty"
        ));
    }

    #[test]
    fn rejects_empty_aggregate_id() {
        let err = StreamIdentity::new("orders", "").unwrap_err();

        assert!(matches!(
            err,
            RepositoryError::InvalidStreamIdentity { reason, .. }
                if reason == "aggregate id must not be empty"
        ));
    }
}
