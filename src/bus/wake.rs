//! Runtime-free wakeup for wait-when-idle.
//!
//! `tokio` is optional. In-memory listen still needs to park until `send`,
//! so this is a small multi-waiter notify that compiles in the default crate.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[derive(Clone, Default)]
pub(crate) struct Notify {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    notified: bool,
    waiters: Vec<Waker>,
}

impl Notify {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn notify_waiters(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.notified = true;
        for waker in inner.waiters.drain(..) {
            waker.wake();
        }
    }

    pub(crate) fn notified(&self) -> Notified {
        Notified {
            inner: Arc::clone(&self.inner),
        }
    }
}

pub(crate) struct Notified {
    inner: Arc<Mutex<Inner>>,
}

impl Future for Notified {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner.notified {
            inner.notified = false;
            return Poll::Ready(());
        }
        let waker = cx.waker();
        if !inner
            .waiters
            .iter()
            .any(|existing| existing.will_wake(waker))
        {
            inner.waiters.push(waker.clone());
        }
        Poll::Pending
    }
}
