//! Authentication for the private HTTP boundary between a command host and cells.

use std::sync::Arc;

use sha2::{Digest, Sha256};

/// Environment variable containing the internal cell HTTP secret.
pub const CELL_INTERNAL_SECRET_ENV: &str = "DISTRIBUTED_INTERNAL_SECRET";
/// Header used only on authenticated host-to-cell HTTP requests.
pub const CELL_INTERNAL_SECRET_HEADER: &str = "x-distributed-internal-secret";

const MIN_SECRET_LEN: usize = 32;
const MAX_SECRET_LEN: usize = 512;

/// Validated secret for the private command-host/cell HTTP boundary.
///
/// The value is redacted from Debug output; comparisons use fixed-size digests and
/// constant-time byte accumulation.
#[derive(Clone)]
pub struct InternalHttpSecret {
    value: Arc<str>,
    digest: [u8; 32],
}

impl InternalHttpSecret {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if !(MIN_SECRET_LEN..=MAX_SECRET_LEN).contains(&value.len()) {
            return Err(format!(
                "internal HTTP secret must be {MIN_SECRET_LEN}..={MAX_SECRET_LEN} bytes"
            ));
        }
        if !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Err(
                "internal HTTP secret must contain only visible ASCII without whitespace".into(),
            );
        }
        let digest: [u8; 32] = Sha256::digest(value.as_bytes()).into();
        Ok(Self {
            value: Arc::from(value),
            digest,
        })
    }

    /// Header value for an outbound request. Never log this value.
    pub fn header_value(&self) -> &str {
        &self.value
    }

    /// Verify an inbound header without early-exit comparison of secret bytes.
    pub fn matches(&self, candidate: &str) -> bool {
        let candidate: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        candidate
            .iter()
            .zip(self.digest.iter())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
    }
}

impl std::fmt::Debug for InternalHttpSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InternalHttpSecret([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-only-internal-secret-32-bytes";

    #[test]
    fn validates_and_matches_exact_secret() {
        let secret = InternalHttpSecret::new(SECRET).expect("valid secret");
        assert!(secret.matches(SECRET));
        assert!(!secret.matches("test-only-internal-secret-32-bytez"));
        assert!(!secret.matches(""));
        assert_eq!(format!("{secret:?}"), "InternalHttpSecret([redacted])");
    }

    #[test]
    fn rejects_short_long_or_header_ambiguous_values() {
        assert!(InternalHttpSecret::new("short").is_err());
        assert!(InternalHttpSecret::new("x".repeat(MAX_SECRET_LEN + 1)).is_err());
        assert!(InternalHttpSecret::new(format!("{SECRET}\n")).is_err());
        assert!(InternalHttpSecret::new(format!(" {SECRET}")).is_err());
    }
}
