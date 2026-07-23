use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use parking_lot::{Condvar, Mutex};

/// Event-driven cancellation signal shared by one bounded native operation.
#[derive(Clone, Debug)]
pub struct Cancellation {
    inner: Arc<Inner>,
}

#[derive(Debug)]
pub(crate) struct Inner {
    cancelled: AtomicBool,
    gate: Mutex<()>,
    changed: Condvar,
}

impl Inner {
    pub(crate) fn cancel(&self) {
        let was_cancelled = self.cancelled.swap(true, Ordering::AcqRel);
        if !was_cancelled {
            self.changed.notify_all();
        }
    }
}

impl Cancellation {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                cancelled: AtomicBool::new(false),
                gate: Mutex::new(()),
                changed: Condvar::new(),
            }),
        }
    }

    pub fn cancel(&self) -> bool {
        let was_cancelled = self.inner.cancelled.load(Ordering::Acquire);
        self.inner.cancel();
        !was_cancelled
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub fn wait(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut guard = self.inner.gate.lock();
        while !self.is_cancelled() {
            self.inner.changed.wait(&mut guard);
        }
    }

    pub fn wait_until(&self, deadline: Instant) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            return Ok(());
        }
        let mut guard = self.inner.gate.lock();
        loop {
            if self.is_cancelled() {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Cancelled);
            }
            self.inner
                .changed
                .wait_for(&mut guard, deadline.saturating_duration_since(now));
        }
    }

    pub fn wait_for(&self, duration: Duration) -> Result<(), Cancelled> {
        self.wait_until(Instant::now() + duration)
    }

    pub(crate) fn registry_weak(&self) -> std::sync::Weak<Inner> {
        Arc::downgrade(&self.inner)
    }
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;
