//! Framework-authenticated ordering evidence attached by receive adapters.
//!
//! A public [`Message`](super::Message) deliberately cannot carry this value in
//! metadata. Only transport implementations inside this crate can construct an
//! `OrderedDelivery`; the shared runner forwards it separately to the message
//! router. Causal projector routes fail closed when it is absent.

use crate::projection_protocol::{
    ProjectionEpoch, ProjectionProtocolValidationError, ProjectionSource, MAX_PROJECTION_POSITION,
};

/// Exact ordered position authenticated by a built-in source adapter.
///
/// The fields are readable for diagnostics but construction is crate-private.
/// Application messages, UUIDs, timestamps, delivery attempts, and arbitrary
/// broker headers can therefore never mint projection ordering evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedDelivery {
    source: ProjectionSource,
    epoch: ProjectionEpoch,
    position: u64,
    gap_free: bool,
}

impl OrderedDelivery {
    pub(crate) fn new(
        source: ProjectionSource,
        epoch: ProjectionEpoch,
        position: u64,
        gap_free: bool,
    ) -> Result<Self, ProjectionProtocolValidationError> {
        if position > MAX_PROJECTION_POSITION {
            return Err(ProjectionProtocolValidationError::TooLarge {
                field: "ordered delivery position",
                value: position,
                max: MAX_PROJECTION_POSITION,
            });
        }
        Ok(Self {
            source,
            epoch,
            position,
            gap_free,
        })
    }

    pub fn source(&self) -> &ProjectionSource {
        &self.source
    }

    pub fn epoch(&self) -> &ProjectionEpoch {
        &self.epoch
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn is_gap_free(&self) -> bool {
        self.gap_free
    }
}
