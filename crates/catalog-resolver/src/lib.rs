//! Bounded coordinate lookup and policy-checked artifact acquisition.
//!
//! NMP selects one canonical manifest event through [`ManifestLookupPort`].
//! This crate validates finite lookup evidence and raw HTTPS acquisition facts,
//! then delegates all signature, manifest, path-hash, aggregate, and immutable
//! byte handling to `nmp-native-artifact`.

use std::{
    collections::BTreeMap,
    fmt,
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use nmp_native_artifact::{
    ArtifactLimits, ArtifactSourcePolicy, BlobFetchRequest, BlobFetchResponse, BlobSourceError,
    FileArtifactCache, ManifestBlobSource, ManifestCoordinate, ManifestEventVerifier, Sha256Digest,
    SignedArtifactResolver, VerifiedArtifactHandle, VerifiedArtifactIndex,
};
use parking_lot::Mutex;
use thiserror::Error;
use url::{Host, Url};

const KIND_SNAPSHOT: u16 = 5_129;
const KIND_ROOT: u16 = 15_129;
const KIND_NAMED: u16 = 35_129;

/// Finite resolver-wide limits. Every value must be non-zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolverLimits {
    pub maximum_in_flight: usize,
    pub maximum_lookup_facts: usize,
    pub maximum_acquisition_facts: usize,
    pub maximum_manifest_bytes: usize,
    pub maximum_resolved_addresses: usize,
    pub maximum_url_bytes: usize,
    pub maximum_source_label_bytes: usize,
    pub maximum_reason_bytes: usize,
}

impl Default for ResolverLimits {
    fn default() -> Self {
        Self {
            maximum_in_flight: 4,
            maximum_lookup_facts: 64,
            maximum_acquisition_facts: 4_096,
            maximum_manifest_bytes: 256 * 1_024,
            maximum_resolved_addresses: 16,
            maximum_url_bytes: 2_048,
            maximum_source_label_bytes: 256,
            maximum_reason_bytes: 512,
        }
    }
}

impl ResolverLimits {
    fn validate(self) -> Result<Self, ResolveError> {
        if self.maximum_in_flight == 0
            || self.maximum_lookup_facts == 0
            || self.maximum_acquisition_facts == 0
            || self.maximum_manifest_bytes == 0
            || self.maximum_resolved_addresses == 0
            || self.maximum_url_bytes == 0
            || self.maximum_source_label_bytes == 0
            || self.maximum_reason_bytes == 0
        {
            return Err(ResolveError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Cloneable cooperative cancellation shared with injected I/O ports.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug)]
pub struct ManifestLookupRequest {
    coordinate: ManifestCoordinate,
    maximum_event_bytes: usize,
    maximum_facts: usize,
}

impl ManifestLookupRequest {
    pub fn coordinate(&self) -> &ManifestCoordinate {
        &self.coordinate
    }

    pub fn maximum_event_bytes(&self) -> usize {
        self.maximum_event_bytes
    }

    pub fn maximum_facts(&self) -> usize {
        self.maximum_facts
    }
}

/// A source-scoped fact. `Observed` never means globally complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoordinateLookupFact {
    source: Arc<str>,
    state: CoordinateLookupState,
}

impl CoordinateLookupFact {
    pub fn observed(source: impl Into<Arc<str>>, rows: usize) -> Self {
        Self {
            source: source.into(),
            state: CoordinateLookupState::Observed { rows },
        }
    }

    pub fn shortfall(source: impl Into<Arc<str>>, reason: impl Into<Arc<str>>) -> Self {
        Self {
            source: source.into(),
            state: CoordinateLookupState::Shortfall {
                reason: reason.into(),
            },
        }
    }

