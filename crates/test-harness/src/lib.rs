//! Deterministic runtime integration fixtures.
//!
//! Relay/blob/signer protocol services live in the conformance workstream. This
//! crate supplies a finite fake for the public host-data seam so runtime
//! ownership can be tested without importing or emulating NMP internals.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use nmp_native_runtime_core::{
    AcceptedWrite, BindingEventSink, BindingRequest, HostBindingHandle, HostBindingSnapshot,
    HostDataError, HostDataPlane, ReceiptEventSink, ReceiptObservation, ReceiptReattachment,
    ReceiptSnapshot, WriteReceiptId,
};
use parking_lot::Mutex;

mod services;

pub use services::{
    BlobBody, BlobResponse, BlobScenarioService, DeterministicServiceError, FixtureLoader,
    FsFixtureLoader, ManualClock, RelayAction, RelayConnection, RelayScenarioService,
    ScenarioCatalog, ServiceCensus, SignerOutcome, SignerScenarioService,
};

#[derive(Debug)]
pub struct FakeHostDataPlane {
    maximum_bindings: usize,
    next_id: AtomicU64,
    state: Arc<Mutex<FakeState>>,
}

#[derive(Debug, Default)]
struct FakeState {
    bindings: BTreeMap<String, FakeBinding>,
    receipts: BTreeMap<String, FakeReceipt>,
}

#[derive(Debug)]
struct FakeBinding {
    request: BindingRequest,
    sink: Arc<dyn BindingEventSink>,
}

#[derive(Debug)]
struct FakeReceipt {
    accepted: AcceptedWrite,
    latest: ReceiptSnapshot,
    observers: Vec<Weak<dyn ReceiptEventSink>>,
}

impl FakeHostDataPlane {
    pub fn new(maximum_bindings: usize) -> Self {
        assert!(maximum_bindings > 0);
        Self {
            maximum_bindings,
            next_id: AtomicU64::new(0),
            state: Arc::new(Mutex::new(FakeState::default())),
        }
    }

    pub fn binding_count(&self) -> usize {
        self.state.lock().bindings.len()
    }

    pub fn receipt_count(&self) -> usize {
        self.state.lock().receipts.len()
    }

    pub fn emit_binding(
        &self,
        logical_id: &str,
        snapshot: HostBindingSnapshot,
    ) -> Result<(), HostDataError> {
        let sink = self
            .state
            .lock()
            .bindings
            .get(logical_id)
            .map(|binding| Arc::clone(&binding.sink))
            .ok_or_else(|| HostDataError::BindingRefused {
                reason: Arc::from("unknown fake binding"),
            })?;
        sink.push_latest(snapshot)
            .map_err(|error| HostDataError::BindingRefused {
                reason: Arc::from(error.to_string()),
            })
    }

    pub fn binding_request(&self, logical_id: &str) -> Option<BindingRequest> {
        self.state
            .lock()
            .bindings
            .get(logical_id)
            .map(|binding| binding.request.clone())
    }
}

impl HostDataPlane for FakeHostDataPlane {
    fn open_binding(
        &self,
        request: BindingRequest,
        sink: Arc<dyn BindingEventSink>,
    ) -> Result<Arc<dyn HostBindingHandle>, HostDataError> {
        let mut state = self.state.lock();
        if state.bindings.len() >= self.maximum_bindings {
            return Err(HostDataError::ExecutorSaturated {
                component: Arc::from("fake-binding"),
                capacity: self.maximum_bindings,
            });
        }
        let ordinal = self.next_id.fetch_add(1, Ordering::AcqRel) + 1;
        let logical_id = format!("fake-observation-{ordinal}");
        state
            .bindings
            .insert(logical_id.clone(), FakeBinding { request, sink });
        Ok(Arc::new(FakeHandle {
            logical_id,
            binding_state: Arc::downgrade(&self.state),
            closed: AtomicBool::new(false),
        }))
    }

    fn accept_write(
        &self,
        approved: nmp_native_runtime_core::ApprovedWrite,
        receipt_sink: Arc<dyn ReceiptEventSink>,
    ) -> Result<AcceptedWrite, HostDataError> {
        let ordinal = self.next_id.fetch_add(1, Ordering::AcqRel) + 1;
        let receipt_id = WriteReceiptId(Arc::from(format!("fake-receipt-{ordinal}")));
        let accepted = AcceptedWrite {
            receipt_id: receipt_id.clone(),
            frozen_account: approved.account,
        };
        let latest = ReceiptSnapshot {
            receipt_id: receipt_id.clone(),
            state: nmp_native_runtime_core::BoundedJson::from_value(
                &serde_json::json!({"stage": "accepted"}),
                1024,
            )
            .expect("static receipt fits"),
        };
        receipt_sink
            .push_latest(latest.clone())
            .map_err(|error| HostDataError::WriteRefused {
                reason: Arc::from(error.to_string()),
            })?;
        self.state.lock().receipts.insert(
            receipt_id.0.to_string(),
            FakeReceipt {
                accepted: accepted.clone(),
                latest,
                observers: vec![Arc::downgrade(&receipt_sink)],
            },
        );
        Ok(accepted)
    }

