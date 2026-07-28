//! Temporary compatibility export for the pre-cutover shared registry.
//!
//! Task 20 removes this module after `TodoState` becomes the canonical export.

pub use crate::projection_v2::TodoState as TodoFact;
