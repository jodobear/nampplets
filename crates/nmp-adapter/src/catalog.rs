//! Bounded NMP-backed napplet manifest discovery.
//!
//! This module never owns relay selection, canonical Nostr rows, replacement
//! semantics, or a second event cache. It opens finite observations through
//! the pinned [`nmp::Engine`] facade and projects each delivered frame into
//! screen-shaped catalog candidates plus source-scoped evidence.
//!
//! NIP-50 is not available through the pinned facade. Browse search is
//! therefore an explicitly local filter over the current finite NMP window,
//! never a claim that the network was searched exhaustively.

use std::{
    collections::BTreeSet,
    fmt,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

use nmp::{
    AccessContext, AcquisitionEvidence, Binding, Demand, Engine, Filter, IndexedTagName, LiveQuery,
    ObservationCancel, Row, ShortfallFact, SourceAuthority, SourceStatus, Window, WindowLoad,
};
use nmp_native_artifact::ManifestCoordinate;
use nmp_native_catalog_resolver::{
    CoordinateLookupFact, LookupPortError, ManifestLookupCompletion, ManifestLookupOperation,
    ManifestLookupPort, ManifestLookupRequest, ManifestLookupResponse,
};
use thiserror::Error;

const MANIFEST_SNAPSHOT_KIND: u16 = 5_129;
const MANIFEST_ROOT_KIND: u16 = 15_129;
const MANIFEST_NAMED_KIND: u16 = 35_129;
const EXACT_WINDOW_ROWS: usize = 2;
const ABSOLUTE_MAXIMUM_SEARCH_BYTES: usize = 256;

/// Finite catalog-query limits. Values are policy, not relay protocol facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManifestCatalogLimits {
    pub maximum_in_flight_lookups: usize,
    pub maximum_browse_observations: usize,
    pub browse_initial_rows: usize,
    pub browse_maximum_rows: usize,
    pub maximum_projected_rows: usize,
    pub maximum_search_bytes: usize,
    pub maximum_event_bytes: usize,
    pub maximum_tags_per_event: usize,
    pub maximum_tag_fields: usize,
    pub maximum_tag_string_bytes: usize,
    pub maximum_sources_per_row: usize,
    pub maximum_source_label_bytes: usize,
    pub maximum_evidence_sources: usize,
    pub maximum_evidence_shortfalls: usize,
}

impl Default for ManifestCatalogLimits {
    fn default() -> Self {
        Self {
            maximum_in_flight_lookups: 4,
            maximum_browse_observations: 4,
            browse_initial_rows: 50,
            browse_maximum_rows: 512,
            maximum_projected_rows: 100,
            maximum_search_bytes: 256,
            maximum_event_bytes: 256 * 1_024,
            maximum_tags_per_event: 1_024,
            maximum_tag_fields: 64,
            maximum_tag_string_bytes: 16 * 1_024,
            maximum_sources_per_row: 32,
            maximum_source_label_bytes: 256,
            maximum_evidence_sources: 64,
            maximum_evidence_shortfalls: 64,
        }
    }
}

impl ManifestCatalogLimits {
    fn validate(self) -> Result<Self, ManifestCatalogError> {
        if self.maximum_in_flight_lookups == 0
            || self.maximum_browse_observations == 0
            || self.browse_initial_rows == 0
            || self.browse_maximum_rows == 0
            || self.maximum_projected_rows == 0
            || self.maximum_search_bytes == 0
            || self.maximum_event_bytes == 0
            || self.maximum_tags_per_event == 0
            || self.maximum_tag_fields == 0
            || self.maximum_tag_string_bytes == 0
            || self.maximum_sources_per_row == 0
            || self.maximum_source_label_bytes == 0
            || self.maximum_evidence_sources == 0
            || self.maximum_evidence_shortfalls == 0
            || self.browse_initial_rows > self.browse_maximum_rows
            || self.maximum_projected_rows > self.browse_maximum_rows
            || self.maximum_search_bytes > ABSOLUTE_MAXIMUM_SEARCH_BYTES
        {
            return Err(ManifestCatalogError::InvalidLimits);
        }
        Ok(self)
    }
}

/// One local text filter over a bounded browse window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogBrowseRequest {
    search: Option<Arc<str>>,
}

impl CatalogBrowseRequest {
    pub fn new(search: Option<&str>) -> Result<Self, ManifestCatalogError> {
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        if let Some(search) = search
            && search.len() > ABSOLUTE_MAXIMUM_SEARCH_BYTES
        {
            return Err(ManifestCatalogError::SearchTooLarge {
                actual: search.len(),
                maximum: ABSOLUTE_MAXIMUM_SEARCH_BYTES,
            });
        }
        Ok(Self {
            search: search.map(Arc::from),
        })
    }

    pub fn search(&self) -> Option<&str> {
        self.search.as_deref()
    }
}

/// A candidate from the current canonical NMP window.
///
/// The event remains a candidate until the artifact verifier validates its
/// signature, exact coordinate, manifest schema, and aggregate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogManifestCandidate {
    pub event_id: Arc<str>,
    pub author: Arc<str>,
    pub kind: u16,
    pub created_at: u64,
    pub d_tag: Option<Arc<str>>,
    pub title: Option<Arc<str>>,
    pub description: Option<Arc<str>>,
    pub aggregate: Option<Arc<str>>,
    pub event_json: Arc<[u8]>,
    pub observed_sources: Arc<[Arc<str>]>,
}

