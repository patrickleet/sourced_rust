//! Lock - Pluggable locking abstractions
//!
//! This module provides traits and implementations for per-entity locking,
//! used by `QueuedRepository` and `QueuedReadModelStore` to serialize access.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │              LockManager (per repository)                    │
//! │  - get_lock(id) → Arc<Lock>                                 │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     Lock Trait                               │
//! │  lock() / try_lock() / unlock()                              │
//! └─────────────────────────────────────────────────────────────┘
//!          │                  │                     │
//!          ▼                  ▼                     ▼
//! ┌─────────────┐    ┌─────────────┐    ┌─────────────────────┐
//! │InMemoryLock │    │ RedisLock   │    │ PostgresAdvisory    │
//! │ (included)  │    │ (external)  │    │    (external)       │
//! └─────────────┘    └─────────────┘    └─────────────────────┘
//! ```

mod error;
mod in_memory;
mod lock;
mod lock_manager;

pub use error::LockError;
pub use in_memory::{InMemoryLock, InMemoryLockManager};
pub use lock::Lock;
pub use lock_manager::LockManager;
