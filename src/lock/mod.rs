//! Lock - Pluggable locking abstractions
//!
//! This module provides traits and implementations for per-entity locking,
//! used by `QueuedRepository` to serialize aggregate access.
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

mod async_in_memory;
mod async_lock;
mod async_lock_manager;
mod error;
mod in_memory;
mod lock;
mod lock_manager;

pub use async_in_memory::{InMemoryAsyncLock, InMemoryAsyncLockFuture, InMemoryAsyncLockManager};
pub use async_lock::AsyncLock;
pub use async_lock_manager::AsyncLockManager;
pub use error::LockError;
pub use in_memory::{InMemoryLock, InMemoryLockManager};
pub use lock::Lock;
pub use lock_manager::LockManager;
