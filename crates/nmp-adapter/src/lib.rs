//! NMP public-facade adapter for bounded runtime bindings and durable writes.
//!
//! This crate is the only runtime crate that depends on NMP. It deliberately
//! imports only the supported `nmp` facade; mechanism crates are not
//! dependencies and no canonical Nostr row or write state is persisted here.

use std::{
    collections::BTreeMap,
    fmt,
    num::NonZeroUsize,
    str::FromStr,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

use nmp::{
    Binding, Demand, Durability, Engine, EngineConfig, EngineError, FifoReceiver, Filter,
    LiveQuery, ObservationCancel, ReceiptId, ReceiptReattachment as NmpReceiptReattachment,
    ReceiptReplayCursor, ShortfallFact, SourceStatus, Window, WindowLoad, WriteIntent,
    WritePayload, WriteRouting, WriteStatus,
};
use nmp_native_runtime_core::{
    AcceptedWrite, ApprovedWrite, BindingEventSink, BindingRequest, BindingSinkError, BoundedJson,
    HostBindingHandle, HostBindingSnapshot, HostDataError, HostDataPlane, PublicIdentity,
    PublicIdentityChangeSink, PublicIdentityDataPlane, PublicIdentityError,
    PublicIdentityObservation, PublicIdentityQuery, PublicIdentityRead, PublicIdentityReadLimits,
    PublicIdentitySubscription, ReceiptEventSink, ReceiptObservation, ReceiptReattachment,
    ReceiptSinkError, ReceiptSnapshot, WriteReceiptId,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

const EVENT_COLLECTION_FAMILY: &str = "event.collection";
const EVENT_COLLECTION_SCHEMA: &str = "nostr.events.collection/1";
const DEFAULT_INITIAL_ROWS: usize = 20;
const MIN_FRAME_BYTES: usize = 1_024;

/// One local trust profile backed by exactly one NMP engine.
pub struct NmpDataPlane {
    engine: Arc<Engine>,
    workers: Arc<WorkerAdmission>,
    identity: Arc<Mutex<IdentityState>>,
    closed: AtomicBool,
}

const MAX_IDENTITY_OBSERVERS: usize = 64;

#[derive(Debug)]
struct IdentityState {
    generation: u64,
    current: Option<nmp_native_runtime_core::AccountRef>,
    next_observer_id: u64,
    observers: BTreeMap<u64, Arc<dyn PublicIdentityChangeSink>>,
}

impl fmt::Debug for NmpDataPlane {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NmpDataPlane")
            .field(
                "active_workers",
                &self.workers.active.load(Ordering::Acquire),
            )
            .field("maximum_workers", &self.workers.maximum)
            .field("identity_observers", &self.identity.lock().observers.len())
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish()
    }
}

impl NmpDataPlane {
    pub fn open(
        config: EngineConfig,
        maximum_bridge_workers: usize,
    ) -> Result<Self, HostDataError> {
        if maximum_bridge_workers == 0 {
            return Err(HostDataError::BindingRefused {
                reason: Arc::from("maximum bridge workers must be non-zero"),
            });
        }
        let engine = Engine::new(config).map_err(map_open_engine_error)?;
        Ok(Self::from_engine(Arc::new(engine), maximum_bridge_workers))
    }

    pub fn from_engine(engine: Arc<Engine>, maximum_bridge_workers: usize) -> Self {
        assert!(
            maximum_bridge_workers > 0,
            "maximum bridge workers must be non-zero"
        );
        let current = engine
            .active_account()
            .ok()
            .flatten()
            .map(|pubkey| nmp_native_runtime_core::AccountRef(Arc::from(pubkey.to_string())));
        Self {
            engine,
            workers: Arc::new(WorkerAdmission {
                active: AtomicUsize::new(0),
                maximum: maximum_bridge_workers,
            }),
            identity: Arc::new(Mutex::new(IdentityState {
                generation: 0,
                current,
                next_observer_id: 0,
                observers: BTreeMap::new(),
            })),
            closed: AtomicBool::new(false),
        }
    }

    pub fn active_bridge_workers(&self) -> usize {
        self.workers.active.load(Ordering::Acquire)
    }

    /// Close the profile. Engine shutdown is idempotent and wakes all query
    /// and receipt drains without a polling loop.
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let observers = std::mem::take(&mut self.identity.lock().observers);
            for (_, observer) in observers {
                observer.close();
            }
            self.engine.shutdown();
        }
    }

    /// Native account-selection boundary. The adapter remains the exclusive
    /// mutator of its privately owned NMP engine and emits one change after
    /// the public facade confirms the new active account.
    pub fn set_active_public_identity(
        &self,
        account: Option<nmp_native_runtime_core::AccountRef>,
    ) -> Result<PublicIdentity, PublicIdentityError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(PublicIdentityError::Closed);
        }
        let parsed = account
            .as_ref()
            .map(|account| nmp::PublicKey::from_str(&account.0))
            .transpose()
            .map_err(|_| PublicIdentityError::InvalidSourceData)?;
        self.engine
            .set_active_account(parsed)
            .map_err(map_identity_engine_error)?;
        Ok(self.update_identity(account))
    }

    fn update_identity(
        &self,
        current: Option<nmp_native_runtime_core::AccountRef>,
    ) -> PublicIdentity {
        let (identity, observers) = {
            let mut state = self.identity.lock();
            let changed = state.current != current;
            if changed {
                state.generation = state.generation.saturating_add(1);
                state.current = current;
            }
            (
                PublicIdentity {
                    generation: state.generation,
                    account: state.current.clone(),
                },
                changed
                    .then(|| state.observers.values().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
        };
        for observer in observers {
            observer.changed(identity.clone());
        }
        identity
    }

    fn refresh_identity(&self) -> Result<PublicIdentity, PublicIdentityError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(PublicIdentityError::Closed);
        }
        let current = self
            .engine
            .active_account()
            .map_err(map_identity_engine_error)?
            .map(|pubkey| nmp_native_runtime_core::AccountRef(Arc::from(pubkey.to_string())));
        Ok(self.update_identity(current))
    }

    fn ensure_open(&self) -> Result<(), HostDataError> {
        if self.closed.load(Ordering::Acquire) {
            Err(HostDataError::ServiceClosed)
        } else {
            Ok(())
        }
    }

    fn spawn_receipt_drain(
        &self,
        component: &'static str,
        statuses: FifoReceiver<WriteStatus>,
        next_cursor: Option<ReceiptReplayCursor>,
        raw_receipt_id: ReceiptId,
        receipt_id: WriteReceiptId,
        sink: Arc<dyn ReceiptEventSink>,
        permit: WorkerPermit,
    ) -> Result<(JoinHandle<()>, Arc<ReceiptDeliveryControl>), HostDataError> {
        let engine = Arc::clone(&self.engine);
        let control = Arc::new(ReceiptDeliveryControl::default());
        let worker_control = Arc::clone(&control);
        let worker = thread::Builder::new()
            .name(component.to_owned())
            .spawn(move || {
                let _permit = permit;
                drain_receipt_pages(
                    engine,
                    raw_receipt_id,
                    statuses,
                    next_cursor,
                    receipt_id,
                    sink,
                    Some(worker_control),
                );
            })
            .map_err(|error| HostDataError::ThreadUnavailable {
                component: Arc::from(component),
                reason: Arc::from(error.to_string()),
            })?;
        Ok((worker, control))
    }
}

