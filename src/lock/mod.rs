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
//! ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
//! │ InMemoryAsyncLock│  │   PostgresLock   │  │    SqliteLock    │
//! │   (process-local)│  │ (durable lease,  │  │ (durable lease,  │
//! │                  │  │  feature=postgres│  │  feature=sqlite) │
//! └──────────────────┘  └──────────────────┘  └──────────────────┘
//! ```
//!
//! The SQLx locks ([`PostgresLock`]/[`SqliteLock`]) persist a per-stream lease in
//! the `aggregate_locks` table so a `QueuedRepository` can serialize aggregate
//! access across processes.

mod async_in_memory;
mod async_lock;
mod async_lock_manager;
mod error;
#[cfg(feature = "postgres")]
mod postgres_lock;
#[cfg(feature = "sqlite")]
mod sqlite_lock;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod sqlx_common;

pub use async_in_memory::{InMemoryAsyncLock, InMemoryAsyncLockFuture, InMemoryAsyncLockManager};
pub use async_lock::AsyncLock;
pub use async_lock_manager::AsyncLockManager;
pub use error::{LockError, RetryClass};
#[cfg(feature = "postgres")]
pub use postgres_lock::{PostgresLock, PostgresLockManager};
#[cfg(feature = "sqlite")]
pub use sqlite_lock::{SqliteLock, SqliteLockManager};
