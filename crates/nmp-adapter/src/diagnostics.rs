//! Bounded passthrough of NMP's engine-global diagnostics read-out.
//!
//! NMP owns relay planning, wire subscription accounting, event counting, and
//! coverage proof. This module opens one facade diagnostics observation and
//! reshapes each delivered snapshot into finite screen rows. Nothing here is
//! recomputed, estimated, cached, or joined against another source.
//!
//! Absence stays absence: a filter without a coverage row is unproven, never
//! zero and never complete.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use nmp::{
    AccessContext, DiagnosticsSnapshot, Engine, Lane, ObservationCancel, RelayDiagnosticsSnapshot,
};
use thiserror::Error;

const ABSOLUTE_MAXIMUM_RELAYS: usize = 256;

/// Finite diagnostics projection limits. Values are presentation policy, not
/// relay protocol facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayDiagnosticsLimits {
    pub maximum_observations: usize,
    pub maximum_relays: usize,
    pub maximum_subscriptions_per_relay: usize,
    pub maximum_lanes_per_relay: usize,
    pub maximum_kinds_per_relay: usize,
    pub maximum_supported_nips: usize,
    pub maximum_dropped_merge_rules: usize,
    pub maximum_string_bytes: usize,
    pub maximum_filter_bytes: usize,
}

impl Default for RelayDiagnosticsLimits {
    fn default() -> Self {
        Self {
            maximum_observations: 2,
            maximum_relays: 64,
            maximum_subscriptions_per_relay: 64,
            maximum_lanes_per_relay: 16,
            maximum_kinds_per_relay: 64,
            maximum_supported_nips: 128,
            maximum_dropped_merge_rules: 32,
            maximum_string_bytes: 1_024,
            maximum_filter_bytes: 8 * 1_024,
        }
    }
}

impl RelayDiagnosticsLimits {
    fn validate(self) -> Result<Self, RelayDiagnosticsError> {
        if self.maximum_observations == 0
            || self.maximum_relays == 0
            || self.maximum_subscriptions_per_relay == 0
            || self.maximum_lanes_per_relay == 0
            || self.maximum_kinds_per_relay == 0
            || self.maximum_supported_nips == 0
            || self.maximum_dropped_merge_rules == 0
            || self.maximum_string_bytes == 0
            || self.maximum_filter_bytes == 0
            || self.maximum_relays > ABSOLUTE_MAXIMUM_RELAYS
        {
            return Err(RelayDiagnosticsError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Frozen access identity of one physical relay session. One relay planned
/// under several contexts yields several rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticsAccessContext {
    Public,
    Nip42 { public_key: Arc<str> },
}

/// NMP's closed routing-lane vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticsLane {
    Nip65Write,
    Nip65Read,
    Hint,
    Provenance,
    UserConfigured,
    IndexerDiscovery,
    GroupHost,
    DmInbox,
    AppRelay,
    Fallback,
    ExplicitPinned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneSubscriptionCount {
    pub lane: DiagnosticsLane,
    pub wire_subscriptions: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindEventCount {
    pub kind: u16,
    pub events: u64,
}

/// A proven `[from, through]` interval for one exact filter shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterCoverageWindow {
    pub from_seconds: u64,
    pub through_seconds: u64,
}

/// One currently active wire subscription filter, as sent to the relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireSubscriptionView {
    /// The exact wire JSON NMP sent. It is never re-encoded or derived here.
    pub filter: Arc<str>,
    /// `None` means the relay has no proven row for this filter's shape.
    pub coverage: Option<FilterCoverageWindow>,
}

/// One physical relay session's current diagnostics row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayDiagnosticsView {
    pub relay: Arc<str>,
    pub access: DiagnosticsAccessContext,
    pub wire_subscription_count: usize,
    pub authors_served: usize,
    pub lanes: Arc<[LaneSubscriptionCount]>,
    pub omitted_lanes: usize,
    pub subscriptions: Arc<[WireSubscriptionView]>,
    pub omitted_subscriptions: usize,
    pub events_by_kind: Arc<[KindEventCount]>,
    pub omitted_kinds: usize,
    pub nip11_supported_nips: Option<Arc<[u16]>>,
    pub omitted_supported_nips: usize,
    pub nip11_document_revision: Option<Arc<str>>,
    pub nip11_freshness: Option<Arc<str>>,
    pub nip11_last_error: Option<Arc<str>>,
    pub nip77_advertisement: Arc<str>,
    pub nip77_behavior: Arc<str>,
    pub nip77_handoff: Arc<str>,
}

/// One self-contained engine-global diagnostics update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayDiagnosticsFrame {
    pub relays: Arc<[RelayDiagnosticsView]>,
    pub omitted_relays: usize,
    pub uncovered_author_count: usize,
    pub dropped_merge_rules: Arc<[Arc<str>]>,
    pub omitted_dropped_merge_rules: usize,
    pub discovered_private_relays_rejected: u64,
    pub sessions_rejected_over_cap: u64,
    pub store_degraded: Option<Arc<str>>,
    pub transport_degraded: Option<Arc<str>>,
}