impl HostDataPlane for NmpDataPlane {
    fn open_binding(
        &self,
        request: BindingRequest,
        sink: Arc<dyn BindingEventSink>,
    ) -> Result<Arc<dyn HostBindingHandle>, HostDataError> {
        self.ensure_open()?;
        let permit = self.workers.reserve("nmp-binding")?;
        let query = collection_query(&request)?;
        let maximum_rows = NonZeroUsize::new(request.maximum_rows as usize).ok_or_else(|| {
            HostDataError::BindingRefused {
                reason: Arc::from("maximum_rows must be non-zero"),
            }
        })?;
        if request.maximum_frame_bytes < MIN_FRAME_BYTES {
            return Err(HostDataError::BindingRefused {
                reason: Arc::from("maximum_frame_bytes is below the 1024-byte minimum"),
            });
        }
        let initial = NonZeroUsize::new(DEFAULT_INITIAL_ROWS.min(maximum_rows.get()))
            .expect("the minimum of two non-zero row counts is non-zero");
        let subscription = self
            .engine
            .observe(
                query,
                Some(Window::Expandable {
                    initial,
                    max: maximum_rows,
                }),
            )
            .map_err(map_binding_engine_error)?;
        let cancel = subscription.cancel_handle();
        let handle = Arc::new(NmpBindingHandle {
            logical_id: Arc::clone(&request.workspace_binding_id),
            cancel,
            worker: Mutex::new(None),
        });
        let thread_handle = Arc::clone(&handle);
        let maximum_frame_bytes = request.maximum_frame_bytes;
        let worker = thread::Builder::new()
            .name("nmp-binding".to_owned())
            .spawn(move || {
                let _permit = permit;
                let mut generation = 0_u64;
                while let Ok(frame) = subscription.recv() {
                    generation = generation.saturating_add(1);
                    let snapshot = match project_frame(generation, frame, maximum_frame_bytes) {
                        Ok(snapshot) => snapshot,
                        Err(reason) => {
                            sink.close(Some(Arc::from(reason)));
                            return;
                        }
                    };
                    match sink.push_latest(snapshot) {
                        Ok(()) => {}
                        Err(BindingSinkError::Closed) => return,
                        Err(BindingSinkError::FrameTooLarge) => {
                            sink.close(Some(Arc::from("binding sink refused an oversized frame")));
                            return;
                        }
                    }
                }
                sink.close(None);
            })
            .map_err(|error| {
                thread_handle.cancel.cancel();
                HostDataError::ThreadUnavailable {
                    component: Arc::from("nmp-binding"),
                    reason: Arc::from(error.to_string()),
                }
            })?;
        *handle.worker.lock() = Some(worker);
        Ok(handle)
    }