    pub fn selected(source: impl Into<Arc<str>>, event_id: impl Into<Arc<str>>) -> Self {
        Self {
            source: source.into(),
            state: CoordinateLookupState::Selected {
                event_id: event_id.into(),
            },
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn state(&self) -> &CoordinateLookupState {
        &self.state
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoordinateLookupState {
    Observed { rows: usize },
    Shortfall { reason: Arc<str> },
    Selected { event_id: Arc<str> },
}

#[derive(Clone, Debug)]
pub struct ManifestLookupResponse {
    selected_event_json: Option<Arc<[u8]>>,
    facts: Arc<[CoordinateLookupFact]>,
}

impl ManifestLookupResponse {
    pub fn found(
        selected_event_json: impl Into<Arc<[u8]>>,
        facts: impl Into<Arc<[CoordinateLookupFact]>>,
    ) -> Self {
        Self {
            selected_event_json: Some(selected_event_json.into()),
            facts: facts.into(),
        }
    }

    pub fn not_found(facts: impl Into<Arc<[CoordinateLookupFact]>>) -> Self {
        Self {
            selected_event_json: None,
            facts: facts.into(),
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("manifest lookup port failed: {reason}")]
pub struct LookupPortError {
    reason: Arc<str>,
}

impl LookupPortError {
    pub fn new(reason: impl Into<Arc<str>>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// NMP/public-facade boundary. Implementations return NMP's selected row and
/// scoped evidence; they do not hand relay choice or replacement policy here.
pub trait ManifestLookupPort: Send + Sync + fmt::Debug {
    fn lookup(
        &self,
        request: &ManifestLookupRequest,
        cancellation: &CancellationToken,
    ) -> Result<ManifestLookupResponse, LookupPortError>;
}

#[derive(Clone, Debug)]
pub struct HttpsFetchRequest {
    url: Arc<str>,
    maximum_bytes: usize,
}

impl HttpsFetchRequest {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn maximum_bytes(&self) -> usize {
        self.maximum_bytes
    }

    /// Redirect following is always forbidden for artifact acquisition.
    pub fn follow_redirects(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct HttpsFetchResponse {
    effective_url: Arc<str>,
    status: u16,
    redirect_location: Option<Arc<str>>,
    resolved_addresses: Arc<[IpAddr]>,
    body: Arc<[u8]>,
}

impl HttpsFetchResponse {
    pub fn new(
        effective_url: impl Into<Arc<str>>,
        status: u16,
        redirect_location: Option<Arc<str>>,
        resolved_addresses: impl Into<Arc<[IpAddr]>>,
        body: impl Into<Arc<[u8]>>,
    ) -> Self {
        Self {
            effective_url: effective_url.into(),
            status,
            redirect_location,
            resolved_addresses: resolved_addresses.into(),
            body: body.into(),
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("HTTPS acquisition port failed: {reason}")]
pub struct HttpsPortError {
    reason: Arc<str>,
}

impl HttpsPortError {
    pub fn new(reason: impl Into<Arc<str>>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Raw HTTPS executor. Implementations must disable redirects and cap streaming
/// reads to `maximum_bytes + 1`; Rust validates every returned raw fact.
pub trait HttpsAcquisitionPort: Send + Sync + fmt::Debug {
    fn fetch(
        &self,
        request: &HttpsFetchRequest,
        cancellation: &CancellationToken,
    ) -> Result<HttpsFetchResponse, HttpsPortError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcquisitionFact {
    logical_path: Arc<str>,
    source_url: Arc<str>,
    outcome: AcquisitionOutcome,
}

impl AcquisitionFact {
    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }

    pub fn source_url(&self) -> &str {
        &self.source_url
    }

    pub fn outcome(&self) -> &AcquisitionOutcome {
        &self.outcome
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcquisitionOutcome {
    TransportFailed { reason: Arc<str> },
    HttpStatus { status: u16 },
    Refused { reason: AcquisitionRefusal },
    Succeeded { bytes: usize },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AcquisitionRefusal {
    #[error("operation was cancelled")]
    Cancelled,
    #[error("candidate URL is invalid")]
    InvalidCandidate,
    #[error("candidate URL is not credential-free HTTPS")]
    NonHttps,
    #[error("candidate URL or DNS result is not a public address: {address}")]
    NonPublicAddress { address: IpAddr },
    #[error("HTTPS response has no resolved-address evidence")]
    MissingAddressEvidence,
    #[error("HTTPS response has {actual} resolved addresses; the maximum is {maximum}")]
    AddressLimit { actual: usize, maximum: usize },
    #[error("redirect was refused")]
    Redirect,
    #[error("effective response URL differs from the exact requested candidate")]
    SourceConfusion,
    #[error("response is {actual} bytes; the maximum is {maximum}")]
    Oversize { actual: usize, maximum: usize },
    #[error("acquisition evidence reached its maximum of {maximum} facts")]
    EvidenceCapacity { maximum: usize },
    #[error("every finite approved source failed")]
    AllSourcesFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolutionOrigin {
    OnlineVerified,
    OfflineSealed,
}

#[derive(Clone, Debug)]
pub struct ResolvedArtifact {
    handle: VerifiedArtifactHandle,
    origin: ResolutionOrigin,
    lookup_facts: Arc<[CoordinateLookupFact]>,
    acquisition_facts: Arc<[AcquisitionFact]>,
}

impl ResolvedArtifact {
    pub fn handle(&self) -> &VerifiedArtifactHandle {
        &self.handle
    }

    pub fn origin(&self) -> ResolutionOrigin {
        self.origin
    }

    pub fn lookup_facts(&self) -> &[CoordinateLookupFact] {
        &self.lookup_facts
    }

    pub fn acquisition_facts(&self) -> &[AcquisitionFact] {
        &self.acquisition_facts
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SealedCacheError {
    #[error("sealed cache is closed")]
    Closed,
    #[error("sealed cache entry conflicts with an existing aggregate")]
    Conflict,
    #[error("sealed cache capacity exceeded")]
    Capacity,
    #[error("sealed cache implementation failed: {reason}")]
    Implementation { reason: Arc<str> },
}

/// Exact verified identity for an offline artifact record. Aggregate alone is
/// insufficient because two publishers may intentionally ship identical bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SealedArtifactKey {
    coordinate: SealedCoordinate,
    aggregate: Sha256Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SealedCoordinate {
    Snapshot {
        event_id: Sha256Digest,
        author: Sha256Digest,
    },
    Root {
        author: Sha256Digest,
    },
    Named {
        author: Sha256Digest,
        d_tag: Arc<str>,
    },
}

impl SealedArtifactKey {
    pub fn for_coordinate(coordinate: &ManifestCoordinate, aggregate: Sha256Digest) -> Self {
        let coordinate = match coordinate {
            ManifestCoordinate::Snapshot { event_id, author } => SealedCoordinate::Snapshot {
                event_id: event_id.clone(),
                author: author.clone(),
            },
            ManifestCoordinate::Root { author } => SealedCoordinate::Root {
                author: author.clone(),
            },
            ManifestCoordinate::Named { author, d_tag } => SealedCoordinate::Named {
                author: author.clone(),
                d_tag: Arc::clone(d_tag),
            },
        };
        Self {
            coordinate,
            aggregate,
        }
    }

    pub fn aggregate(&self) -> &Sha256Digest {
        &self.aggregate
    }
}

/// Indexes artifact-owned sealed handles. Implementations must never write or
/// reinterpret artifact bytes.
pub trait SealedArtifactCache: Send + Sync + fmt::Debug {
    fn load(
        &self,
        key: &SealedArtifactKey,
    ) -> Result<Option<VerifiedArtifactHandle>, SealedCacheError>;

    fn retain(
        &self,
        key: &SealedArtifactKey,
        handle: &VerifiedArtifactHandle,
    ) -> Result<(), SealedCacheError>;
}

#[derive(Debug)]
pub struct MemorySealedArtifactCache {
    maximum_entries: usize,
    maximum_bytes: usize,
    state: Mutex<MemoryCacheState>,
}

#[derive(Debug, Default)]
struct MemoryCacheState {
    total_bytes: usize,
    entries: BTreeMap<SealedArtifactKey, VerifiedArtifactHandle>,
}

impl MemorySealedArtifactCache {
    pub fn new(maximum_entries: usize, maximum_bytes: usize) -> Result<Self, SealedCacheError> {
        if maximum_entries == 0 || maximum_bytes == 0 {
            return Err(SealedCacheError::Capacity);
        }
        Ok(Self {
            maximum_entries,
            maximum_bytes,
            state: Mutex::new(MemoryCacheState::default()),
        })
    }
}

impl SealedArtifactCache for MemorySealedArtifactCache {
    fn load(
        &self,
        key: &SealedArtifactKey,
    ) -> Result<Option<VerifiedArtifactHandle>, SealedCacheError> {
        Ok(self.state.lock().entries.get(key).cloned())
    }

    fn retain(
        &self,
        key: &SealedArtifactKey,
        handle: &VerifiedArtifactHandle,
    ) -> Result<(), SealedCacheError> {
        if handle.index().aggregate() != key.aggregate()
            || !sealed_coordinate_matches_index(&key.coordinate, handle.index())
        {
            return Err(SealedCacheError::Conflict);
        }
        let bytes = handle
            .index()
            .entries()
            .try_fold(0usize, |total, entry| total.checked_add(entry.bytes()))
            .ok_or(SealedCacheError::Capacity)?;
        let mut state = self.state.lock();
        if let Some(existing) = state.entries.get(key) {
            return if existing.index() == handle.index() {
                Ok(())
            } else {
                Err(SealedCacheError::Conflict)
            };
        }
        let total_bytes = state
            .total_bytes
            .checked_add(bytes)
            .ok_or(SealedCacheError::Capacity)?;
        if state.entries.len() >= self.maximum_entries || total_bytes > self.maximum_bytes {
            return Err(SealedCacheError::Capacity);
        }
        state.total_bytes = total_bytes;
        state.entries.insert(key.clone(), handle.clone());
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("resolver limits must be finite and non-zero")]
    InvalidLimits,
    #[error("resolver is saturated at {maximum} concurrent operations")]
    Saturated { maximum: usize },
    #[error("operation was cancelled")]
    Cancelled,
    #[error("manifest lookup failed: {reason}")]
    Lookup { reason: Arc<str> },
    #[error("manifest lookup returned no selected row; inspect scoped lookup facts")]
    NotFound { facts: Arc<[CoordinateLookupFact]> },
    #[error("lookup returned {actual} facts; the maximum is {maximum}")]
    LookupFactLimit { actual: usize, maximum: usize },
    #[error("lookup fact violates bounded evidence policy")]
    InvalidLookupFact,
    #[error("selected manifest is {actual} bytes; the maximum is {maximum}")]
    ManifestTooLarge { actual: usize, maximum: usize },
    #[error("artifact verification or sealing failed: {reason}")]
    Artifact { reason: Arc<str> },
    #[error("artifact acquisition was refused: {reason}")]
    Acquisition {
        reason: AcquisitionRefusal,
        facts: Arc<[AcquisitionFact]>,
    },
    #[error("sealed artifact cache failed: {0}")]
    Cache(#[from] SealedCacheError),
    #[error("offline aggregate does not match the requested coordinate")]
    OfflineCoordinateMismatch,
    #[error("no sealed artifact exists for aggregate {aggregate:?}")]
    OfflineMiss { aggregate: Sha256Digest },
}

impl ResolveError {
    pub fn lookup_facts(&self) -> Option<&[CoordinateLookupFact]> {
        match self {
            Self::NotFound { facts } => Some(facts),
            _ => None,
        }
    }

    pub fn acquisition_facts(&self) -> Option<&[AcquisitionFact]> {
        match self {
            Self::Acquisition { facts, .. } => Some(facts),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct CatalogResolver<'a> {
    limits: ResolverLimits,
    artifact_limits: ArtifactLimits,
    source_policy: ArtifactSourcePolicy,
    lookup: &'a dyn ManifestLookupPort,
    transport: &'a dyn HttpsAcquisitionPort,
    artifact_cache: &'a FileArtifactCache,
    sealed_cache: &'a dyn SealedArtifactCache,
    admission: Admission,
}

impl<'a> CatalogResolver<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        limits: ResolverLimits,
        artifact_limits: ArtifactLimits,
        source_policy: ArtifactSourcePolicy,
        lookup: &'a dyn ManifestLookupPort,
        transport: &'a dyn HttpsAcquisitionPort,
        artifact_cache: &'a FileArtifactCache,
        sealed_cache: &'a dyn SealedArtifactCache,
    ) -> Result<Self, ResolveError> {
        let limits = limits.validate()?;
        if artifact_limits.maximum_files == 0
            || artifact_limits.maximum_file_bytes == 0
            || artifact_limits.maximum_total_bytes == 0
        {
            return Err(ResolveError::InvalidLimits);
        }
        Ok(Self {
            limits,
            artifact_limits,
            source_policy,
            lookup,
            transport,
            artifact_cache,
            sealed_cache,
            admission: Admission::new(limits.maximum_in_flight),
        })
    }

    pub fn resolve(
        &self,
        coordinate: &ManifestCoordinate,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedArtifact, ResolveError> {
        let _permit = self.admission.reserve()?;
        ensure_not_cancelled(cancellation)?;
        let request = ManifestLookupRequest {
            coordinate: coordinate.clone(),
            maximum_event_bytes: self.limits.maximum_manifest_bytes,
            maximum_facts: self.limits.maximum_lookup_facts,
        };
        let lookup = self
            .lookup
            .lookup(&request, cancellation)
            .map_err(|error| ResolveError::Lookup {
                reason: bounded_reason(error.reason, self.limits.maximum_reason_bytes),
            })?;
        ensure_not_cancelled(cancellation)?;
        self.validate_lookup_facts(&lookup.facts)?;
        let event_json = lookup
            .selected_event_json
            .ok_or_else(|| ResolveError::NotFound {
                facts: Arc::clone(&lookup.facts),
            })?;
        if event_json.len() > self.limits.maximum_manifest_bytes {
            return Err(ResolveError::ManifestTooLarge {
                actual: event_json.len(),
                maximum: self.limits.maximum_manifest_bytes,
            });
        }

        let source = SafeManifestBlobSource::new(self.transport, cancellation.clone(), self.limits);
        let resolver = SignedArtifactResolver::new(
            ManifestEventVerifier::pinned(),
            self.artifact_limits,
            self.source_policy.clone(),
            &source,
            self.artifact_cache,
        )
        .map_err(|error| ResolveError::Artifact {
            reason: Arc::from(error.to_string()),
        })?;
        let handle = match resolver.resolve_json(&event_json, coordinate) {
            Ok(handle) => handle,
            Err(error) => {
                let facts = source.facts();
                if let Some(reason) = source.terminal_refusal() {
                    return Err(ResolveError::Acquisition { reason, facts });
                }
                return Err(ResolveError::Artifact {
                    reason: Arc::from(error.to_string()),
                });
            }
        };
        ensure_not_cancelled(cancellation)?;
        let key = SealedArtifactKey::for_coordinate(coordinate, handle.index().aggregate().clone());
        self.sealed_cache.retain(&key, &handle)?;
        Ok(ResolvedArtifact {
            handle,
            origin: ResolutionOrigin::OnlineVerified,
            lookup_facts: lookup.facts,
            acquisition_facts: source.facts(),
        })
    }

    pub fn resolve_offline(
        &self,
        coordinate: &ManifestCoordinate,
        aggregate: &Sha256Digest,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedArtifact, ResolveError> {
        let _permit = self.admission.reserve()?;
        ensure_not_cancelled(cancellation)?;
        let key = SealedArtifactKey::for_coordinate(coordinate, aggregate.clone());
        let handle = self
            .sealed_cache
            .load(&key)?
            .ok_or_else(|| ResolveError::OfflineMiss {
                aggregate: aggregate.clone(),
            })?;
        if !index_matches_coordinate(handle.index(), coordinate) {
            return Err(ResolveError::OfflineCoordinateMismatch);
        }
        ensure_not_cancelled(cancellation)?;
        Ok(ResolvedArtifact {
            handle,
            origin: ResolutionOrigin::OfflineSealed,
            lookup_facts: Arc::from([]),
            acquisition_facts: Arc::from([]),
        })
    }

    fn validate_lookup_facts(&self, facts: &[CoordinateLookupFact]) -> Result<(), ResolveError> {
        if facts.len() > self.limits.maximum_lookup_facts {
            return Err(ResolveError::LookupFactLimit {
                actual: facts.len(),
                maximum: self.limits.maximum_lookup_facts,
            });
        }
        for fact in facts {
            if fact.source.is_empty() || fact.source.len() > self.limits.maximum_source_label_bytes
            {
                return Err(ResolveError::InvalidLookupFact);
            }
            match &fact.state {
                CoordinateLookupState::Observed { .. } => {}
                CoordinateLookupState::Shortfall { reason } => {
                    if reason.is_empty() || reason.len() > self.limits.maximum_reason_bytes {
                        return Err(ResolveError::InvalidLookupFact);
                    }
                }
                CoordinateLookupState::Selected { event_id } => {
                    if Sha256Digest::parse(event_id.to_string()).is_err() {
                        return Err(ResolveError::InvalidLookupFact);
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Admission {
    maximum: usize,
    active: Mutex<usize>,
}

impl Admission {
    fn new(maximum: usize) -> Self {
        Self {
            maximum,
            active: Mutex::new(0),
        }
    }

    fn reserve(&self) -> Result<AdmissionPermit<'_>, ResolveError> {
        let mut active = self.active.lock();
        if *active >= self.maximum {
            return Err(ResolveError::Saturated {
                maximum: self.maximum,
            });
        }
        *active += 1;
        Ok(AdmissionPermit { admission: self })
    }
}

struct AdmissionPermit<'a> {
    admission: &'a Admission,
}

impl Drop for AdmissionPermit<'_> {
    fn drop(&mut self) {
        let mut active = self.admission.active.lock();
        *active = active.saturating_sub(1);
    }
}

#[derive(Debug)]
struct SafeManifestBlobSource<'a> {
    transport: &'a dyn HttpsAcquisitionPort,
    cancellation: CancellationToken,
    limits: ResolverLimits,
    state: Mutex<AcquisitionState>,
}

#[derive(Debug, Default)]
struct AcquisitionState {
    facts: Vec<AcquisitionFact>,
    terminal_refusal: Option<AcquisitionRefusal>,
}

impl<'a> SafeManifestBlobSource<'a> {
    fn new(
        transport: &'a dyn HttpsAcquisitionPort,
        cancellation: CancellationToken,
        limits: ResolverLimits,
    ) -> Self {
        Self {
            transport,
            cancellation,
            limits,
            state: Mutex::new(AcquisitionState::default()),
        }
    }

    fn facts(&self) -> Arc<[AcquisitionFact]> {
        self.state.lock().facts.clone().into()
    }

    fn terminal_refusal(&self) -> Option<AcquisitionRefusal> {
        self.state.lock().terminal_refusal.clone()
    }

    fn refuse(
        &self,
        logical_path: &str,
        source_url: &str,
        reason: AcquisitionRefusal,
    ) -> BlobSourceError {
        let fact = AcquisitionFact {
            logical_path: Arc::from(logical_path),
            source_url: Arc::from(source_url),
            outcome: AcquisitionOutcome::Refused {
                reason: reason.clone(),
            },
        };
        let mut state = self.state.lock();
        if state.facts.len() < self.limits.maximum_acquisition_facts {
            state.facts.push(fact);
            state.terminal_refusal = Some(reason.clone());
        } else {
            state.terminal_refusal = Some(AcquisitionRefusal::EvidenceCapacity {
                maximum: self.limits.maximum_acquisition_facts,
            });
        }
        BlobSourceError {
            reason: state
                .terminal_refusal
                .as_ref()
                .expect("terminal refusal was just assigned")
                .to_string(),
        }
    }

    fn record(
        &self,
        logical_path: &str,
        source_url: &str,
        outcome: AcquisitionOutcome,
    ) -> Result<(), BlobSourceError> {
        let mut state = self.state.lock();
        if state.facts.len() >= self.limits.maximum_acquisition_facts {
            let reason = AcquisitionRefusal::EvidenceCapacity {
                maximum: self.limits.maximum_acquisition_facts,
            };
            state.terminal_refusal = Some(reason.clone());
            return Err(BlobSourceError {
                reason: reason.to_string(),
            });
        }
        state.facts.push(AcquisitionFact {
            logical_path: Arc::from(logical_path),
            source_url: Arc::from(source_url),
            outcome,
        });
        Ok(())
    }
}

impl ManifestBlobSource for SafeManifestBlobSource<'_> {
    fn fetch(&self, request: &BlobFetchRequest) -> Result<BlobFetchResponse, BlobSourceError> {
        for candidate in request.candidate_urls() {
            if self.cancellation.is_cancelled() {
                return Err(self.refuse(
                    request.logical_path(),
                    candidate,
                    AcquisitionRefusal::Cancelled,
                ));
            }
            let validated = match validate_candidate(candidate, self.limits.maximum_url_bytes) {
                Ok(url) => url,
                Err(reason) => {
                    return Err(self.refuse(request.logical_path(), candidate, reason));
                }
            };
            let raw_request = HttpsFetchRequest {
                url: Arc::from(candidate),
                maximum_bytes: request.maximum_bytes(),
            };
            let response = match self.transport.fetch(&raw_request, &self.cancellation) {
                Ok(response) => response,
                Err(error) => {
                    let reason = bounded_reason(error.reason, self.limits.maximum_reason_bytes);
                    self.record(
                        request.logical_path(),
                        candidate,
                        AcquisitionOutcome::TransportFailed { reason },
                    )?;
                    continue;
                }
            };
            if self.cancellation.is_cancelled() {
                return Err(self.refuse(
                    request.logical_path(),
                    candidate,
                    AcquisitionRefusal::Cancelled,
                ));
            }
            if response.resolved_addresses.is_empty() {
                return Err(self.refuse(
                    request.logical_path(),
                    candidate,
                    AcquisitionRefusal::MissingAddressEvidence,
                ));
            }
            if response.resolved_addresses.len() > self.limits.maximum_resolved_addresses {
                return Err(self.refuse(
                    request.logical_path(),
                    candidate,
                    AcquisitionRefusal::AddressLimit {
                        actual: response.resolved_addresses.len(),
                        maximum: self.limits.maximum_resolved_addresses,
                    },
                ));
            }
            for address in response.resolved_addresses.iter().copied() {
                if !is_public_ip(address) {
                    return Err(self.refuse(
                        request.logical_path(),
                        candidate,
                        AcquisitionRefusal::NonPublicAddress { address },
                    ));
                }
            }
            if response.redirect_location.is_some() || (300..400).contains(&response.status) {
                return Err(self.refuse(
                    request.logical_path(),
                    candidate,
                    AcquisitionRefusal::Redirect,
                ));
            }
            if response.effective_url.as_ref() != validated.as_str() {
                return Err(self.refuse(
                    request.logical_path(),
                    candidate,
                    AcquisitionRefusal::SourceConfusion,
                ));
            }
            if response.body.len() > request.maximum_bytes() {
                return Err(self.refuse(
                    request.logical_path(),
                    candidate,
                    AcquisitionRefusal::Oversize {
                        actual: response.body.len(),
                        maximum: request.maximum_bytes(),
                    },
                ));
            }
            if response.status != 200 {
                self.record(
                    request.logical_path(),
                    candidate,
                    AcquisitionOutcome::HttpStatus {
                        status: response.status,
                    },
                )?;
                continue;
            }
            self.record(
                request.logical_path(),
                candidate,
                AcquisitionOutcome::Succeeded {
                    bytes: response.body.len(),
                },
            )?;
            return Ok(BlobFetchResponse::ok(
                candidate,
                Box::new(Cursor::new(response.body)),
            ));
        }
        Err(self.refuse(
            request.logical_path(),
            "",
            AcquisitionRefusal::AllSourcesFailed,
        ))
    }
}

fn validate_candidate(
    candidate: &str,
    maximum_url_bytes: usize,
) -> Result<Url, AcquisitionRefusal> {
    if candidate.len() > maximum_url_bytes {
        return Err(AcquisitionRefusal::InvalidCandidate);
    }
    let url = Url::parse(candidate).map_err(|_| AcquisitionRefusal::InvalidCandidate)?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AcquisitionRefusal::NonHttps);
    }
    match url.host() {
        Some(Host::Ipv4(address)) if !is_public_ip(IpAddr::V4(address)) => {
            Err(AcquisitionRefusal::NonPublicAddress {
                address: IpAddr::V4(address),
            })
        }
        Some(Host::Ipv6(address)) if !is_public_ip(IpAddr::V6(address)) => {
            Err(AcquisitionRefusal::NonPublicAddress {
                address: IpAddr::V6(address),
            })
        }
        Some(Host::Domain(domain))
            if domain.eq_ignore_ascii_case("localhost")
                || domain.to_ascii_lowercase().ends_with(".localhost") =>
        {
            Err(AcquisitionRefusal::NonHttps)
        }
        Some(_) => Ok(url),
        None => Err(AcquisitionRefusal::InvalidCandidate),
    }
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || address == Ipv4Addr::BROADCAST)
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if address.is_unspecified() || address.is_loopback() || address.is_multicast() {
        return false;
    }
    let segments = address.segments();
    if segments[0] & 0xfe00 == 0xfc00
        || segments[0] & 0xffc0 == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
    {
        return false;
    }
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    true
}

fn bounded_reason(reason: Arc<str>, maximum: usize) -> Arc<str> {
    if reason.len() <= maximum {
        return reason;
    }
    let mut end = maximum;
    while !reason.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    Arc::from(&reason[..end])
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), ResolveError> {
    if cancellation.is_cancelled() {
        Err(ResolveError::Cancelled)
    } else {
        Ok(())
    }
}

fn index_matches_coordinate(
    index: &VerifiedArtifactIndex,
    coordinate: &ManifestCoordinate,
) -> bool {
    match coordinate {
        ManifestCoordinate::Snapshot { event_id, author } => {
            index.kind() == KIND_SNAPSHOT
                && index.event_id() == event_id
                && index.author() == author
                && index.d_tag().is_none()
        }
        ManifestCoordinate::Root { author } => {
            index.kind() == KIND_ROOT && index.author() == author && index.d_tag().is_none()
        }
        ManifestCoordinate::Named { author, d_tag } => {
            index.kind() == KIND_NAMED
                && index.author() == author
                && index.d_tag() == Some(d_tag.as_ref())
        }
    }
}

fn sealed_coordinate_matches_index(
    coordinate: &SealedCoordinate,
    index: &VerifiedArtifactIndex,
) -> bool {
    match coordinate {
        SealedCoordinate::Snapshot { event_id, author } => {
            index.kind() == KIND_SNAPSHOT
                && index.event_id() == event_id
                && index.author() == author
                && index.d_tag().is_none()
        }
        SealedCoordinate::Root { author } => {
            index.kind() == KIND_ROOT && index.author() == author && index.d_tag().is_none()
        }
        SealedCoordinate::Named { author, d_tag } => {
            index.kind() == KIND_NAMED
                && index.author() == author
                && index.d_tag() == Some(d_tag.as_ref())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nmp_native_artifact::{ArtifactSourcePolicy, INDEX_PATH};
    use tempfile::TempDir;

    use super::*;

    const EVENT: &[u8] =
        include_bytes!("../../../conformance/napplet-corpus/published/good-morning/event.json");
    const INDEX: &[u8] =
        include_bytes!("../../../conformance/napplet-corpus/published/good-morning/index.html");
    const AUTHOR: &str = "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5";
    const EVENT_ID: &str = "b330bfaefd2ddf268ebe4196403e6163533c54f41dabc3518bdc1a896c68f40e";
    const AGGREGATE: &str = "828a6df02afd56782ea20f805084acce65c53f7c37554948c1e0a64aa5a2b0a8";
    const PUBLIC_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));

    #[derive(Debug)]
    struct FixtureLookup {
        calls: AtomicUsize,
    }

    impl ManifestLookupPort for FixtureLookup {
        fn lookup(
            &self,
            _request: &ManifestLookupRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ManifestLookupResponse, LookupPortError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(ManifestLookupResponse::found(
                EVENT,
                vec![
                    CoordinateLookupFact::observed("author-outbox", 1),
                    CoordinateLookupFact::selected("nmp", EVENT_ID),
                ],
            ))
        }
    }

    #[derive(Clone, Debug)]
    enum TransportMode {
        Good,
        Redirect,
        Private,
        Confused,
        Oversize,
    }

    #[derive(Debug)]
    struct FixtureTransport {
        calls: AtomicUsize,
        mode: TransportMode,
    }

    impl HttpsAcquisitionPort for FixtureTransport {
        fn fetch(
            &self,
            request: &HttpsFetchRequest,
            _cancellation: &CancellationToken,
        ) -> Result<HttpsFetchResponse, HttpsPortError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let (effective, status, redirect, addresses, body) = match self.mode {
                TransportMode::Good => (
                    Arc::from(request.url()),
                    200,
                    None,
                    Arc::from([PUBLIC_ADDRESS]),
                    Arc::<[u8]>::from(INDEX),
                ),
                TransportMode::Redirect => (
                    Arc::from(request.url()),
                    302,
                    Some(Arc::from("https://evil.example/blob")),
                    Arc::from([PUBLIC_ADDRESS]),
                    Arc::<[u8]>::from([]),
                ),
                TransportMode::Private => (
                    Arc::from(request.url()),
                    200,
                    None,
                    Arc::from([IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))]),
                    Arc::<[u8]>::from(INDEX),
                ),
                TransportMode::Confused => (
                    Arc::from("https://other.example/blob"),
                    200,
                    None,
                    Arc::from([PUBLIC_ADDRESS]),
                    Arc::<[u8]>::from(INDEX),
                ),
                TransportMode::Oversize => (
                    Arc::from(request.url()),
                    200,
                    None,
                    Arc::from([PUBLIC_ADDRESS]),
                    Arc::<[u8]>::from(vec![0; request.maximum_bytes() + 1]),
                ),
            };
            Ok(HttpsFetchResponse::new(
                effective, status, redirect, addresses, body,
            ))
        }
    }

    struct Fixture {
        temp: TempDir,
        lookup: FixtureLookup,
        transport: FixtureTransport,
        sealed: MemorySealedArtifactCache,
    }

    impl fmt::Debug for Fixture {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.debug_struct("Fixture").finish_non_exhaustive()
        }
    }

    impl Fixture {
        fn new(mode: TransportMode) -> Self {
            Self {
                temp: TempDir::new().expect("temp"),
                lookup: FixtureLookup {
                    calls: AtomicUsize::new(0),
                },
                transport: FixtureTransport {
                    calls: AtomicUsize::new(0),
                    mode,
                },
                sealed: MemorySealedArtifactCache::new(4, 64 * 1_024 * 1_024)
                    .expect("bounded cache"),
            }
        }

        fn coordinate() -> ManifestCoordinate {
            ManifestCoordinate::named(AUTHOR, "good-morning").expect("coordinate")
        }

        fn with_resolver<T>(&self, operation: impl FnOnce(&CatalogResolver<'_>) -> T) -> T {
            let artifact_cache = FileArtifactCache::open(self.temp.path().join("artifacts"))
                .expect("artifact cache");
            let resolver = CatalogResolver::new(
                ResolverLimits::default(),
                ArtifactLimits::default(),
                ArtifactSourcePolicy::manifest_https_only(8).expect("source policy"),
                &self.lookup,
                &self.transport,
                &artifact_cache,
                &self.sealed,
            )
            .expect("resolver");
            operation(&resolver)
        }
    }

    #[test]
    fn online_resolution_seals_then_offline_reinstall_uses_no_ports() {
        let fixture = Fixture::new(TransportMode::Good);
        fixture.with_resolver(|resolver| {
            let online = resolver
                .resolve(&Fixture::coordinate(), &CancellationToken::default())
                .expect("online resolution");
            assert_eq!(online.origin(), ResolutionOrigin::OnlineVerified);
            assert_eq!(
                online
                    .handle()
                    .read_verified(INDEX_PATH, 4 * 1_024 * 1_024)
                    .expect("sealed bytes"),
                INDEX
            );
            assert_eq!(online.acquisition_facts().len(), 1);

            let offline = resolver
                .resolve_offline(
                    &Fixture::coordinate(),
                    &Sha256Digest::parse(AGGREGATE).expect("aggregate"),
                    &CancellationToken::default(),
                )
                .expect("offline resolution");
            assert_eq!(offline.origin(), ResolutionOrigin::OfflineSealed);
            assert_eq!(
                offline
                    .handle()
                    .read_verified(INDEX_PATH, 4 * 1_024 * 1_024)
                    .expect("offline sealed bytes"),
                INDEX
            );
        });
        assert_eq!(fixture.lookup.calls.load(Ordering::Relaxed), 1);
        assert_eq!(fixture.transport.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn redirect_private_dns_source_confusion_and_oversize_fail_before_retention() {
        let cases = [
            (TransportMode::Redirect, "redirect"),
            (TransportMode::Private, "public address"),
            (TransportMode::Confused, "effective response URL"),
            (TransportMode::Oversize, "maximum"),
        ];
        for (mode, expected) in cases {
            let fixture = Fixture::new(mode);
            fixture.with_resolver(|resolver| {
                let error = resolver
                    .resolve(&Fixture::coordinate(), &CancellationToken::default())
                    .expect_err("policy refusal");
                assert!(error.to_string().contains(expected), "{error}");
                assert!(matches!(error, ResolveError::Acquisition { .. }));
                assert!(matches!(
                    resolver.resolve_offline(
                        &Fixture::coordinate(),
                        &Sha256Digest::parse(AGGREGATE).expect("aggregate"),
                        &CancellationToken::default(),
                    ),
                    Err(ResolveError::OfflineMiss { .. })
                ));
            });
        }
    }

    #[test]
    fn cancelled_operation_never_calls_lookup_or_transport() {
        let fixture = Fixture::new(TransportMode::Good);
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        fixture.with_resolver(|resolver| {
            assert!(matches!(
                resolver.resolve(&Fixture::coordinate(), &cancellation),
                Err(ResolveError::Cancelled)
            ));
        });
        assert_eq!(fixture.lookup.calls.load(Ordering::Relaxed), 0);
        assert_eq!(fixture.transport.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn malformed_lookup_evidence_is_refused_before_transport() {
        #[derive(Debug)]
        struct InvalidLookup;
        impl ManifestLookupPort for InvalidLookup {
            fn lookup(
                &self,
                _request: &ManifestLookupRequest,
                _cancellation: &CancellationToken,
            ) -> Result<ManifestLookupResponse, LookupPortError> {
                Ok(ManifestLookupResponse::found(
                    EVENT,
                    vec![CoordinateLookupFact::shortfall("", "missing source")],
                ))
            }
        }
        let fixture = Fixture::new(TransportMode::Good);
        let artifact_cache =
            FileArtifactCache::open(fixture.temp.path().join("artifacts")).expect("artifact cache");
        let resolver = CatalogResolver::new(
            ResolverLimits::default(),
            ArtifactLimits::default(),
            ArtifactSourcePolicy::manifest_https_only(8).expect("source policy"),
            &InvalidLookup,
            &fixture.transport,
            &artifact_cache,
            &fixture.sealed,
        )
        .expect("resolver");
        assert!(matches!(
            resolver.resolve(&Fixture::coordinate(), &CancellationToken::default()),
            Err(ResolveError::InvalidLookupFact)
        ));
        assert_eq!(fixture.transport.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn literal_private_https_candidate_is_refused_without_network() {
        assert!(matches!(
            validate_candidate("https://127.0.0.1/blob", 2_048),
            Err(AcquisitionRefusal::NonPublicAddress { .. })
        ));
        assert!(matches!(
            validate_candidate("https://[::1]/blob", 2_048),
            Err(AcquisitionRefusal::NonPublicAddress { .. })
        ));
    }

    #[test]
    fn bounded_cache_is_immutable_for_an_exact_aggregate() {
        let fixture = Fixture::new(TransportMode::Good);
        fixture.with_resolver(|resolver| {
            let first = resolver
                .resolve(&Fixture::coordinate(), &CancellationToken::default())
                .expect("first");
            let key = SealedArtifactKey::for_coordinate(
                &Fixture::coordinate(),
                first.handle().index().aggregate().clone(),
            );
            fixture
                .sealed
                .retain(&key, first.handle())
                .expect("idempotent");
            assert_eq!(fixture.sealed.state.lock().entries.len(), 1);
        });
    }

    #[test]
    fn admission_is_finite_and_has_no_waiting_queue() {
        let admission = Admission::new(1);
        let _permit = admission.reserve().expect("first permit");
        assert!(matches!(
            admission.reserve(),
            Err(ResolveError::Saturated { maximum: 1 })
        ));
    }

    #[test]
    fn not_found_preserves_scoped_shortfall_facts() {
        #[derive(Debug)]
        struct EmptyLookup;
        impl ManifestLookupPort for EmptyLookup {
            fn lookup(
                &self,
                _request: &ManifestLookupRequest,
                _cancellation: &CancellationToken,
            ) -> Result<ManifestLookupResponse, LookupPortError> {
                Ok(ManifestLookupResponse::not_found(vec![
                    CoordinateLookupFact::shortfall("author-outbox", "relay unavailable"),
                ]))
            }
        }
        let fixture = Fixture::new(TransportMode::Good);
        let artifact_cache =
            FileArtifactCache::open(fixture.temp.path().join("artifacts")).expect("artifact cache");
        let resolver = CatalogResolver::new(
            ResolverLimits::default(),
            ArtifactLimits::default(),
            ArtifactSourcePolicy::manifest_https_only(8).expect("source policy"),
            &EmptyLookup,
            &fixture.transport,
            &artifact_cache,
            &fixture.sealed,
        )
        .expect("resolver");
        let error = resolver
            .resolve(&Fixture::coordinate(), &CancellationToken::default())
            .expect_err("not found");
        assert_eq!(
            error
                .lookup_facts()
                .expect("facts")
                .first()
                .expect("fact")
                .source(),
            "author-outbox"
        );
        assert_eq!(fixture.transport.calls.load(Ordering::Relaxed), 0);
    }
}