/// Typed diagnostics refusal. None of these values imply a relay or network
/// result.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RelayDiagnosticsError {
    #[error("relay diagnostics limits are invalid")]
    InvalidLimits,
    #[error("the NMP engine refused the diagnostics observation: {reason}")]
    ObservationRefused { reason: Arc<str> },
    #[error("the diagnostics observation ended")]
    ObservationEnded,
    #[error("the diagnostics observation limit is full at {maximum} observations")]
    ObservationCapacity { maximum: usize },
}

/// NMP-backed diagnostics entry point for one profile-owned engine. Clones
/// share the same bounded observation admission domain.
#[derive(Clone)]
pub struct NmpRelayDiagnostics {
    engine: Arc<Engine>,
    limits: RelayDiagnosticsLimits,
    admission: Arc<DiagnosticsAdmission>,
}

impl fmt::Debug for NmpRelayDiagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NmpRelayDiagnostics")
            .field("limits", &self.limits)
            .field(
                "active_observations",
                &self.admission.active.load(Ordering::Acquire),
            )
            .finish_non_exhaustive()
    }
}

impl NmpRelayDiagnostics {
    pub fn new(
        engine: Arc<Engine>,
        limits: RelayDiagnosticsLimits,
    ) -> Result<Self, RelayDiagnosticsError> {
        let limits = limits.validate()?;
        Ok(Self {
            engine,
            limits,
            admission: Arc::new(DiagnosticsAdmission {
                active: AtomicUsize::new(0),
                maximum: limits.maximum_observations,
            }),
        })
    }

    pub fn active_observations(&self) -> usize {
        self.admission.active.load(Ordering::Acquire)
    }

    /// Open one live diagnostics stream through the pinned NMP facade.
    pub fn observe(&self) -> Result<RelayDiagnosticsObservation, RelayDiagnosticsError> {
        let permit = self.admission.try_acquire()?;
        let subscription = self.engine.observe_diagnostics().map_err(|error| {
            RelayDiagnosticsError::ObservationRefused {
                reason: Arc::from(error.to_string()),
            }
        })?;
        let cancel = RelayDiagnosticsCancel {
            inner: subscription.cancel_handle(),
            lease: permit.lease(),
        };
        Ok(RelayDiagnosticsObservation {
            subscription,
            cancel,
            limits: self.limits,
            _permit: permit,
        })
    }
}

/// One live diagnostics observation. It owns no snapshot cache: every return
/// is projected directly from NMP's current delivery.
pub struct RelayDiagnosticsObservation {
    subscription: nmp::DiagnosticsSubscription,
    cancel: RelayDiagnosticsCancel,
    limits: RelayDiagnosticsLimits,
    _permit: DiagnosticsPermit,
}

impl fmt::Debug for RelayDiagnosticsObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayDiagnosticsObservation")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl RelayDiagnosticsObservation {
    /// Block for the next event-driven NMP snapshot: the current one on the
    /// first call, then one per coverage change. No timer or polling loop is
    /// involved; cancelling from another owner wakes the blocked receiver.
    pub fn recv(&self) -> Result<RelayDiagnosticsFrame, RelayDiagnosticsError> {
        let snapshot = self
            .subscription
            .recv()
            .ok_or(RelayDiagnosticsError::ObservationEnded)?;
        Ok(project_snapshot(&snapshot, self.limits))
    }

    pub fn cancel_handle(&self) -> RelayDiagnosticsCancel {
        self.cancel.clone()
    }

    pub fn cancel(self) {}
}

/// Cloneable, idempotent cancellation for a blocking diagnostics receiver.
#[derive(Clone)]
pub struct RelayDiagnosticsCancel {
    inner: ObservationCancel,
    lease: Arc<DiagnosticsLease>,
}