/// Why one untrusted row was kept out of the projected catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogCandidateRefusal {
    pub event_id: Arc<str>,
    pub reason: CatalogCandidateRefusalReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogCandidateRefusalReason {
    EventTooLarge,
    TooManyTags,
    TooManyTagFields,
    TagStringTooLarge,
    TooManyObservedSources,
    SourceLabelTooLarge,
    SerializationFailed,
}

/// Current source-scoped acquisition evidence for the browse demand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogSourceEvidence {
    pub relay: Arc<str>,
    pub access: CatalogAccessContext,
    pub reconciled_through: Option<u64>,
    pub status: CatalogSourceStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogAccessContext {
    Public,
    Nip42 { public_key: Arc<str> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogSourceStatus {
    Requesting,
    Connecting,
    Disconnected,
    AwaitingAuth,
    AuthDenied,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogShortfall {
    NoPlannedSource,
    NoResolvedDemand,
    LocalLimit,
}

/// One self-contained, finite browse update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogBrowseFrame {
    pub candidates: Arc<[CatalogManifestCandidate]>,
    pub refused: Arc<[CatalogCandidateRefusal]>,
    pub locally_filtered_rows: usize,
    pub projection_limit_rows: usize,
    pub source_evidence: Arc<[CatalogSourceEvidence]>,
    pub shortfalls: Arc<[CatalogShortfall]>,
    pub window_load: WindowLoad,
}

impl CatalogBrowseFrame {
    /// Applies a local text filter to the current finite NMP replacement.
    ///
    /// This does not open another relay subscription and makes no NIP-50 or
    /// network-completeness claim. The permanent profile observation remains
    /// the sole source of catalog rows.
    pub fn filtered(&self, request: &CatalogBrowseRequest) -> Self {
        let Some(needle) = request.search().map(str::to_lowercase) else {
            return self.clone();
        };
        let mut filtered_rows = self.locally_filtered_rows;
        let candidates = self
            .candidates
            .iter()
            .filter(|candidate| {
                let matches = candidate_matches(candidate, &needle);
                if !matches {
                    filtered_rows = filtered_rows.saturating_add(1);
                }
                matches
            })
            .cloned()
            .collect::<Vec<_>>();
        Self {
            candidates: candidates.into(),
            refused: Arc::clone(&self.refused),
            locally_filtered_rows: filtered_rows,
            projection_limit_rows: self.projection_limit_rows,
            source_evidence: Arc::clone(&self.source_evidence),
            shortfalls: Arc::clone(&self.shortfalls),
            window_load: self.window_load,
        }
    }
}

/// Typed catalog-query refusal. None of these values imply a global network
/// result.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ManifestCatalogError {
    #[error("manifest catalog limits are invalid")]
    InvalidLimits,
    #[error("catalog search is {actual} bytes; the maximum is {maximum}")]
    SearchTooLarge { actual: usize, maximum: usize },
    #[error("the NMP engine refused the catalog observation: {reason}")]
    ObservationRefused { reason: Arc<str> },
    #[error("the catalog observation ended")]
    ObservationEnded,
    #[error("catalog evidence has {actual} entries; the maximum is {maximum}")]
    EvidenceCapacity { actual: usize, maximum: usize },
    #[error("catalog source label is {actual} bytes; the maximum is {maximum}")]
    SourceLabelTooLarge { actual: usize, maximum: usize },
    #[error("the exact manifest lookup limit is full at {maximum} operations")]
    LookupCapacity { maximum: usize },
    #[error("the catalog browse observation limit is full at {maximum} observations")]
    BrowseCapacity { maximum: usize },
    #[error("the catalog lookup worker could not start: {reason}")]
    WorkerUnavailable { reason: Arc<str> },
    #[error("NMP returned more than one canonical row for an exact manifest coordinate")]
    ExactCoordinateInvariant,
}

/// NMP-backed catalog entry point for one profile-owned engine.
#[derive(Clone)]
pub struct NmpManifestCatalog {
    engine: Arc<Engine>,
    limits: ManifestCatalogLimits,
    lookup_admission: Arc<LookupAdmission>,
    browse_admission: Arc<BrowseAdmission>,
}

impl fmt::Debug for NmpManifestCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NmpManifestCatalog")
            .field("limits", &self.limits)
            .field(
                "active_lookups",
                &self.lookup_admission.active.load(Ordering::Acquire),
            )
            .field(
                "active_browse_observations",
                &self.browse_admission.active.load(Ordering::Acquire),
            )
            .finish_non_exhaustive()
    }
}

impl NmpManifestCatalog {
    pub fn new(
        engine: Arc<Engine>,
        limits: ManifestCatalogLimits,
    ) -> Result<Self, ManifestCatalogError> {
        let limits = limits.validate()?;
        Ok(Self {
            engine,
            limits,
            lookup_admission: Arc::new(LookupAdmission {
                active: AtomicUsize::new(0),
                maximum: limits.maximum_in_flight_lookups,
            }),
            browse_admission: Arc::new(BrowseAdmission {
                active: AtomicUsize::new(0),
                maximum: limits.maximum_browse_observations,
            }),
        })
    }

