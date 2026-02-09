use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use super::{Lock, LockError, LockManager};

/// In-memory lock backed by `Mutex<bool>` + `Condvar`.
///
/// This is the default lock implementation — the same logic that was
/// previously the concrete `Lock` struct, now behind the `Lock` trait.
pub struct InMemoryLock {
    state: Mutex<bool>,
    wake: Condvar,
}

impl InMemoryLock {
    pub fn new() -> Self {
        InMemoryLock {
            state: Mutex::new(false),
            wake: Condvar::new(),
        }
    }
}

impl Default for InMemoryLock {
    fn default() -> Self {
        Self::new()
    }
}

impl Lock for InMemoryLock {
    fn lock(&self) -> Result<(), LockError> {
        let mut locked = self
            .state
            .lock()
            .map_err(|e| LockError::Poisoned(e.to_string()))?;
        while *locked {
            locked = self
                .wake
                .wait(locked)
                .map_err(|e| LockError::Poisoned(e.to_string()))?;
        }
        *locked = true;
        Ok(())
    }

    fn try_lock(&self) -> Result<bool, LockError> {
        let mut locked = self
            .state
            .lock()
            .map_err(|e| LockError::Poisoned(e.to_string()))?;
        if *locked {
            Ok(false)
        } else {
            *locked = true;
            Ok(true)
        }
    }

    fn unlock(&self) -> Result<(), LockError> {
        let mut locked = self
            .state
            .lock()
            .map_err(|e| LockError::Poisoned(e.to_string()))?;
        if *locked {
            *locked = false;
            self.wake.notify_one();
        }
        Ok(())
    }
}

/// In-memory lock manager backed by a `HashMap<String, Arc<InMemoryLock>>`.
///
/// This is the default `LockManager` — it lazily creates one `InMemoryLock`
/// per unique key and returns the same `Arc` for repeated lookups.
pub struct InMemoryLockManager {
    locks: Mutex<HashMap<String, Arc<InMemoryLock>>>,
}

impl InMemoryLockManager {
    pub fn new() -> Self {
        InMemoryLockManager {
            locks: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryLockManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LockManager for InMemoryLockManager {
    type Lock = InMemoryLock;

    fn get_lock(&self, id: &str) -> Result<Arc<InMemoryLock>, LockError> {
        let mut locks = self
            .locks
            .lock()
            .map_err(|_| LockError::Poisoned("lock manager map poisoned".into()))?;
        Ok(locks
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(InMemoryLock::new()))
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // InMemoryLock tests (migrated from old lock/mod.rs)
    // ========================================================================

    #[test]
    fn test_lock_new() {
        let lock = InMemoryLock::new();
        assert!(lock.try_lock().unwrap()); // unlocked by default
        lock.unlock().unwrap();
    }

    #[test]
    fn test_lock_lock() {
        let lock = InMemoryLock::new();
        lock.lock().unwrap();
        assert!(!lock.try_lock().unwrap()); // already locked
        lock.unlock().unwrap();
    }

    #[test]
    fn test_lock_try_lock() {
        let lock = InMemoryLock::new();
        assert!(lock.try_lock().unwrap());
        assert!(!lock.try_lock().unwrap());
        lock.unlock().unwrap();
        assert!(lock.try_lock().unwrap());
        lock.unlock().unwrap();
    }

    #[test]
    fn test_lock_unlock() {
        let lock = InMemoryLock::new();
        lock.lock().unwrap();
        lock.unlock().unwrap();
        assert!(lock.try_lock().unwrap()); // can lock again after unlock
        lock.unlock().unwrap();
    }

    // ========================================================================
    // InMemoryLockManager tests
    // ========================================================================

    #[test]
    fn same_id_returns_same_arc() {
        let manager = InMemoryLockManager::new();
        let lock1 = manager.get_lock("entity-1").unwrap();
        let lock2 = manager.get_lock("entity-1").unwrap();
        assert!(Arc::ptr_eq(&lock1, &lock2));
    }

    #[test]
    fn different_id_returns_different_arc() {
        let manager = InMemoryLockManager::new();
        let lock1 = manager.get_lock("entity-1").unwrap();
        let lock2 = manager.get_lock("entity-2").unwrap();
        assert!(!Arc::ptr_eq(&lock1, &lock2));
    }

    #[test]
    fn manager_locks_are_functional() {
        let manager = InMemoryLockManager::new();
        let lock = manager.get_lock("test").unwrap();

        assert!(lock.try_lock().unwrap());
        assert!(!lock.try_lock().unwrap());
        lock.unlock().unwrap();
        assert!(lock.try_lock().unwrap());
        lock.unlock().unwrap();
    }
}