impl fmt::Debug for RelayDiagnosticsCancel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayDiagnosticsCancel")
            .finish_non_exhaustive()
    }
}

impl RelayDiagnosticsCancel {
    pub fn cancel(&self) {
        self.inner.cancel();
        self.lease.release();
    }
}

fn project_snapshot(
    snapshot: &DiagnosticsSnapshot,
    limits: RelayDiagnosticsLimits,
) -> RelayDiagnosticsFrame {
    let mut relays = Vec::new();
    let mut omitted_relays = 0_usize;
    for row in snapshot.relays.iter() {
        if relays.len() >= limits.maximum_relays {
            omitted_relays = omitted_relays.saturating_add(1);
            continue;
        }
        match project_relay(row, limits) {
            Some(view) => relays.push(view),
            None => omitted_relays = omitted_relays.saturating_add(1),
        }
    }

    let mut dropped_merge_rules = Vec::new();
    let mut omitted_dropped_merge_rules = 0_usize;
    for rule in snapshot.dropped_merge_rules.iter() {
        if dropped_merge_rules.len() >= limits.maximum_dropped_merge_rules
            || rule.len() > limits.maximum_string_bytes
        {
            omitted_dropped_merge_rules = omitted_dropped_merge_rules.saturating_add(1);
            continue;
        }
        dropped_merge_rules.push(Arc::from(*rule));
    }

    RelayDiagnosticsFrame {
        relays: relays.into(),
        omitted_relays,
        uncovered_author_count: snapshot.uncovered_author_count,
        dropped_merge_rules: dropped_merge_rules.into(),
        omitted_dropped_merge_rules,
        discovered_private_relays_rejected: snapshot.discovered_private_relays_rejected,
        sessions_rejected_over_cap: snapshot.sessions_rejected_over_cap,
        store_degraded: bounded_optional(snapshot.store_degraded.as_deref(), limits),
        transport_degraded: bounded_optional(snapshot.transport_degraded.as_deref(), limits),
    }
}

/// Returns `None` for a row whose fixed strings exceed the negotiated bound;
/// the caller counts it as omitted rather than presenting a truncated relay
/// identity or a truncated NIP-11 citation.
fn project_relay(
    row: &RelayDiagnosticsSnapshot,
    limits: RelayDiagnosticsLimits,
) -> Option<RelayDiagnosticsView> {
    let relay = bounded(&row.relay.to_string(), limits)?;
    let access = match &row.access {
        AccessContext::Public => DiagnosticsAccessContext::Public,
        AccessContext::Nip42(public_key) => DiagnosticsAccessContext::Nip42 {
            public_key: bounded(&public_key.to_string(), limits)?,
        },
    };
    let nip11_document_revision = optional_bounded(row.nip11_document_revision.as_deref(), limits)?;
    let nip11_freshness = optional_bounded(row.nip11_freshness, limits)?;
    let nip11_last_error = optional_bounded(row.nip11_last_error.as_deref(), limits)?;
    let nip77_advertisement = bounded(row.nip77_advertisement, limits)?;
    let nip77_behavior = bounded(row.nip77_behavior, limits)?;
    let nip77_handoff = bounded(row.nip77_handoff, limits)?;

    let mut lanes = Vec::new();
    let mut omitted_lanes = 0_usize;
    for (lane, count) in row.by_lane.iter() {
        if lanes.len() >= limits.maximum_lanes_per_relay {
            omitted_lanes = omitted_lanes.saturating_add(1);
            continue;
        }
        lanes.push(LaneSubscriptionCount {
            lane: project_lane(*lane),
            wire_subscriptions: *count,
        });
    }

    let mut subscriptions = Vec::new();
    let mut omitted_subscriptions = 0_usize;
    for (index, filter) in row.filters.iter().enumerate() {
        if subscriptions.len() >= limits.maximum_subscriptions_per_relay
            || filter.len() > limits.maximum_filter_bytes
        {
            omitted_subscriptions = omitted_subscriptions.saturating_add(1);
            continue;
        }
        subscriptions.push(WireSubscriptionView {
            filter: Arc::from(filter.as_str()),
            coverage: row
                .coverage
                .get(index)
                .filter(|entry| entry.filter == *filter)
                .and_then(|entry| entry.coverage)
                .map(|interval| FilterCoverageWindow {
                    from_seconds: interval.from.as_secs(),
                    through_seconds: interval.through.as_secs(),
                }),
        });
    }

    let mut events_by_kind = Vec::new();
    let mut omitted_kinds = 0_usize;
    for (kind, events) in row.events_by_kind.iter() {
        if events_by_kind.len() >= limits.maximum_kinds_per_relay {
            omitted_kinds = omitted_kinds.saturating_add(1);
            continue;
        }
        events_by_kind.push(KindEventCount {
            kind: *kind,
            events: *events,
        });
    }

    let mut omitted_supported_nips = 0_usize;
    let nip11_supported_nips = row.nip11_supported_nips.as_ref().map(|nips| {
        let kept = nips.len().min(limits.maximum_supported_nips);
        omitted_supported_nips = nips.len().saturating_sub(kept);
        Arc::from(&nips[..kept])
    });

    Some(RelayDiagnosticsView {
        relay,
        access,
        wire_subscription_count: row.wire_sub_count,
        authors_served: row.authors_served,
        lanes: lanes.into(),
        omitted_lanes,
        subscriptions: subscriptions.into(),
        omitted_subscriptions,
        events_by_kind: events_by_kind.into(),
        omitted_kinds,
        nip11_supported_nips,
        omitted_supported_nips,
        nip11_document_revision,
        nip11_freshness,
        nip11_last_error,
        nip77_advertisement,
        nip77_behavior,
        nip77_handoff,
    })
}