    fn accept_write(
        &self,
        approved: ApprovedWrite,
        receipt_sink: Arc<dyn ReceiptEventSink>,
    ) -> Result<AcceptedWrite, HostDataError> {
        self.ensure_open()?;
        let permit = self.workers.reserve("nmp-receipt")?;
        type InitialReceipt = Option<(FifoReceiver<WriteStatus>, WriteReceiptId, ReceiptId)>;
        let (ready_tx, ready_rx): (
            mpsc::SyncSender<InitialReceipt>,
            mpsc::Receiver<InitialReceipt>,
        ) = mpsc::sync_channel(1);
        let frozen_account = approved.account.clone();
        let receipt_sink_for_worker = Arc::clone(&receipt_sink);
        let engine = Arc::clone(&self.engine);
        let worker = thread::Builder::new()
            .name("nmp-receipt".to_owned())
            .spawn(move || {
                let _permit = permit;
                let Ok(Some((statuses, receipt_id, raw_receipt_id))) = ready_rx.recv() else {
                    return;
                };
                drain_receipt_pages(
                    engine,
                    raw_receipt_id,
                    statuses,
                    None,
                    receipt_id,
                    receipt_sink_for_worker,
                    None,
                );
            })
            .map_err(|error| HostDataError::ThreadUnavailable {
                component: Arc::from("nmp-receipt"),
                reason: Arc::from(error.to_string()),
            })?;

        let intent = approved_write_intent(&approved)?;
        let stream = match self.engine.publish_tracked(intent) {
            Ok(stream) => stream,
            Err(error) => {
                let _ = ready_tx.send(None);
                let _ = worker.join();
                return Err(map_write_engine_error(error));
            }
        };
        let receipt_id = WriteReceiptId(Arc::from(stream.id.0.to_string()));
        if ready_tx
            .send(Some((stream.statuses, receipt_id.clone(), stream.id)))
            .is_err()
        {
            return Err(HostDataError::ReceiptUnreadable {
                reason: Arc::from("receipt drain terminated after NMP accepted the write"),
            });
        }
        // The profile owns the drain until the receipt terminates or the
        // engine closes. Detaching component UI never cancels the obligation.
        drop(worker);
        Ok(AcceptedWrite {
            receipt_id,
            frozen_account,
        })
    }

    fn reattach_receipt(
        &self,
        receipt_id: WriteReceiptId,
        receipt_sink: Arc<dyn ReceiptEventSink>,
    ) -> Result<ReceiptReattachment, HostDataError> {
        self.ensure_open()?;
        let raw_id = receipt_id
            .0
            .parse::<u64>()
            .map_err(|_| HostDataError::ReceiptUnreadable {
                reason: Arc::from("receipt id is not a valid NMP receipt identifier"),
            })?;
        let permit = self.workers.reserve("nmp-receipt-reattach")?;
        match self
            .engine
            .reattach_receipt(ReceiptId(raw_id))
            .map_err(map_receipt_engine_error)?
        {
            NmpReceiptReattachment::Attached {
                id,
                statuses,
                next_cursor,
            } => {
                let (worker, control) = self.spawn_receipt_drain(
                    "nmp-receipt-reattach",
                    statuses,
                    next_cursor,
                    id,
                    receipt_id.clone(),
                    receipt_sink,
                    permit,
                )?;
                Ok(ReceiptReattachment::Attached(Arc::new(
                    ReceiptDrainHandle {
                        receipt_id,
                        control,
                        worker: Mutex::new(Some(worker)),
                    },
                )))
            }
            NmpReceiptReattachment::NotFound => Ok(ReceiptReattachment::NotFound),
            NmpReceiptReattachment::RetainedButUnreadable => {
                Err(HostDataError::ReceiptUnreadable {
                    reason: Arc::from("NMP retained the receipt but its evidence is unreadable"),
                })
            }
        }
    }
}

impl PublicIdentityDataPlane for NmpDataPlane {
    fn freeze_public_identity(&self) -> Result<PublicIdentity, PublicIdentityError> {
        self.refresh_identity()
    }

    fn read_public_identity(
        &self,
        _frozen: &PublicIdentity,
        query: PublicIdentityQuery,
        cancellation: &nmp_native_runtime_core::Cancellation,
        _limits: PublicIdentityReadLimits,
    ) -> Result<PublicIdentityRead, PublicIdentityError> {
        if cancellation.is_cancelled() {
            return Err(PublicIdentityError::Cancelled);
        }
        // The pinned public facade exposes active-account read/selection, but
        // no governed identity/profile/list projection. Do not fabricate
        // those values or reach into mechanism crates.
        Err(PublicIdentityError::QueryUnavailable {
            query: Arc::from(public_identity_query_name(&query)),
        })
    }

    fn observe_public_identity(
        &self,
        sink: Arc<dyn PublicIdentityChangeSink>,
    ) -> Result<PublicIdentitySubscription, PublicIdentityError> {
        self.refresh_identity()?;
        let (current, id) = {
            let mut state = self.identity.lock();
            if self.closed.load(Ordering::Acquire) {
                return Err(PublicIdentityError::Closed);
            }
            if state.observers.len() >= MAX_IDENTITY_OBSERVERS {
                return Err(PublicIdentityError::ObserverCapacity {
                    capacity: MAX_IDENTITY_OBSERVERS,
                });
            }
            state.next_observer_id = state.next_observer_id.checked_add(1).ok_or_else(|| {
                PublicIdentityError::Failed {
                    reason: Arc::from("identity observer identifier space is exhausted"),
                }
            })?;
            let id = state.next_observer_id;
            state.observers.insert(id, sink);
            (
                PublicIdentity {
                    generation: state.generation,
                    account: state.current.clone(),
                },
                id,
            )
        };
        Ok(PublicIdentitySubscription {
            current,
            observation: Arc::new(NmpIdentityObservation {
                id,
                state: Arc::downgrade(&self.identity),
                closed: AtomicBool::new(false),
            }),
        })
    }
}

