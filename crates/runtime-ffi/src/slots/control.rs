//! Cancellation and safe stale-worker reaping for the shared slot worker.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread::{self, JoinHandle},
};

use tokio::sync::watch;

#[derive(Debug)]
pub(super) struct HubControl {
    cancelled: AtomicBool,
    signal: watch::Sender<bool>,
}

impl HubControl {
    pub(super) fn new() -> Self {
        let (signal, _) = watch::channel(false);
        Self {
            cancelled: AtomicBool::new(false),
            signal,
        }
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<bool> {
        self.signal.subscribe()
    }

    pub(super) fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.signal.send_replace(true);
        }
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub(super) fn reap(worker: Option<JoinHandle<()>>) {
    if let Some(worker) = worker {
        if worker.thread().id() == thread::current().id() {
            return;
        }
        let _ = worker.join();
    }
}
