//! Temporary compatibility export for the pre-cutover shared registry.
//!
//! The semantic event remains `chat_message.posted`; Task 20 removes this alias
//! when `ChatMessageState` becomes canonical.

pub use crate::projection_v2::ChatMessageState as ChatMessagePosted;