struct NmpIdentityObservation {
    id: u64,
    state: Weak<Mutex<IdentityState>>,
    closed: AtomicBool,
}

impl fmt::Debug for NmpIdentityObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NmpIdentityObservation")
            .field("id", &self.id)
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish()
    }
}

impl PublicIdentityObservation for NmpIdentityObservation {
    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(state) = self.state.upgrade() {
            state.lock().observers.remove(&self.id);
        }
    }
}

impl Drop for NmpIdentityObservation {
    fn drop(&mut self) {
        if !*self.closed.get_mut()
            && let Some(state) = self.state.upgrade()
        {
            state.lock().observers.remove(&self.id);
        }
    }
}

impl Drop for NmpDataPlane {
    fn drop(&mut self) {
        self.close();
    }
}

struct NmpBindingHandle {
    logical_id: Arc<str>,
    cancel: ObservationCancel,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl fmt::Debug for NmpBindingHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NmpBindingHandle")
            .field("logical_id", &self.logical_id)
            .field("worker_active", &self.worker.lock().is_some())
            .finish()
    }
}

impl HostBindingHandle for NmpBindingHandle {
    fn logical_id(&self) -> &str {
        &self.logical_id
    }

    fn close(&self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.lock().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for NmpBindingHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.get_mut().take() {
            let _ = worker.join();
        }
    }
}

/// Receipt delivery is independently stoppable; the durable NMP obligation is
/// not. The pinned direct-Rust FIFO exposes a public close operation that
/// wakes the drain and detaches only this exact observer.
#[derive(Debug)]
struct ReceiptDrainHandle {
    receipt_id: WriteReceiptId,
    control: Arc<ReceiptDeliveryControl>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl ReceiptObservation for ReceiptDrainHandle {
    fn receipt_id(&self) -> &WriteReceiptId {
        &self.receipt_id
    }

    fn stop_delivery(&self) {
        self.control.stop();
        if let Some(worker) = self.worker.lock().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ReceiptDrainHandle {
    fn drop(&mut self) {
        self.control.stop();
        if let Some(worker) = self.worker.get_mut().take() {
            let _ = worker.join();
        }
    }
}

#[derive(Default)]
struct ReceiptDeliveryControl {
    stopped: AtomicBool,
    current: Mutex<Option<Arc<FifoReceiver<WriteStatus>>>>,
}

impl fmt::Debug for ReceiptDeliveryControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReceiptDeliveryControl")
            .field("stopped", &self.stopped.load(Ordering::Acquire))
            .field("receiver_attached", &self.current.lock().is_some())
            .finish()
    }
}

impl ReceiptDeliveryControl {
    fn install(&self, receiver: Arc<FifoReceiver<WriteStatus>>) {
        if self.stopped.load(Ordering::Acquire) {
            receiver.close();
            return;
        }
        *self.current.lock() = Some(receiver);
        if self.stopped.load(Ordering::Acquire) {
            if let Some(receiver) = self.current.lock().take() {
                receiver.close();
            }
        }
    }

    fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            if let Some(receiver) = self.current.lock().take() {
                receiver.close();
            }
        }
    }
}

#[derive(Debug)]
struct WorkerAdmission {
    active: AtomicUsize,
    maximum: usize,
}

impl WorkerAdmission {
    fn reserve(self: &Arc<Self>, component: &'static str) -> Result<WorkerPermit, HostDataError> {
        let reserved = self
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.maximum).then_some(active + 1)
            });
        if reserved.is_err() {
            return Err(HostDataError::ExecutorSaturated {
                component: Arc::from(component),
                capacity: self.maximum,
            });
        }
        Ok(WorkerPermit {
            admission: Arc::clone(self),
        })
    }
}

#[derive(Debug)]
struct WorkerPermit {
    admission: Arc<WorkerAdmission>,
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.admission.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionParameters {
    #[serde(default)]
    kinds: Vec<u16>,
    #[serde(default)]
    authors: Vec<String>,
    since: Option<u64>,
    until: Option<u64>,
}

fn collection_query(request: &BindingRequest) -> Result<LiveQuery, HostDataError> {
    if request.family.as_ref() != EVENT_COLLECTION_FAMILY
        || request.schema.as_ref() != EVENT_COLLECTION_SCHEMA
    {
        return Err(HostDataError::BindingRefused {
            reason: Arc::from(format!(
                "unsupported binding family/schema: {}/{}",
                request.family, request.schema
            )),
        });
    }
    let parameters: CollectionParameters = serde_json::from_str(request.parameters.as_str())
        .map_err(|error| HostDataError::BindingRefused {
            reason: Arc::from(format!("invalid event collection parameters: {error}")),
        })?;
    for author in &parameters.authors {
        if author.len() != 64
            || author
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(HostDataError::BindingRefused {
                reason: Arc::from("authors must be lowercase 32-byte hex public keys"),
            });
        }
    }
    let filter = Filter {
        kinds: (!parameters.kinds.is_empty()).then(|| parameters.kinds.into_iter().collect()),
        authors: (!parameters.authors.is_empty())
            .then(|| Binding::Literal(parameters.authors.into_iter().collect())),
        since: parameters.since,
        until: parameters.until,
        ..Filter::default()
    };
    Ok(LiveQuery(Demand::from_filter(filter)))
}

