use std::sync::OnceLock;

use super::registry::registry;

static TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    registry().reset();
}

#[cfg(test)]
pub(crate) fn lock_for_tests() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .blocking_lock()
}

#[cfg(test)]
pub(crate) async fn async_lock_for_tests() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}