    pub fn active_exact_lookups(&self) -> usize {
        self.lookup_admission.active.load(Ordering::Acquire)
    }

    pub fn active_browse_observations(&self) -> usize {
        self.browse_admission.active.load(Ordering::Acquire)
    }

    /// Observe manifest kinds on the engine's configured public source plan.
    ///
    /// Search remains local to the current finite window. Call
    /// [`CatalogBrowseObservation::request_rows`] to grow that same
    /// observation monotonically up to `browse_maximum_rows`.
    pub fn observe_browse(
        &self,
        request: CatalogBrowseRequest,
    ) -> Result<CatalogBrowseObservation, ManifestCatalogError> {
        if let Some(search) = request.search()
            && search.len() > self.limits.maximum_search_bytes
        {
            return Err(ManifestCatalogError::SearchTooLarge {
                actual: search.len(),
                maximum: self.limits.maximum_search_bytes,
            });
        }
        let permit = self.browse_admission.try_acquire()?;
        let initial = NonZeroUsize::new(self.limits.browse_initial_rows)
            .expect("validated catalog limits are non-zero");
        let max = NonZeroUsize::new(self.limits.browse_maximum_rows)
            .expect("validated catalog limits are non-zero");
        let subscription = self
            .engine
            .observe(
                broad_manifest_query(),
                Some(Window::Expandable { initial, max }),
            )
            .map_err(|error| ManifestCatalogError::ObservationRefused {
                reason: Arc::from(error.to_string()),
            })?;
        let cancel = CatalogBrowseCancel {
            inner: subscription.cancel_handle(),
            lease: permit.lease(),
        };
        Ok(CatalogBrowseObservation {
            subscription,
            cancel,
            request,
            limits: self.limits,
            _permit: permit,
        })
    }
}

/// One live finite catalog observation. It owns no row cache: every return is
/// projected directly from NMP's current authoritative window frame.
pub struct CatalogBrowseObservation {
    subscription: nmp::Subscription,
    cancel: CatalogBrowseCancel,
    request: CatalogBrowseRequest,
    limits: ManifestCatalogLimits,
    _permit: BrowsePermit,
}

impl fmt::Debug for CatalogBrowseObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CatalogBrowseObservation")
            .field("request", &self.request)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl CatalogBrowseObservation {
    /// Block for the next event-driven NMP frame. No timer or polling loop is
    /// involved; cancelling from another owner wakes the blocked receiver.
    pub fn recv(&self) -> Result<CatalogBrowseFrame, ManifestCatalogError> {
        let frame = self
            .subscription
            .recv()
            .map_err(|_| ManifestCatalogError::ObservationEnded)?;
        let window = frame
            .window
            .expect("this observation was constructed with a finite window");
        project_browse_frame(
            &window.rows,
            &frame.evidence,
            window.load,
            &self.request,
            self.limits,
        )
    }

    pub fn request_rows(&self, at_least: usize) -> Result<(), ManifestCatalogError> {
        self.subscription.request_rows(at_least).map_err(|error| {
            ManifestCatalogError::ObservationRefused {
                reason: Arc::from(error.to_string()),
            }
        })
    }

    pub fn cancel_handle(&self) -> CatalogBrowseCancel {
        self.cancel.clone()
    }

    pub fn cancel(self) {}
}

/// Cloneable, idempotent cancellation for a blocking browse receiver.
#[derive(Clone)]
pub struct CatalogBrowseCancel {
    inner: ObservationCancel,
    lease: Arc<BrowseLease>,
}

impl fmt::Debug for CatalogBrowseCancel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CatalogBrowseCancel")
            .finish_non_exhaustive()
    }
}

impl CatalogBrowseCancel {
    pub fn cancel(&self) {
        self.inner.cancel();
        self.lease.release();
    }
}

#[derive(Debug)]
struct LookupAdmission {
    active: AtomicUsize,
    maximum: usize,
}

#[derive(Debug)]
struct BrowseAdmission {
    active: AtomicUsize,
    maximum: usize,
}