fn project_frame(
    source_generation: u64,
    frame: nmp::Frame,
    maximum_frame_bytes: usize,
) -> Result<HostBindingSnapshot, String> {
    let window = frame
        .window
        .ok_or_else(|| "NMP adapter requires a bounded window frame".to_owned())?;
    let rows = window
        .rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "event": row.event,
                "sources": row.sources.iter().map(ToString::to_string).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let load = match window.load {
        WindowLoad::Idle => serde_json::json!({"state": "idle"}),
        WindowLoad::Requesting => serde_json::json!({"state": "requesting"}),
        WindowLoad::Returned { added } => {
            serde_json::json!({"state": "returned", "added": added})
        }
        WindowLoad::AtBound { max } => {
            serde_json::json!({"state": "at_bound", "max": max})
        }
        _ => serde_json::json!({"state": "unknown_future"}),
    };
    let value_json = serde_json::json!({
        "schema": EVENT_COLLECTION_SCHEMA,
        "rows": rows,
        "windowLoad": load,
    });
    let evidence_sources = frame
        .evidence
        .sources
        .iter()
        .map(|source| {
            serde_json::json!({
                "relay": source.relay.to_string(),
                "access": format!("{:?}", source.access),
                "reconciledThrough": source.reconciled_through.map(|value| value.as_secs()),
                "status": source_status_name(source.status),
            })
        })
        .collect::<Vec<_>>();
    let shortfall = frame
        .evidence
        .shortfall
        .iter()
        .map(shortfall_json)
        .collect::<Vec<_>>();
    let evidence_json = serde_json::json!({
        "sources": evidence_sources,
        "shortfall": shortfall,
    });
    let value_raw = serde_json::to_string(&value_json).map_err(|error| error.to_string())?;
    let evidence_raw = serde_json::to_string(&evidence_json).map_err(|error| error.to_string())?;
    let combined = value_raw.len().saturating_add(evidence_raw.len());
    if combined > maximum_frame_bytes {
        return Err(format!(
            "NMP binding frame is {combined} bytes; negotiated maximum is {maximum_frame_bytes}"
        ));
    }
    let value =
        BoundedJson::from_raw(value_raw, maximum_frame_bytes).map_err(|error| error.to_string())?;
    let scoped_evidence = BoundedJson::from_raw(evidence_raw, maximum_frame_bytes)
        .map_err(|error| error.to_string())?;
    Ok(HostBindingSnapshot {
        source_generation,
        value,
        scoped_evidence,
    })
}

fn source_status_name(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Requesting => "requesting",
        SourceStatus::Connecting => "connecting",
        SourceStatus::Disconnected => "disconnected",
        SourceStatus::AwaitingAuth { .. } => "awaiting_auth",
        SourceStatus::AuthDenied => "auth_denied",
        SourceStatus::Error => "error",
    }
}

fn shortfall_json(shortfall: &ShortfallFact) -> serde_json::Value {
    match shortfall {
        ShortfallFact::NoPlannedSource { atom } => serde_json::json!({
            "kind": "no_planned_source",
            "atom": format!("{atom:?}"),
        }),
        ShortfallFact::NoResolvedDemand => {
            serde_json::json!({"kind": "no_resolved_demand"})
        }
        ShortfallFact::LocalLimit { atom } => serde_json::json!({
            "kind": "local_limit",
            "atom": format!("{atom:?}"),
        }),
    }
}

fn approved_write_intent(approved: &ApprovedWrite) -> Result<WriteIntent, HostDataError> {
    let unsigned: nmp::UnsignedEvent =
        serde_json::from_str(approved.draft.as_str()).map_err(|error| {
            HostDataError::WriteRefused {
                reason: Arc::from(format!("invalid approved unsigned event: {error}")),
            }
        })?;
    let account =
        nmp::PublicKey::from_str(&approved.account.0).map_err(|_| HostDataError::WriteRefused {
            reason: Arc::from("approved account is not a valid Nostr public key"),
        })?;
    if unsigned.pubkey != account {
        return Err(HostDataError::WriteRefused {
            reason: Arc::from("approved draft author does not match the frozen account"),
        });
    }
    let correlation =
        nmp::CorrelationToken::try_from(approved.approval_id.as_ref()).map_err(|error| {
            HostDataError::WriteRefused {
                reason: Arc::from(format!("invalid approval correlation token: {error}")),
            }
        })?;
    Ok(WriteIntent {
        payload: WritePayload::Unsigned(unsigned),
        durability: Durability::Durable,
        routing: WriteRouting::AuthorOutbox,
        identity_override: Some(account),
        correlation: Some(correlation),
    })
}