fn project_lane(lane: Lane) -> DiagnosticsLane {
    match lane {
        Lane::Nip65Write => DiagnosticsLane::Nip65Write,
        Lane::Nip65Read => DiagnosticsLane::Nip65Read,
        Lane::Hint => DiagnosticsLane::Hint,
        Lane::Provenance => DiagnosticsLane::Provenance,
        Lane::UserConfigured => DiagnosticsLane::UserConfigured,
        Lane::IndexerDiscovery => DiagnosticsLane::IndexerDiscovery,
        Lane::GroupHost => DiagnosticsLane::GroupHost,
        Lane::DmInbox => DiagnosticsLane::DmInbox,
        Lane::AppRelay => DiagnosticsLane::AppRelay,
        Lane::Fallback => DiagnosticsLane::Fallback,
        Lane::ExplicitPinned => DiagnosticsLane::ExplicitPinned,
    }
}

fn bounded(value: &str, limits: RelayDiagnosticsLimits) -> Option<Arc<str>> {
    (value.len() <= limits.maximum_string_bytes).then(|| Arc::from(value))
}

/// `Ok(None)` for an absent value, `None` for a present but over-long one, so
/// unknown never masquerades as within-bounds.
fn optional_bounded(
    value: Option<&str>,
    limits: RelayDiagnosticsLimits,
) -> Option<Option<Arc<str>>> {
    match value {
        Some(value) => bounded(value, limits).map(Some),
        None => Some(None),
    }
}

fn bounded_optional(value: Option<&str>, limits: RelayDiagnosticsLimits) -> Option<Arc<str>> {
    value.and_then(|value| bounded(value, limits))
}

#[derive(Debug)]
struct DiagnosticsAdmission {
    active: AtomicUsize,
    maximum: usize,
}

impl DiagnosticsAdmission {
    fn try_acquire(self: &Arc<Self>) -> Result<DiagnosticsPermit, RelayDiagnosticsError> {
        let mut active = self.active.load(Ordering::Acquire);
        loop {
            if active >= self.maximum {
                return Err(RelayDiagnosticsError::ObservationCapacity {
                    maximum: self.maximum,
                });
            }
            match self.active.compare_exchange_weak(
                active,
                active + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(DiagnosticsPermit {
                        lease: Arc::new(DiagnosticsLease {
                            admission: Arc::clone(self),
                            active: AtomicBool::new(true),
                        }),
                    });
                }
                Err(observed) => active = observed,
            }
        }
    }
}

#[derive(Debug)]
struct DiagnosticsPermit {
    lease: Arc<DiagnosticsLease>,
}

impl DiagnosticsPermit {
    fn lease(&self) -> Arc<DiagnosticsLease> {
        Arc::clone(&self.lease)
    }
}

impl Drop for DiagnosticsPermit {
    fn drop(&mut self) {
        self.lease.release();
    }
}

#[derive(Debug)]
struct DiagnosticsLease {
    admission: Arc<DiagnosticsAdmission>,
    active: AtomicBool,
}

impl DiagnosticsLease {
    fn release(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.admission.active.fetch_sub(1, Ordering::AcqRel);
        }
    }
}
