//! Wall clock. `wasm32-unknown-unknown` has no `SystemTime::now`.

use std::time::SystemTime;

pub(crate) fn now() -> SystemTime {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(js_sys::Date::now() as u64)
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        SystemTime::now()
    }
}