const MAX_RECEIPT_RELAYS: usize = 64;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptProjection {
    schema: &'static str,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    frozen_pubkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conflict: Option<ReceiptConflict>,
    relays: BTreeMap<String, RelayProjection>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptConflict {
    expected: Option<String>,
    actual: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayProjection {
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    reason_truncated: bool,
    terminal: bool,
}

impl Default for ReceiptProjection {
    fn default() -> Self {
        Self {
            schema: "nostr.write.receipt/1",
            state: "observing",
            frozen_pubkey: None,
            event_id: None,
            failure: None,
            conflict: None,
            relays: BTreeMap::new(),
        }
    }
}

impl ReceiptProjection {
    fn apply(&mut self, status: &WriteStatus) -> Result<BoundedJson, String> {
        match status {
            WriteStatus::Accepted => self.state = "accepted",
            WriteStatus::Cancelled => self.state = "cancelled",
            WriteStatus::AwaitingCapability { pubkey } => {
                self.state = "awaiting_capability";
                self.frozen_pubkey = Some(pubkey.to_string());
            }
            WriteStatus::Signed(event_id) => {
                self.state = "signed";
                self.event_id = Some(event_id.to_string());
            }
            WriteStatus::Routed(relays) => {
                if relays.len() > MAX_RECEIPT_RELAYS {
                    return Err(format!(
                        "receipt routed to {} relays; the projection maximum is {MAX_RECEIPT_RELAYS}",
                        relays.len()
                    ));
                }
                self.state = "delivering";
                for relay in relays {
                    self.relay(relay.to_string())?;
                }
            }
            WriteStatus::AwaitingRelay { relay } => {
                self.set_relay(relay.to_string(), "awaiting_relay", None, None, None, false)?;
            }
            WriteStatus::AwaitingAuth { relay } => {
                self.set_relay(relay.to_string(), "awaiting_auth", None, None, None, false)?;
            }
            WriteStatus::RetryEligible {
                relay,
                attempt,
                eligible_at,
            } => {
                self.set_relay(
                    relay.to_string(),
                    "retry_eligible",
                    Some(*attempt),
                    Some(eligible_at.as_secs()),
                    None,
                    false,
                )?;
            }
            WriteStatus::HandoffAmbiguous {
                relay,
                attempt,
                observed_at,
            } => {
                self.set_relay(
                    relay.to_string(),
                    "handoff_ambiguous",
                    Some(*attempt),
                    Some(observed_at.as_secs()),
                    None,
                    false,
                )?;
            }
            WriteStatus::Sent {
                relay,
                attempt,
                written_at,
            } => {
                self.set_relay(
                    relay.to_string(),
                    "sent",
                    Some(*attempt),
                    Some(written_at.as_secs()),
                    None,
                    false,
                )?;
            }
            WriteStatus::Acked(relay) => {
                self.set_relay(relay.to_string(), "acked", None, None, None, true)?;
            }
            WriteStatus::Rejected(relay, reason) => {
                let (reason, truncated) = bounded_text(reason, 1_024);
                self.set_relay(
                    relay.to_string(),
                    "rejected",
                    None,
                    None,
                    Some(reason),
                    true,
                )?;
                if let Some(lane) = self.relays.get_mut(&relay.to_string()) {
                    lane.reason_truncated = truncated;
                }
            }
            WriteStatus::GaveUp(relay) => {
                self.set_relay(relay.to_string(), "gave_up", None, None, None, true)?;
            }
            WriteStatus::PersistenceBlocked(relay) => {
                self.set_relay(
                    relay.to_string(),
                    "persistence_blocked",
                    None,
                    None,
                    None,
                    false,
                )?;
            }
            WriteStatus::RoutePersistenceBlocked(relay) => {
                self.set_relay(
                    relay.to_string(),
                    "route_persistence_blocked",
                    None,
                    None,
                    None,
                    false,
                )?;
            }
            WriteStatus::OutcomeUnknown(relay) => {
                self.set_relay(relay.to_string(), "outcome_unknown", None, None, None, true)?;
            }
            WriteStatus::ReplaceableConflict { expected, actual } => {
                self.state = "replaceable_conflict";
                self.conflict = Some(ReceiptConflict {
                    expected: expected.map(|value| value.to_string()),
                    actual: actual.map(|value| value.to_string()),
                });
            }
            WriteStatus::Failed(reason) => {
                self.state = "failed";
                self.failure = Some(bounded_text(reason, 1_024).0);
            }
        }
        self.recompute_delivery_state();
        receipt_projection_json(self)
    }

    fn relay(&mut self, relay: String) -> Result<&mut RelayProjection, String> {
        if !self.relays.contains_key(&relay) && self.relays.len() >= MAX_RECEIPT_RELAYS {
            return Err(format!(
                "receipt exceeds the projection maximum of {MAX_RECEIPT_RELAYS} relays"
            ));
        }
        Ok(self.relays.entry(relay).or_insert(RelayProjection {
            state: "routed",
            attempt: None,
            observed_at: None,
            reason: None,
            reason_truncated: false,
            terminal: false,
        }))
    }

    fn set_relay(
        &mut self,
        relay: String,
        state: &'static str,
        attempt: Option<u64>,
        observed_at: Option<u64>,
        reason: Option<String>,
        terminal: bool,
    ) -> Result<(), String> {
        let lane = self.relay(relay)?;
        *lane = RelayProjection {
            state,
            attempt,
            observed_at,
            reason,
            reason_truncated: false,
            terminal,
        };
        Ok(())
    }

    fn recompute_delivery_state(&mut self) {
        if self.relays.is_empty()
            || matches!(
                self.state,
                "cancelled"
                    | "failed"
                    | "replaceable_conflict"
                    | "awaiting_capability"
                    | "accepted"
                    | "signed"
            )
        {
            return;
        }
        if self.relays.values().all(|relay| relay.terminal) {
            let acknowledgements = self
                .relays
                .values()
                .filter(|relay| relay.state == "acked")
                .count();
            self.state = if acknowledgements == self.relays.len() {
                "delivered"
            } else if acknowledgements > 0 {
                "partial_delivery"
            } else {
                "exhausted"
            };
        } else {
            self.state = "delivering";
        }
    }
}

fn receipt_projection_json(projection: &ReceiptProjection) -> Result<BoundedJson, String> {
    const MAX_RECEIPT_STATUS_BYTES: usize = 16 * 1_024;
    let value = serde_json::to_value(projection).map_err(|error| error.to_string())?;
    BoundedJson::from_value(&value, MAX_RECEIPT_STATUS_BYTES).map_err(|error| error.to_string())
}

fn bounded_text(value: &str, maximum_bytes: usize) -> (String, bool) {
    if value.len() <= maximum_bytes {
        return (value.to_owned(), false);
    }
    let mut boundary = maximum_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (value[..boundary].to_owned(), true)
}

fn map_open_engine_error(error: EngineError) -> HostDataError {
    map_engine_error_with(error, |reason| HostDataError::BindingRefused { reason })
}

fn map_binding_engine_error(error: EngineError) -> HostDataError {
    map_engine_error_with(error, |reason| HostDataError::BindingRefused { reason })
}

fn map_write_engine_error(error: EngineError) -> HostDataError {
    map_engine_error_with(error, |reason| HostDataError::WriteRefused { reason })
}

fn map_receipt_engine_error(error: EngineError) -> HostDataError {
    map_engine_error_with(error, |reason| HostDataError::ReceiptUnreadable { reason })
}

fn map_identity_engine_error(error: EngineError) -> PublicIdentityError {
    match error {
        EngineError::EngineClosed => PublicIdentityError::Closed,
        other => PublicIdentityError::Failed {
            reason: Arc::from(other.to_string()),
        },
    }
}

fn public_identity_query_name(query: &PublicIdentityQuery) -> &'static str {
    match query {
        PublicIdentityQuery::Relays => "relays",
        PublicIdentityQuery::Profile => "profile",
        PublicIdentityQuery::Follows => "follows",
        PublicIdentityQuery::List { .. } => "list",
        PublicIdentityQuery::Zaps => "zaps",
        PublicIdentityQuery::Mutes => "mutes",
        PublicIdentityQuery::Blocked => "blocked",
        PublicIdentityQuery::Badges => "badges",
    }
}

fn map_engine_error_with(
    error: EngineError,
    contextual: impl FnOnce(Arc<str>) -> HostDataError,
) -> HostDataError {
    match error {
        EngineError::ThreadUnavailable { component, reason } => HostDataError::ThreadUnavailable {
            component: Arc::from(component),
            reason: Arc::from(reason),
        },
        EngineError::EngineClosed => HostDataError::ServiceClosed,
        other => contextual(Arc::from(other.to_string())),
    }
}

fn drain_receipt_pages(
    engine: Arc<Engine>,
    raw_receipt_id: ReceiptId,
    statuses: FifoReceiver<WriteStatus>,
    mut next_cursor: Option<ReceiptReplayCursor>,
    receipt_id: WriteReceiptId,
    sink: Arc<dyn ReceiptEventSink>,
    control: Option<Arc<ReceiptDeliveryControl>>,
) {
    let mut statuses = Arc::new(statuses);
    let mut projection = ReceiptProjection::default();
    if let Some(control) = &control {
        control.install(Arc::clone(&statuses));
    }
    loop {
        while let Ok(status) = statuses.recv() {
            let state = match projection.apply(&status) {
                Ok(state) => state,
                Err(reason) => {
                    sink.close(Some(Arc::from(reason)));
                    return;
                }
            };
            match sink.push_latest(ReceiptSnapshot {
                receipt_id: receipt_id.clone(),
                state,
            }) {
                Ok(()) => {}
                Err(ReceiptSinkError::Closed) => return,
                Err(ReceiptSinkError::FrameTooLarge) => {
                    sink.close(Some(Arc::from(
                        "receipt status exceeded the negotiated frame bound",
                    )));
                    return;
                }
            }
        }

        if control
            .as_ref()
            .is_some_and(|control| control.stopped.load(Ordering::Acquire))
        {
            sink.close(None);
            return;
        }
        let Some(cursor) = next_cursor.take() else {
            sink.close(None);
            return;
        };
        match engine.reattach_receipt_from(raw_receipt_id, cursor) {
            Ok(NmpReceiptReattachment::Attached {
                statuses: next_statuses,
                next_cursor: following_cursor,
                ..
            }) => {
                statuses = Arc::new(next_statuses);
                if let Some(control) = &control {
                    control.install(Arc::clone(&statuses));
                }
                next_cursor = following_cursor;
            }
            Ok(NmpReceiptReattachment::NotFound) => {
                sink.close(Some(Arc::from(
                    "receipt disappeared while continuing bounded replay",
                )));
                return;
            }
            Ok(NmpReceiptReattachment::RetainedButUnreadable) => {
                sink.close(Some(Arc::from(
                    "receipt became unreadable while continuing bounded replay",
                )));
                return;
            }
            Err(error) => {
                sink.close(Some(Arc::from(error.to_string())));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };

    use parking_lot::Condvar;

    use super::*;

    fn request(parameters: serde_json::Value) -> BindingRequest {
        BindingRequest {
            workspace_binding_id: Arc::from("feed"),
            family: Arc::from(EVENT_COLLECTION_FAMILY),
            schema: Arc::from(EVENT_COLLECTION_SCHEMA),
            parameters: BoundedJson::from_value(&parameters, 1024).unwrap(),
            maximum_rows: 40,
            maximum_frame_bytes: 256 * 1024,
        }
    }

    #[test]
    fn event_collection_query_is_window_compatible() {
        let query = collection_query(&request(serde_json::json!({
            "kinds": [1],
            "authors": ["ab".repeat(32)],
        })))
        .unwrap();
        assert_eq!(query.0.selection.kinds, Some(BTreeSet::from([1])));
        assert_eq!(query.0.selection.limit, None);
    }

    #[test]
    fn malformed_author_is_refused_before_observation() {
        let error = collection_query(&request(serde_json::json!({
            "authors": ["not-a-key"],
        })))
        .unwrap_err();
        assert!(matches!(error, HostDataError::BindingRefused { .. }));
    }

    #[test]
    fn worker_admission_has_zero_queue() {
        let admission = Arc::new(WorkerAdmission {
            active: AtomicUsize::new(0),
            maximum: 1,
        });
        let permit = admission.reserve("test").unwrap();
        assert!(matches!(
            admission.reserve("test"),
            Err(HostDataError::ExecutorSaturated { capacity: 1, .. })
        ));
        drop(permit);
        assert!(admission.reserve("test").is_ok());
    }

    #[test]
    fn receipt_delivery_stop_wakes_blocked_fifo_without_cancelling_a_write() {
        let (_producer, receiver) = nmp::fifo_channel::<WriteStatus>();
        let receiver = Arc::new(receiver);
        let control = Arc::new(ReceiptDeliveryControl::default());
        control.install(Arc::clone(&receiver));
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let closed = receiver.recv().is_err();
            done_tx.send(closed).unwrap();
        });

        control.stop();
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("public FIFO close should wake the exact observer")
        );
        worker.join().unwrap();
    }

    #[test]
    fn receipt_projection_preserves_mixed_per_relay_evidence() {
        let first = nmp::RelayUrl::parse("wss://first.example").unwrap();
        let second = nmp::RelayUrl::parse("wss://second.example").unwrap();
        let mut projection = ReceiptProjection::default();

        projection
            .apply(&WriteStatus::Routed(BTreeSet::from([
                first.clone(),
                second.clone(),
            ])))
            .unwrap();
        projection
            .apply(&WriteStatus::Acked(first.clone()))
            .unwrap();
        let state = projection
            .apply(&WriteStatus::Rejected(second.clone(), "policy".to_owned()))
            .unwrap()
            .decode()
            .unwrap();

        assert_eq!(state["state"], "partial_delivery");
        assert_eq!(state["relays"][first.to_string()]["state"], "acked");
        assert_eq!(state["relays"][second.to_string()]["state"], "rejected");
        assert_eq!(state["relays"][second.to_string()]["reason"], "policy");
    }

    #[derive(Debug, Default)]
    struct LatestBindingSink {
        latest: Mutex<Option<HostBindingSnapshot>>,
        changed: Condvar,
        closed: AtomicBool,
    }

    impl BindingEventSink for LatestBindingSink {
        fn push_latest(&self, snapshot: HostBindingSnapshot) -> Result<(), BindingSinkError> {
            if self.closed.load(Ordering::Acquire) {
                return Err(BindingSinkError::Closed);
            }
            *self.latest.lock() = Some(snapshot);
            self.changed.notify_all();
            Ok(())
        }

        fn close(&self, _reason: Option<Arc<str>>) {
            self.closed.store(true, Ordering::Release);
            self.changed.notify_all();
        }
    }

    impl LatestBindingSink {
        fn wait_for_snapshot(&self, deadline: Instant) -> Option<HostBindingSnapshot> {
            let mut latest = self.latest.lock();
            while latest.is_none() && !self.closed.load(Ordering::Acquire) {
                let now = Instant::now();
                if now >= deadline {
                    return None;
                }
                self.changed
                    .wait_for(&mut latest, deadline.saturating_duration_since(now));
            }
            latest.take()
        }
    }

    #[test]
    fn bounded_nmp_binding_delivers_honest_scoped_evidence_and_tears_down() {
        let plane = NmpDataPlane::open(EngineConfig::default(), 2).unwrap();
        let sink = Arc::new(LatestBindingSink::default());
        let handle = plane
            .open_binding(request(serde_json::json!({"kinds": [1]})), sink.clone())
            .unwrap();

        let snapshot = sink
            .wait_for_snapshot(Instant::now() + Duration::from_secs(2))
            .expect("in-memory NMP observation should emit its initial bounded frame");
        let evidence = snapshot.scoped_evidence.as_str();
        assert!(evidence.contains("shortfall"));
        assert!(!evidence.contains("synced"));
        assert!(!evidence.contains("complete"));

        handle.close();
        assert_eq!(plane.active_bridge_workers(), 0);
        plane.close();
    }
}
