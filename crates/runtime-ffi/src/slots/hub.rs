//! One lazy worker and bounded registry for all non-diagnostics slots.

use std::{
    collections::BTreeMap,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
};

use nmp_native_runtime_app::AppObserver;
use parking_lot::Mutex;

use super::{
    RuntimeReceiptsSlotObservation, RuntimeReceiptsSlotObserver, RuntimeReceiptsSlotProjection,
    control::{HubControl, reap},
    project_receipts,
};
use crate::{RuntimeController, RuntimeRefusal};

struct ReceiptObserverEntry {
    observer: Arc<dyn RuntimeReceiptsSlotObserver>,
    last_revision: u64,
}

struct HubState {
    closed: bool,
    next_observer_id: u64,
    receipts: BTreeMap<u64, ReceiptObserverEntry>,
    control: Option<Arc<HubControl>>,
    worker: Option<JoinHandle<()>>,
}

impl fmt::Debug for HubState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HubState")
            .field("closed", &self.closed)
            .field("receipt_observers", &self.receipts.len())
            .field("worker_open", &self.control.is_some())
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct SlotHub {
    state: Mutex<HubState>,
    global_observers: Arc<AtomicUsize>,
}

impl SlotHub {
    pub(crate) fn new(global_observers: Arc<AtomicUsize>) -> Self {
        Self {
            state: Mutex::new(HubState {
                closed: false,
                next_observer_id: 0,
                receipts: BTreeMap::new(),
                control: None,
                worker: None,
            }),
            global_observers,
        }
    }

    pub(crate) fn register_receipts(
        self: &Arc<Self>,
        controller: Arc<RuntimeController>,
        observer: Arc<dyn RuntimeReceiptsSlotObserver>,
        initial_revision: u64,
        app_observer: AppObserver,
    ) -> Result<Arc<RuntimeReceiptsSlotObservation>, RuntimeRefusal> {
        let (id, stale_worker) = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(controller.refusal("receipts-slot-closed", "runtime is closed"));
            }
            let id = state.next_observer_id;
            let Some(next_id) = id.checked_add(1) else {
                return Err(controller.refusal(
                    "receipts-slot-observer-id-exhausted",
                    "the receipt observer identity space is exhausted",
                ));
            };
            state.next_observer_id = next_id;
            state.receipts.insert(
                id,
                ReceiptObserverEntry {
                    observer,
                    last_revision: initial_revision,
                },
            );

            let stale_worker = if state.control.is_none() {
                let stale = state.worker.take();
                let control = Arc::new(HubControl::new());
                let hub = Arc::clone(self);
                let feed_control = Arc::clone(&control);
                let feed_controller = Arc::clone(&controller);
                let worker = thread::Builder::new()
                    .name("runtime-ffi-slot-hub".to_owned())
                    .spawn(move || {
                        hub.run_receipt_feed(feed_controller, app_observer, feed_control);
                    });
                match worker {
                    Ok(worker) => {
                        state.control = Some(control);
                        state.worker = Some(worker);
                        stale
                    }
                    Err(error) => {
                        state.receipts.remove(&id);
                        return Err(controller.refusal(
                            "slot-observer-thread",
                            format!("could not start the slot worker: {error}"),
                        ));
                    }
                }
            } else {
                None
            };
            (id, stale_worker)
        };
        reap(stale_worker);
        Ok(Arc::new(RuntimeReceiptsSlotObservation {
            hub: Arc::clone(self),
            id,
            stopped: AtomicBool::new(false),
        }))
    }

    fn run_receipt_feed(
        self: Arc<Self>,
        controller: Arc<RuntimeController>,
        mut app_observer: AppObserver,
        control: Arc<HubControl>,
    ) {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            self.fail_worker(
                &controller,
                &control,
                "could not construct the slot observation runtime",
            );
            return;
        };
        runtime.block_on(async {
            let mut cancellation = control.subscribe();
            loop {
                if control.is_cancelled() {
                    return;
                }
                tokio::select! {
                    changed = app_observer.changed() => {
                        let Ok(source) = changed else {
                            self.finish_closed(&control);
                            return;
                        };
                        let closed = source.closed;
                        let projection = project_receipts(&controller, &source);
                        let delivery = self.receipt_delivery(
                            &control,
                            source.revisions.receipts,
                            closed,
                            projection,
                        );
                        self.deliver(&controller, delivery);
                        if closed {
                            self.finish_closed(&control);
                            return;
                        }
                    }
                    cancelled = cancellation.changed() => {
                        if cancelled.is_err() || *cancellation.borrow() {
                            return;
                        }
                    }
                }
            }
        });
    }

    fn receipt_delivery(
        &self,
        control: &Arc<HubControl>,
        revision: u64,
        closed: bool,
        projection: RuntimeReceiptsSlotProjection,
    ) -> Vec<(
        u64,
        Arc<dyn RuntimeReceiptsSlotObserver>,
        RuntimeReceiptsSlotProjection,
    )> {
        let mut state = self.state.lock();
        if state.closed
            || control.is_cancelled()
            || state
                .control
                .as_ref()
                .is_none_or(|active| !Arc::ptr_eq(active, control))
        {
            return Vec::new();
        }
        state
            .receipts
            .iter_mut()
            .filter_map(|(id, entry)| {
                if !closed && revision <= entry.last_revision {
                    return None;
                }
                entry.last_revision = revision;
                Some((*id, Arc::clone(&entry.observer), projection.clone()))
            })
            .collect()
    }

    /// Foreign callbacks run outside the registry lock. A panic removes only
    /// that observer and becomes bounded refusal evidence.
    fn deliver(
        &self,
        controller: &RuntimeController,
        delivery: Vec<(
            u64,
            Arc<dyn RuntimeReceiptsSlotObserver>,
            RuntimeReceiptsSlotProjection,
        )>,
    ) {
        for (id, observer, projection) in delivery {
            if catch_unwind(AssertUnwindSafe(|| observer.update(projection))).is_err() {
                self.remove_receipts(id);
                controller.record_refusal(
                    "slot-observer-panic",
                    "a receipts slot observer panicked and was removed",
                );
            }
        }
    }

    fn fail_worker(&self, controller: &RuntimeController, control: &Arc<HubControl>, detail: &str) {
        let source = controller.app.snapshot();
        let refusal = controller.refusal("slot-observer-runtime", detail);
        let projection = RuntimeReceiptsSlotProjection::Refused {
            revision: source.revisions.receipts,
            closed: true,
            refusal: refusal.clone(),
        };
        let delivery = self.receipt_delivery(control, source.revisions.receipts, true, projection);
        self.deliver(controller, delivery);
        controller.record_refusal(refusal.code, refusal.detail);
        self.finish_closed(control);
    }

    pub(crate) fn remove_receipts(&self, id: u64) {
        let (removed, control) = {
            let mut state = self.state.lock();
            let removed = state.receipts.remove(&id).is_some();
            let control = if removed && state.receipts.is_empty() {
                state.control.take()
            } else {
                None
            };
            (removed, control)
        };
        if removed {
            self.global_observers.fetch_sub(1, Ordering::AcqRel);
        }
        if let Some(control) = control {
            control.cancel();
        }
    }

    fn finish_closed(&self, control: &Arc<HubControl>) {
        let removed = {
            let mut state = self.state.lock();
            if state
                .control
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, control))
            {
                state.closed = true;
                state.control = None;
                std::mem::take(&mut state.receipts).len()
            } else {
                0
            }
        };
        self.global_observers.fetch_sub(removed, Ordering::AcqRel);
        control.cancel();
    }
}