    fn reattach_receipt(
        &self,
        receipt_id: WriteReceiptId,
        receipt_sink: Arc<dyn ReceiptEventSink>,
    ) -> Result<ReceiptReattachment, HostDataError> {
        let mut state = self.state.lock();
        let Some(receipt) = state.receipts.get_mut(receipt_id.0.as_ref()) else {
            return Ok(ReceiptReattachment::NotFound);
        };
        debug_assert_eq!(receipt.accepted.receipt_id, receipt_id);
        receipt_sink
            .push_latest(receipt.latest.clone())
            .map_err(|error| HostDataError::ReceiptUnreadable {
                reason: Arc::from(error.to_string()),
            })?;
        receipt.observers.push(Arc::downgrade(&receipt_sink));
        Ok(ReceiptReattachment::Attached(Arc::new(
            FakeReceiptObservation {
                receipt_id,
                closed: AtomicBool::new(false),
            },
        )))
    }
}

#[derive(Debug)]
struct FakeHandle {
    logical_id: String,
    binding_state: Weak<Mutex<FakeState>>,
    closed: AtomicBool,
}

impl HostBindingHandle for FakeHandle {
    fn logical_id(&self) -> &str {
        &self.logical_id
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(state) = self.binding_state.upgrade() {
            state.lock().bindings.remove(&self.logical_id);
        }
    }
}

#[derive(Debug)]
struct FakeReceiptObservation {
    receipt_id: WriteReceiptId,
    closed: AtomicBool,
}

impl ReceiptObservation for FakeReceiptObservation {
    fn receipt_id(&self) -> &WriteReceiptId {
        &self.receipt_id
    }

    fn stop_delivery(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

impl Drop for FakeHandle {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Debug)]
pub struct LatestReceiptSink {
    maximum_bytes: usize,
    latest: Mutex<Option<ReceiptSnapshot>>,
    closed: AtomicBool,
}

impl LatestReceiptSink {
    pub fn new(maximum_bytes: usize) -> Self {
        Self {
            maximum_bytes,
            latest: Mutex::new(None),
            closed: AtomicBool::new(false),
        }
    }

    pub fn latest(&self) -> Option<ReceiptSnapshot> {
        self.latest.lock().clone()
    }
}

impl ReceiptEventSink for LatestReceiptSink {
    fn push_latest(
        &self,
        snapshot: ReceiptSnapshot,
    ) -> Result<(), nmp_native_runtime_core::ReceiptSinkError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(nmp_native_runtime_core::ReceiptSinkError::Closed);
        }
        if snapshot.state.byte_len() > self.maximum_bytes {
            return Err(nmp_native_runtime_core::ReceiptSinkError::FrameTooLarge);
        }
        *self.latest.lock() = Some(snapshot);
        Ok(())
    }

    fn close(&self, _reason: Option<Arc<str>>) {
        self.closed.store(true, Ordering::Release);
    }
}

impl fmt::Display for FakeHostDataPlane {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "FakeHostDataPlane(bindings={}, receipts={})",
            self.binding_count(),
            self.receipt_count()
        )
    }
}

#[cfg(test)]
mod tests {
    use nmp_native_runtime_core::{
        AccountRef, ApprovedWrite, BindingRequest, BoundedJson, Principal, SessionId,
    };

    use super::*;

    fn principal() -> Principal {
        Principal::new("a".repeat(64), "composer", "b".repeat(64)).unwrap()
    }

    #[test]
    fn accepted_write_survives_component_scope_and_freezes_account() {
        let host = FakeHostDataPlane::new(4);
        let sink = Arc::new(LatestReceiptSink::new(1024));
        let accepted = host
            .accept_write(
                ApprovedWrite {
                    approval_id: Arc::from("approval-1"),
                    origin_principal: principal(),
                    origin_session: SessionId(9),
                    account: AccountRef(Arc::from("account-a")),
                    draft: BoundedJson::from_value(
                        &serde_json::json!({"kind": 1, "content": "hello"}),
                        1024,
                    )
                    .unwrap(),
                },
                sink,
            )
            .unwrap();
        // No session/component object is retained by the fake durable owner.
        assert_eq!(accepted.frozen_account.0.as_ref(), "account-a");
        assert_eq!(host.receipt_count(), 1);
        let replacement_sink = Arc::new(LatestReceiptSink::new(1024));
        assert!(matches!(
            host.reattach_receipt(accepted.receipt_id, replacement_sink),
            Ok(ReceiptReattachment::Attached(_))
        ));
    }

    #[test]
    fn binding_capacity_is_zero_queue_and_observable() {
        let host = FakeHostDataPlane::new(1);
        let binding = nmp_native_surface::Binding::new(
            "feed",
            "nostr.events.collection/1",
            nmp_native_surface::BindingLimits::default(),
        )
        .unwrap();
        let request = BindingRequest {
            workspace_binding_id: Arc::from("feed"),
            family: Arc::from("event-window"),
            schema: Arc::from("nostr.events.collection/1"),
            parameters: BoundedJson::from_value(&serde_json::json!({}), 1024).unwrap(),
            maximum_rows: 100,
            maximum_frame_bytes: 100_000,
        };
        let handle = host.open_binding(request.clone(), binding.clone()).unwrap();
        binding.attach_source(handle).unwrap();
        assert!(matches!(
            host.open_binding(request, binding),
            Err(HostDataError::ExecutorSaturated { capacity: 1, .. })
        ));
    }
}
