use std::sync::{Condvar, Mutex};

pub struct Lock {
    state: Mutex<bool>,
    wake: Condvar,
}

impl Lock {
    pub fn new() -> Self {
        Lock {
            state: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    pub fn lock(&self) {
        let mut locked = self.state.lock().unwrap();
        while *locked {
            locked = self.wake.wait(locked).unwrap();
        }
        *locked = true;
    }

    pub fn try_lock(&self) -> bool {
        let mut locked = self.state.lock().unwrap();
        if *locked {
            false
        } else {
            *locked = true;
            true
        }
    }

    pub fn unlock(&self) {
        let mut locked = self.state.lock().unwrap();
        if *locked {
            *locked = false;
            self.wake.notify_one();
        }
    }

    #[allow(dead_code)]
    pub fn is_locked(&self) -> bool {
        *self.state.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_new() {
        let lock = Lock::new();
        assert!(!lock.is_locked());
    }

    #[test]
    fn test_lock_lock() {
        let lock = Lock::new();
        lock.lock();
        assert!(lock.is_locked());
    }

    #[test]
    fn test_lock_try_lock() {
        let lock = Lock::new();
        assert!(lock.try_lock());
        assert!(!lock.try_lock());
        lock.unlock();
        assert!(lock.try_lock());
    }

    #[test]
    fn test_lock_unlock() {
        let lock = Lock::new();
        lock.lock();
        lock.unlock();
        assert!(!lock.is_locked());
    }

    #[test]
    fn test_lock_is_locked() {
        let lock = Lock::new();
        assert!(!lock.is_locked());
        lock.lock();
        assert!(lock.is_locked());
        lock.unlock();
        assert!(!lock.is_locked());
    }
}
