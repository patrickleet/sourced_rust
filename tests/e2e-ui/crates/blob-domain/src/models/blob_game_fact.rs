//! Temporary compatibility export for the pre-cutover shared registry.
//!
//! Task 20 removes this module after [`BlobGameState`] becomes canonical.

pub use crate::projection_v2::BlobGameState as BlobGameFact;
