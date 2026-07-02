//! Test-only helpers shared by the outbox worker test modules.

use std::future::Future;

/// Minimal busy-poll executor for tests. Runs a future to completion without
/// any runtime, which also asserts the futures under test never need a
/// reactor or timer.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    use std::ptr;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    let waker = unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
}