impl BrowseAdmission {
    fn try_acquire(self: &Arc<Self>) -> Result<BrowsePermit, ManifestCatalogError> {
        let mut active = self.active.load(Ordering::Acquire);
        loop {
            if active >= self.maximum {
                return Err(ManifestCatalogError::BrowseCapacity {
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
                    return Ok(BrowsePermit {
                        lease: Arc::new(BrowseLease {
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
struct BrowsePermit {
    lease: Arc<BrowseLease>,
}

impl BrowsePermit {
    fn lease(&self) -> Arc<BrowseLease> {
        Arc::clone(&self.lease)
    }
}

impl Drop for BrowsePermit {
    fn drop(&mut self) {
        self.lease.release();
    }
}

#[derive(Debug)]
struct BrowseLease {
    admission: Arc<BrowseAdmission>,
    active: AtomicBool,
}

impl BrowseLease {
    fn release(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.admission.active.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl LookupAdmission {
    fn try_acquire(self: &Arc<Self>) -> Result<LookupPermit, ManifestCatalogError> {
        let mut active = self.active.load(Ordering::Acquire);
        loop {
            if active >= self.maximum {
                return Err(ManifestCatalogError::LookupCapacity {
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
                    return Ok(LookupPermit {
                        admission: Arc::clone(self),
                    });
                }
                Err(observed) => active = observed,
            }
        }
    }
}

#[derive(Debug)]
struct LookupPermit {
    admission: Arc<LookupAdmission>,
}

impl Drop for LookupPermit {
    fn drop(&mut self) {
        self.admission.active.fetch_sub(1, Ordering::AcqRel);
    }
}

struct NmpExactLookupOperation {
    cancel: ObservationCancel,
    cancelled: Arc<AtomicBool>,
}

impl fmt::Debug for NmpExactLookupOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NmpExactLookupOperation")
            .field("cancelled", &self.cancelled.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl ManifestLookupOperation for NmpExactLookupOperation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel.cancel();
    }
}

impl Drop for NmpExactLookupOperation {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel.cancel();
    }
}

impl ManifestLookupPort for NmpManifestCatalog {
    fn start_lookup(
        &self,
        request: ManifestLookupRequest,
        completion: ManifestLookupCompletion,
    ) -> Result<Arc<dyn ManifestLookupOperation>, LookupPortError> {
        let permit = self
            .lookup_admission
            .try_acquire()
            .map_err(|error| LookupPortError::new(error.to_string()))?;
        let subscription = self
            .engine
            .observe(
                exact_manifest_query(request.coordinate()),
                Some(Window::Expandable {
                    initial: NonZeroUsize::new(EXACT_WINDOW_ROWS)
                        .expect("the exact window is non-zero"),
                    max: NonZeroUsize::new(EXACT_WINDOW_ROWS)
                        .expect("the exact window is non-zero"),
                }),
            )
            .map_err(|error| LookupPortError::new(error.to_string()))?;
        let cancel = subscription.cancel_handle();
        let cancelled = Arc::new(AtomicBool::new(false));
        let operation: Arc<dyn ManifestLookupOperation> = Arc::new(NmpExactLookupOperation {
            cancel: cancel.clone(),
            cancelled: Arc::clone(&cancelled),
        });
        let limits = self.limits;

        let spawn = thread::Builder::new()
            .name("nmp-manifest-lookup".to_owned())
            .spawn(move || {
                let _permit = permit;
                let result = drain_exact_lookup(&subscription, &request, &cancelled, limits);
                let _ = completion.resolve(result);
            });
        if let Err(error) = spawn {
            cancel.cancel();
            return Err(LookupPortError::new(
                ManifestCatalogError::WorkerUnavailable {
                    reason: Arc::from(error.to_string()),
                }
                .to_string(),
            ));
        }
        Ok(operation)
    }
}

fn broad_manifest_query() -> LiveQuery {
    let selection = Filter {
        kinds: Some(BTreeSet::from([
            MANIFEST_SNAPSHOT_KIND,
            MANIFEST_ROOT_KIND,
            MANIFEST_NAMED_KIND,
        ])),
        ..Filter::default()
    };
    LiveQuery(
        Demand::new(selection, SourceAuthority::Public, AccessContext::Public)
            .expect("an authorless public manifest query is valid"),
    )
}

fn exact_manifest_query(coordinate: &ManifestCoordinate) -> LiveQuery {
    let mut selection = Filter::default();
    match coordinate {
        ManifestCoordinate::Snapshot { event_id, author } => {
            selection.kinds = Some(BTreeSet::from([MANIFEST_SNAPSHOT_KIND]));
            selection.authors = Some(Binding::Literal(BTreeSet::from([author
                .as_str()
                .to_owned()])));
            selection.ids = Some(Binding::Literal(BTreeSet::from([event_id
                .as_str()
                .to_owned()])));
        }
        ManifestCoordinate::Root { author } => {
            selection.kinds = Some(BTreeSet::from([MANIFEST_ROOT_KIND]));
            selection.authors = Some(Binding::Literal(BTreeSet::from([author
                .as_str()
                .to_owned()])));
        }
        ManifestCoordinate::Named { author, d_tag } => {
            selection.kinds = Some(BTreeSet::from([MANIFEST_NAMED_KIND]));
            selection.authors = Some(Binding::Literal(BTreeSet::from([author
                .as_str()
                .to_owned()])));
            selection.tags.insert(
                IndexedTagName::new('d').expect("d is an indexed NIP-01 tag"),
                Binding::Literal(BTreeSet::from([d_tag.to_string()])),
            );
        }
    }
    LiveQuery(
        Demand::new(selection, SourceAuthority::Public, AccessContext::Public)
            .expect("an exact public manifest query is valid"),
    )
}

fn drain_exact_lookup(
    subscription: &nmp::Subscription,
    request: &ManifestLookupRequest,
    cancelled: &AtomicBool,
    limits: ManifestCatalogLimits,
) -> Result<ManifestLookupResponse, LookupPortError> {
    loop {
        let frame = subscription.recv().map_err(|_| {
            if cancelled.load(Ordering::Acquire) {
                LookupPortError::new("manifest lookup was cancelled")
            } else {
                LookupPortError::new("NMP manifest observation ended before a scoped result")
            }
        })?;
        let window = frame.window.ok_or_else(|| {
            LookupPortError::new("NMP exact manifest observation lost its finite window")
        })?;
        if window.rows.len() > 1 {
            return Err(LookupPortError::new(
                ManifestCatalogError::ExactCoordinateInvariant.to_string(),
            ));
        }
        if exact_frame_is_ready(&window.rows, &frame.evidence) {
            return exact_lookup_response(
                &window.rows,
                &frame.evidence,
                request.maximum_event_bytes(),
                request.maximum_facts(),
                limits.maximum_source_label_bytes,
            );
        }
    }
}

fn exact_frame_is_ready(rows: &[Row], evidence: &AcquisitionEvidence) -> bool {
    // NMP owns canonical replacement selection. A sole delivered row is a
    // usable exact-coordinate result immediately, even while some configured
    // sources are still requesting. The accompanying facts remain scoped to
    // this frame and never claim the row is globally latest.
    if !rows.is_empty() {
        return true;
    }
    if evidence.sources.is_empty() {
        return !evidence.shortfall.is_empty();
    }
    evidence.sources.iter().all(|source| {
        source.reconciled_through.is_some()
            || matches!(
                source.status,
                SourceStatus::AuthDenied | SourceStatus::Error
            )
    })
}

fn exact_lookup_response(
    rows: &[Row],
    evidence: &AcquisitionEvidence,
    maximum_event_bytes: usize,
    maximum_facts: usize,
    maximum_source_label_bytes: usize,
) -> Result<ManifestLookupResponse, LookupPortError> {
    if rows.len() > 1 {
        return Err(LookupPortError::new(
            ManifestCatalogError::ExactCoordinateInvariant.to_string(),
        ));
    }
    let facts = resolver_lookup_facts(rows, evidence, maximum_facts, maximum_source_label_bytes)?;
    let Some(row) = rows.first() else {
        return Ok(ManifestLookupResponse::not_found(facts));
    };
    let event_json = serde_json::to_vec(&row.event)
        .map_err(|error| LookupPortError::new(format!("serialize selected manifest: {error}")))?;
    if event_json.len() > maximum_event_bytes {
        return Err(LookupPortError::new(format!(
            "selected manifest is {} bytes; the maximum is {maximum_event_bytes}",
            event_json.len()
        )));
    }
    Ok(ManifestLookupResponse::found(event_json, facts))
}

fn resolver_lookup_facts(
    rows: &[Row],
    evidence: &AcquisitionEvidence,
    maximum_facts: usize,
    maximum_source_label_bytes: usize,
) -> Result<Vec<CoordinateLookupFact>, LookupPortError> {
    let actual = evidence
        .sources
        .len()
        .saturating_add(evidence.shortfall.len())
        .saturating_add(usize::from(!rows.is_empty()));
    if actual > maximum_facts {
        return Err(LookupPortError::new(format!(
            "manifest lookup evidence has {actual} facts; the maximum is {maximum_facts}"
        )));
    }
    let mut facts = Vec::with_capacity(actual);
    for source in &evidence.sources {
        let source_label = source.relay.to_string();
        if source_label.len() > maximum_source_label_bytes {
            return Err(LookupPortError::new(format!(
                "manifest lookup source label is {} bytes; the maximum is {maximum_source_label_bytes}",
                source_label.len()
            )));
        }
        let observed_rows = rows
            .iter()
            .filter(|row| row.sources.contains(&source.relay))
            .count();
        match source.status {
            SourceStatus::AuthDenied => facts.push(CoordinateLookupFact::shortfall(
                source_label,
                "the exact public source denied authentication",
            )),
            SourceStatus::Error => facts.push(CoordinateLookupFact::shortfall(
                source_label,
                "the exact public source reported an acquisition error",
            )),
            SourceStatus::Requesting
            | SourceStatus::Connecting
            | SourceStatus::Disconnected
            | SourceStatus::AwaitingAuth { .. } => {
                facts.push(CoordinateLookupFact::observed(source_label, observed_rows))
            }
        }
    }
    for shortfall in &evidence.shortfall {
        facts.push(CoordinateLookupFact::shortfall(
            "nmp-public-demand",
            shortfall_label(shortfall),
        ));
    }
    if let Some(row) = rows.first() {
        facts.push(CoordinateLookupFact::selected(
            "nmp-canonical-row",
            row.event.id.to_hex(),
        ));
    }
    Ok(facts)
}

fn project_browse_frame(
    rows: &[Row],
    evidence: &AcquisitionEvidence,
    window_load: WindowLoad,
    request: &CatalogBrowseRequest,
    limits: ManifestCatalogLimits,
) -> Result<CatalogBrowseFrame, ManifestCatalogError> {
    let source_evidence = project_evidence(evidence, limits)?;
    let search = request.search().map(|value| value.to_lowercase());
    let mut candidates = Vec::with_capacity(rows.len().min(limits.maximum_projected_rows));
    let mut refused = Vec::new();
    let mut locally_filtered_rows = 0usize;
    let mut projection_limit_rows = 0usize;

    for row in rows {
        match project_candidate(row, limits) {
            Ok(candidate) => {
                if search
                    .as_deref()
                    .is_some_and(|needle| !candidate_matches(&candidate, needle))
                {
                    locally_filtered_rows = locally_filtered_rows.saturating_add(1);
                    continue;
                }
                if candidates.len() < limits.maximum_projected_rows {
                    candidates.push(candidate);
                } else {
                    projection_limit_rows = projection_limit_rows.saturating_add(1);
                }
            }
            Err(reason) => refused.push(CatalogCandidateRefusal {
                event_id: Arc::from(row.event.id.to_hex()),
                reason,
            }),
        }
    }

    Ok(CatalogBrowseFrame {
        candidates: candidates.into(),
        refused: refused.into(),
        locally_filtered_rows,
        projection_limit_rows,
        source_evidence: source_evidence.0.into(),
        shortfalls: source_evidence.1.into(),
        window_load,
    })
}

fn project_candidate(
    row: &Row,
    limits: ManifestCatalogLimits,
) -> Result<CatalogManifestCandidate, CatalogCandidateRefusalReason> {
    if row.event.tags.len() > limits.maximum_tags_per_event {
        return Err(CatalogCandidateRefusalReason::TooManyTags);
    }
    for tag in row.event.tags.iter() {
        let fields = tag.as_slice();
        if fields.len() > limits.maximum_tag_fields {
            return Err(CatalogCandidateRefusalReason::TooManyTagFields);
        }
        if fields
            .iter()
            .any(|field| field.len() > limits.maximum_tag_string_bytes)
        {
            return Err(CatalogCandidateRefusalReason::TagStringTooLarge);
        }
    }
    if row.sources.len() > limits.maximum_sources_per_row {
        return Err(CatalogCandidateRefusalReason::TooManyObservedSources);
    }
    if row
        .sources
        .iter()
        .any(|source| source.to_string().len() > limits.maximum_source_label_bytes)
    {
        return Err(CatalogCandidateRefusalReason::SourceLabelTooLarge);
    }
    let event_json = serde_json::to_vec(&row.event)
        .map_err(|_| CatalogCandidateRefusalReason::SerializationFailed)?;
    if event_json.len() > limits.maximum_event_bytes {
        return Err(CatalogCandidateRefusalReason::EventTooLarge);
    }

    let d_tag = first_tag_value(row, "d").map(Arc::from);
    let title = first_tag_value(row, "title").map(Arc::from);
    let description = first_tag_value(row, "description").map(Arc::from);
    let aggregate = row.event.tags.iter().find_map(|tag| {
        let fields = tag.as_slice();
        (fields.len() == 3 && fields[0] == "x" && fields[2] == "aggregate")
            .then(|| Arc::<str>::from(fields[1].as_str()))
    });
    let observed_sources = row
        .sources
        .iter()
        .map(|source| Arc::<str>::from(source.to_string()))
        .collect::<Vec<_>>();

    Ok(CatalogManifestCandidate {
        event_id: Arc::from(row.event.id.to_hex()),
        author: Arc::from(row.event.pubkey.to_hex()),
        kind: row.event.kind.as_u16(),
        created_at: row.event.created_at.as_secs(),
        d_tag,
        title,
        description,
        aggregate,
        event_json: event_json.into(),
        observed_sources: observed_sources.into(),
    })
}

fn first_tag_value<'a>(row: &'a Row, name: &str) -> Option<&'a str> {
    row.event.tags.iter().find_map(|tag| {
        let fields = tag.as_slice();
        (fields.len() == 2 && fields[0] == name).then(|| fields[1].as_str())
    })
}

fn candidate_matches(candidate: &CatalogManifestCandidate, needle: &str) -> bool {
    [
        Some(candidate.event_id.as_ref()),
        Some(candidate.author.as_ref()),
        candidate.d_tag.as_deref(),
        candidate.title.as_deref(),
        candidate.description.as_deref(),
        candidate.aggregate.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.to_lowercase().contains(needle))
}

fn project_evidence(
    evidence: &AcquisitionEvidence,
    limits: ManifestCatalogLimits,
) -> Result<(Vec<CatalogSourceEvidence>, Vec<CatalogShortfall>), ManifestCatalogError> {
    if evidence.sources.len() > limits.maximum_evidence_sources {
        return Err(ManifestCatalogError::EvidenceCapacity {
            actual: evidence.sources.len(),
            maximum: limits.maximum_evidence_sources,
        });
    }
    if evidence.shortfall.len() > limits.maximum_evidence_shortfalls {
        return Err(ManifestCatalogError::EvidenceCapacity {
            actual: evidence.shortfall.len(),
            maximum: limits.maximum_evidence_shortfalls,
        });
    }
    if let Some(actual) = evidence
        .sources
        .iter()
        .map(|source| source.relay.to_string().len())
        .find(|actual| *actual > limits.maximum_source_label_bytes)
    {
        return Err(ManifestCatalogError::SourceLabelTooLarge {
            actual,
            maximum: limits.maximum_source_label_bytes,
        });
    }
    let sources = evidence
        .sources
        .iter()
        .map(|source| CatalogSourceEvidence {
            relay: Arc::from(source.relay.to_string()),
            access: match source.access {
                AccessContext::Public => CatalogAccessContext::Public,
                AccessContext::Nip42(public_key) => CatalogAccessContext::Nip42 {
                    public_key: Arc::from(public_key.to_hex()),
                },
            },
            reconciled_through: source
                .reconciled_through
                .map(|timestamp| timestamp.as_secs()),
            status: match source.status {
                SourceStatus::Requesting => CatalogSourceStatus::Requesting,
                SourceStatus::Connecting => CatalogSourceStatus::Connecting,
                SourceStatus::Disconnected => CatalogSourceStatus::Disconnected,
                SourceStatus::AwaitingAuth { .. } => CatalogSourceStatus::AwaitingAuth,
                SourceStatus::AuthDenied => CatalogSourceStatus::AuthDenied,
                SourceStatus::Error => CatalogSourceStatus::Error,
            },
        })
        .collect();
    let shortfalls = evidence
        .shortfall
        .iter()
        .map(|shortfall| match shortfall {
            ShortfallFact::NoPlannedSource { .. } => CatalogShortfall::NoPlannedSource,
            ShortfallFact::NoResolvedDemand => CatalogShortfall::NoResolvedDemand,
            ShortfallFact::LocalLimit { .. } => CatalogShortfall::LocalLimit,
        })
        .collect();
    Ok((sources, shortfalls))
}

fn shortfall_label(shortfall: &ShortfallFact) -> &'static str {
    match shortfall {
        ShortfallFact::NoPlannedSource { .. } => "no planned source for exact public demand",
        ShortfallFact::NoResolvedDemand => "exact public demand did not resolve",
        ShortfallFact::LocalLimit { .. } => "a local NMP relay limit narrowed the public demand",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nmp::{RelayUrl, SourceEvidence, Timestamp};
    use nmp_native_catalog_resolver::CoordinateLookupState;

    const PUBLISHED_EVENT: &[u8] =
        include_bytes!("../../../conformance/napplet-corpus/published/good-morning/event.json");
    const PUBLISHED_AUTHOR: &str =
        "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5";
    const PUBLISHED_ID: &str = "b330bfaefd2ddf268ebe4196403e6163533c54f41dabc3518bdc1a896c68f40e";

    fn fixture_row() -> Row {
        Row {
            event: serde_json::from_slice(PUBLISHED_EVENT).unwrap(),
            sources: BTreeSet::new(),
        }
    }

    fn pending_source_evidence() -> AcquisitionEvidence {
        AcquisitionEvidence {
            sources: vec![SourceEvidence {
                relay: RelayUrl::parse("wss://relay.example.com").unwrap(),
                access: AccessContext::Public,
                reconciled_through: None,
                status: SourceStatus::Connecting,
            }],
            shortfall: Vec::new(),
        }
    }

    #[test]
    fn broad_query_is_public_bounded_and_has_only_manifest_kinds() {
        let LiveQuery(demand) = broad_manifest_query();
        assert_eq!(demand.source, SourceAuthority::Public);
        assert_eq!(demand.access, AccessContext::Public);
        assert_eq!(
            demand.selection.kinds,
            Some(BTreeSet::from([
                MANIFEST_SNAPSHOT_KIND,
                MANIFEST_ROOT_KIND,
                MANIFEST_NAMED_KIND
            ]))
        );
        assert_eq!(demand.selection.limit, None);
    }

    #[test]
    fn exact_queries_bind_every_coordinate_component_without_a_filter_limit() {
        let snapshot = ManifestCoordinate::snapshot(PUBLISHED_ID, PUBLISHED_AUTHOR).unwrap();
        let LiveQuery(snapshot_demand) = exact_manifest_query(&snapshot);
        assert_eq!(
            snapshot_demand.selection.kinds,
            Some(BTreeSet::from([MANIFEST_SNAPSHOT_KIND]))
        );
        assert_eq!(
            snapshot_demand.selection.ids,
            Some(Binding::Literal(BTreeSet::from([PUBLISHED_ID.to_owned()])))
        );

        let root = ManifestCoordinate::root(PUBLISHED_AUTHOR).unwrap();
        let LiveQuery(root_demand) = exact_manifest_query(&root);
        assert_eq!(
            root_demand.selection.kinds,
            Some(BTreeSet::from([MANIFEST_ROOT_KIND]))
        );
        assert_eq!(root_demand.selection.ids, None);

        let named = ManifestCoordinate::named(PUBLISHED_AUTHOR, "good-morning").unwrap();
        let LiveQuery(named_demand) = exact_manifest_query(&named);
        assert_eq!(
            named_demand
                .selection
                .tags
                .get(&IndexedTagName::new('d').expect("d is an indexed NIP-01 tag")),
            Some(&Binding::Literal(BTreeSet::from([
                "good-morning".to_owned()
            ])))
        );
        assert_eq!(named_demand.selection.limit, None);
        assert_eq!(named_demand.source, SourceAuthority::Public);
    }

    #[test]
    fn local_search_filters_only_the_delivered_bounded_window() {
        let rows = vec![fixture_row()];
        let frame = project_browse_frame(
            &rows,
            &AcquisitionEvidence::default(),
            WindowLoad::AtBound { max: 512 },
            &CatalogBrowseRequest::new(Some("morning protocol")).unwrap(),
            ManifestCatalogLimits::default(),
        )
        .unwrap();
        assert_eq!(frame.candidates.len(), 1);
        assert_eq!(frame.locally_filtered_rows, 0);

        let filtered = project_browse_frame(
            &rows,
            &AcquisitionEvidence::default(),
            WindowLoad::AtBound { max: 512 },
            &CatalogBrowseRequest::new(Some("not present")).unwrap(),
            ManifestCatalogLimits::default(),
        )
        .unwrap();
        assert!(filtered.candidates.is_empty());
        assert_eq!(filtered.locally_filtered_rows, 1);
    }

    #[test]
    fn exact_selection_refuses_more_than_one_canonical_row() {
        let rows = vec![fixture_row(), fixture_row()];
        let error = exact_lookup_response(&rows, &AcquisitionEvidence::default(), 300_000, 8, 256)
            .unwrap_err();
        assert!(
            error.to_string().contains("more than one canonical row"),
            "{error}"
        );
    }

    #[test]
    fn replaceable_row_returns_while_a_source_requests_but_true_miss_waits_for_evidence() {
        let rows = vec![fixture_row()];
        let mut evidence = pending_source_evidence();
        evidence.sources[0].status = SourceStatus::Requesting;
        assert!(
            exact_frame_is_ready(&rows, &evidence),
            "NMP's sole canonical replaceable row must not wait for every source"
        );
        assert!(
            !exact_frame_is_ready(&[], &evidence),
            "a true miss still needs scoped reconciliation or terminal evidence"
        );

        evidence.sources[0].reconciled_through = Some(Timestamp::from_secs(1));
        assert!(exact_frame_is_ready(&[], &evidence));

        evidence.sources[0].reconciled_through = None;
        evidence.sources[0].status = SourceStatus::Error;
        assert!(exact_frame_is_ready(&[], &evidence));
        let facts = resolver_lookup_facts(&[], &evidence, 8, 256).unwrap();
        assert!(matches!(
            facts[0].state(),
            CoordinateLookupState::Shortfall { reason }
                if reason.contains("acquisition error")
        ));
    }

    #[test]
    fn malformed_candidate_is_refused_without_hiding_valid_rows() {
        let valid = fixture_row();
        let mut oversized = fixture_row();
        oversized.event.content = "x".repeat(512);
        let limits = ManifestCatalogLimits {
            maximum_event_bytes: PUBLISHED_EVENT.len() + 8,
            ..ManifestCatalogLimits::default()
        };
        let frame = project_browse_frame(
            &[valid, oversized],
            &AcquisitionEvidence::default(),
            WindowLoad::AtBound { max: 2 },
            &CatalogBrowseRequest::default(),
            limits,
        )
        .unwrap();
        assert_eq!(frame.candidates.len(), 1);
        assert_eq!(
            frame.refused[0].reason,
            CatalogCandidateRefusalReason::EventTooLarge
        );
    }

    #[test]
    fn all_capacities_are_explicit_and_nonzero() {
        assert_eq!(
            ManifestCatalogLimits {
                maximum_evidence_sources: 0,
                ..ManifestCatalogLimits::default()
            }
            .validate(),
            Err(ManifestCatalogError::InvalidLimits)
        );
        assert_eq!(
            ManifestCatalogLimits {
                browse_initial_rows: 513,
                browse_maximum_rows: 512,
                ..ManifestCatalogLimits::default()
            }
            .validate(),
            Err(ManifestCatalogError::InvalidLimits)
        );
        assert_eq!(
            CatalogBrowseRequest::new(Some(&"x".repeat(ABSOLUTE_MAXIMUM_SEARCH_BYTES + 1))),
            Err(ManifestCatalogError::SearchTooLarge {
                actual: ABSOLUTE_MAXIMUM_SEARCH_BYTES + 1,
                maximum: ABSOLUTE_MAXIMUM_SEARCH_BYTES
            })
        );

        let browse = Arc::new(BrowseAdmission {
            active: AtomicUsize::new(0),
            maximum: 1,
        });
        let first = browse.try_acquire().unwrap();
        assert_eq!(
            browse.try_acquire().unwrap_err(),
            ManifestCatalogError::BrowseCapacity { maximum: 1 }
        );
        first.lease().release();
        assert_eq!(browse.active.load(Ordering::Acquire), 0);
        let replacement = browse.try_acquire().unwrap();
        drop(first);
        assert_eq!(
            browse.active.load(Ordering::Acquire),
            1,
            "dropping a previously released lease cannot release its replacement"
        );
        drop(replacement);

        let lookup = Arc::new(LookupAdmission {
            active: AtomicUsize::new(0),
            maximum: 1,
        });
        let first = lookup.try_acquire().unwrap();
        assert_eq!(
            lookup.try_acquire().unwrap_err(),
            ManifestCatalogError::LookupCapacity { maximum: 1 }
        );
        drop(first);
        lookup.try_acquire().unwrap();
    }
}
