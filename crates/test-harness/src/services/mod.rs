use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use serde::Deserialize;

mod admission;

pub use admission::{DeterministicServiceError, ServiceCensus};
use admission::{admit, validate_fixture_name};

#[derive(Clone, Debug, Deserialize)]
pub struct ScenarioCatalog {
    pub schema: u32,
    pub clock: ClockScenario,
    pub limits: ServiceLimits,
    pub relay: Vec<RelayScenario>,
    pub blob: Vec<BlobScenario>,
    pub signer: Vec<SignerScenario>,
    pub secrets: SecretPolicy,
}

impl ScenarioCatalog {
    pub fn from_json(bytes: &[u8]) -> Result<Self, DeterministicServiceError> {
        let catalog: Self = serde_json::from_slice(bytes)
            .map_err(|error| DeterministicServiceError::InvalidCatalog(error.to_string()))?;
        if catalog.schema != 1
            || catalog.limits.max_frame_bytes == 0
            || catalog.limits.max_connections == 0
            || catalog.limits.max_requests_per_connection == 0
            || catalog.limits.max_blob_bytes == 0
        {
            return Err(DeterministicServiceError::InvalidCatalog(
                "unsupported schema or zero limit".to_owned(),
            ));
        }
        if catalog.clock.kind != "manual" {
            return Err(DeterministicServiceError::InvalidCatalog(
                "clock must be manual".to_owned(),
            ));
        }
        if catalog.secrets.fixture_policy != "no-secret-keys" {
            return Err(DeterministicServiceError::InvalidCatalog(
                "test services may not load secret keys".to_owned(),
            ));
        }
        Ok(catalog)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClockScenario {
    pub kind: String,
    pub initial_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServiceLimits {
    pub max_frame_bytes: usize,
    pub max_connections: usize,
    pub max_requests_per_connection: usize,
    pub max_blob_bytes: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RelayScenario {
    pub id: String,
    pub script: Vec<String>,
    pub observable: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlobScenario {
    pub id: String,
    pub status: u16,
    pub fixture: Option<String>,
    pub mutation: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub generated_bytes: Option<usize>,
    #[serde(default)]
    pub chunk_bytes: Option<usize>,
    #[serde(default)]
    pub clock_steps: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SignerScenario {
    pub id: String,
    pub request: String,
    pub response_fixture: Option<String>,
    pub result: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SecretPolicy {
    pub fixture_policy: String,
    pub signatures: String,
}

#[derive(Debug)]
pub struct ManualClock {
    now: AtomicU64,
}

impl ManualClock {
    pub fn new(initial_unix_seconds: u64) -> Self {
        Self {
            now: AtomicU64::new(initial_unix_seconds),
        }
    }

    pub fn now(&self) -> u64 {
        self.now.load(Ordering::Acquire)
    }

    pub fn advance(&self, seconds: u64) -> u64 {
        self.now.fetch_add(seconds, Ordering::AcqRel) + seconds
    }
}

pub trait FixtureLoader: Send + Sync + std::fmt::Debug {
    fn load(&self, name: &str) -> Result<Vec<u8>, DeterministicServiceError>;
}

#[derive(Debug)]
pub struct FsFixtureLoader {
    root: PathBuf,
    maximum_bytes: usize,
}

impl FsFixtureLoader {
    pub fn new(root: impl Into<PathBuf>, maximum_bytes: usize) -> Self {
        Self {
            root: root.into(),
            maximum_bytes,
        }
    }
}

impl FixtureLoader for FsFixtureLoader {
    fn load(&self, name: &str) -> Result<Vec<u8>, DeterministicServiceError> {
        validate_fixture_name(name)?;
        let path = self.root.join(name);
        let bytes = fs::read(&path)
            .map_err(|source| DeterministicServiceError::FixtureIo { path, source })?;
        if bytes.len() > self.maximum_bytes {
            return Err(DeterministicServiceError::FrameTooLarge {
                actual: bytes.len(),
                maximum: self.maximum_bytes,
            });
        }
        Ok(bytes)
    }
}

#[derive(Debug)]
pub struct RelayScenarioService {
    scenarios: BTreeMap<String, RelayScenario>,
    limits: ServiceLimits,
    clock: Arc<ManualClock>,
    active: Arc<AtomicUsize>,
    high_watermark: Arc<AtomicUsize>,
    refusals: Arc<AtomicU64>,
}

impl RelayScenarioService {
    pub fn new(catalog: &ScenarioCatalog, clock: Arc<ManualClock>) -> Self {
        Self {
            scenarios: catalog
                .relay
                .iter()
                .cloned()
                .map(|scenario| (scenario.id.clone(), scenario))
                .collect(),
            limits: catalog.limits.clone(),
            clock,
            active: Arc::new(AtomicUsize::new(0)),
            high_watermark: Arc::new(AtomicUsize::new(0)),
            refusals: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn connect(&self, scenario_id: &str) -> Result<RelayConnection, DeterministicServiceError> {
        admit(
            &self.active,
            &self.high_watermark,
            &self.refusals,
            self.limits.max_connections,
            "relay connections",
        )?;
        let Some(scenario) = self.scenarios.get(scenario_id).cloned() else {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return Err(DeterministicServiceError::UnknownScenario(
                scenario_id.to_owned(),
            ));
        };
        if scenario.script.len() > self.limits.max_requests_per_connection {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return Err(DeterministicServiceError::ScriptTooLong {
                actual: scenario.script.len(),
                maximum: self.limits.max_requests_per_connection,
            });
        }
        Ok(RelayConnection {
            scenario,
            cursor: 0,
            clock: Arc::clone(&self.clock),
            active: Arc::clone(&self.active),
        })
    }

    pub fn census(&self) -> ServiceCensus {
        ServiceCensus {
            active: self.active.load(Ordering::Acquire),
            high_watermark: self.high_watermark.load(Ordering::Acquire),
            refusals: self.refusals.load(Ordering::Acquire),
        }
    }
}

#[derive(Debug)]
pub struct RelayConnection {
    scenario: RelayScenario,
    cursor: usize,
    clock: Arc<ManualClock>,
    active: Arc<AtomicUsize>,
}

impl RelayConnection {
    pub fn observable(&self) -> &str {
        &self.scenario.observable
    }

    pub fn next_action(&mut self) -> Result<Option<RelayAction>, DeterministicServiceError> {
        let Some(step) = self.scenario.script.get(self.cursor) else {
            return Ok(None);
        };
        self.cursor += 1;
        parse_relay_action(step, &self.clock).map(Some)
    }
}

impl Drop for RelayConnection {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayAction {
    Accept,
    EndOfStoredEvents,
    Event(String),
    AuthChallenge(String),
    AwaitAuth,
    Closed(String),
    Disconnect,
    ClockAdvanced { seconds: u64, now: u64 },
}

fn parse_relay_action(
    step: &str,
    clock: &ManualClock,
) -> Result<RelayAction, DeterministicServiceError> {
    if step == "accept" {
        return Ok(RelayAction::Accept);
    }
    if step == "eose" {
        return Ok(RelayAction::EndOfStoredEvents);
    }
    if step == "await-auth" {
        return Ok(RelayAction::AwaitAuth);
    }
    if step == "disconnect" {
        return Ok(RelayAction::Disconnect);
    }
    if let Some(value) = step.strip_prefix("event:") {
        return Ok(RelayAction::Event(value.to_owned()));
    }
    if let Some(value) = step.strip_prefix("auth:") {
        return Ok(RelayAction::AuthChallenge(value.to_owned()));
    }
    if let Some(value) = step.strip_prefix("closed:") {
        return Ok(RelayAction::Closed(value.to_owned()));
    }
    if let Some(value) = step.strip_prefix("clock:+") {
        let seconds = value
            .parse::<u64>()
            .map_err(|_| DeterministicServiceError::InvalidStep(step.to_owned()))?;
        return Ok(RelayAction::ClockAdvanced {
            seconds,
            now: clock.advance(seconds),
        });
    }
    Err(DeterministicServiceError::InvalidStep(step.to_owned()))
}

#[derive(Debug)]
pub struct BlobScenarioService {
    scenarios: BTreeMap<String, BlobScenario>,
    limits: ServiceLimits,
    clock: Arc<ManualClock>,
    fixtures: Arc<dyn FixtureLoader>,
    active: Arc<AtomicUsize>,
    high_watermark: Arc<AtomicUsize>,
    refusals: Arc<AtomicU64>,
}

impl BlobScenarioService {
    pub fn new(
        catalog: &ScenarioCatalog,
        clock: Arc<ManualClock>,
        fixtures: Arc<dyn FixtureLoader>,
    ) -> Self {
        Self {
            scenarios: catalog
                .blob
                .iter()
                .cloned()
                .map(|scenario| (scenario.id.clone(), scenario))
                .collect(),
            limits: catalog.limits.clone(),
            clock,
            fixtures,
            active: Arc::new(AtomicUsize::new(0)),
            high_watermark: Arc::new(AtomicUsize::new(0)),
            refusals: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn request(&self, scenario_id: &str) -> Result<BlobResponse, DeterministicServiceError> {
        admit(
            &self.active,
            &self.high_watermark,
            &self.refusals,
            self.limits.max_connections,
            "blob requests",
        )?;
        let result = self.build_response(scenario_id);
        if result.is_err() {
            self.active.fetch_sub(1, Ordering::AcqRel);
        }
        result
    }

    pub fn census(&self) -> ServiceCensus {
        ServiceCensus {
            active: self.active.load(Ordering::Acquire),
            high_watermark: self.high_watermark.load(Ordering::Acquire),
            refusals: self.refusals.load(Ordering::Acquire),
        }
    }

    fn build_response(&self, scenario_id: &str) -> Result<BlobResponse, DeterministicServiceError> {
        let scenario = self
            .scenarios
            .get(scenario_id)
            .ok_or_else(|| DeterministicServiceError::UnknownScenario(scenario_id.to_owned()))?;
        let mut bytes = match (&scenario.fixture, scenario.generated_bytes) {
            (Some(fixture), None) => self.fixtures.load(fixture)?,
            (None, Some(size)) => {
                if size > self.limits.max_blob_bytes.saturating_add(1) {
                    return Err(DeterministicServiceError::FrameTooLarge {
                        actual: size,
                        maximum: self.limits.max_blob_bytes.saturating_add(1),
                    });
                }
                vec![0; size]
            }
            (None, None) => Vec::new(),
            (Some(_), Some(_)) => {
                return Err(DeterministicServiceError::InvalidCatalog(
                    "blob scenario has fixture and generated_bytes".to_owned(),
                ));
            }
        };
        if bytes.len() > self.limits.max_blob_bytes.saturating_add(1) {
            return Err(DeterministicServiceError::FrameTooLarge {
                actual: bytes.len(),
                maximum: self.limits.max_blob_bytes.saturating_add(1),
            });
        }
        let mut location = None;
        if scenario.mutation == "none" {
        } else if let Some(index) = scenario.mutation.strip_prefix("flip-byte:") {
            let index = index
                .parse::<usize>()
                .map_err(|_| DeterministicServiceError::InvalidStep(scenario.mutation.clone()))?;
            let byte = bytes
                .get_mut(index)
                .ok_or_else(|| DeterministicServiceError::InvalidStep(scenario.mutation.clone()))?;
            *byte ^= 1;
        } else if let Some(value) = scenario.mutation.strip_prefix("location:") {
            location = Some(value.to_owned());
        } else {
            return Err(DeterministicServiceError::InvalidStep(
                scenario.mutation.clone(),
            ));
        }
        let chunk_bytes = scenario.chunk_bytes.unwrap_or(bytes.len().max(1));
        if chunk_bytes == 0 {
            return Err(DeterministicServiceError::InvalidCatalog(
                "chunk_bytes must be non-zero".to_owned(),
            ));
        }
        Ok(BlobResponse {
            status: scenario.status,
            content_type: scenario.content_type.clone(),
            location,
            body: BlobBody {
                bytes: Arc::from(bytes),
                cursor: 0,
                chunk_bytes,
                remaining_clock_steps: scenario.clock_steps.unwrap_or_default(),
                clock: Arc::clone(&self.clock),
                active: Arc::clone(&self.active),
            },
        })
    }
}

#[derive(Debug)]
pub struct BlobResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub location: Option<String>,
    pub body: BlobBody,
}

#[derive(Debug)]
pub struct BlobBody {
    bytes: Arc<[u8]>,
    cursor: usize,
    chunk_bytes: usize,
    remaining_clock_steps: u64,
    clock: Arc<ManualClock>,
    active: Arc<AtomicUsize>,
}

impl BlobBody {
    pub fn declared_size(&self) -> usize {
        self.bytes.len()
    }

    pub fn next_chunk(&mut self) -> Option<Arc<[u8]>> {
        if self.cursor == self.bytes.len() {
            return None;
        }
        let end = self
            .cursor
            .saturating_add(self.chunk_bytes)
            .min(self.bytes.len());
        let chunk = Arc::from(&self.bytes[self.cursor..end]);
        self.cursor = end;
        if self.remaining_clock_steps > 0 {
            self.clock.advance(1);
            self.remaining_clock_steps -= 1;
        }
        Some(chunk)
    }
}

impl Drop for BlobBody {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub struct SignerScenarioService {
    scenarios: BTreeMap<String, SignerScenario>,
    fixtures: Arc<dyn FixtureLoader>,
    maximum_requests: usize,
    requests: AtomicUsize,
    high_watermark: AtomicUsize,
    refusals: AtomicU64,
}

impl SignerScenarioService {
    pub fn new(catalog: &ScenarioCatalog, fixtures: Arc<dyn FixtureLoader>) -> Self {
        Self {
            scenarios: catalog
                .signer
                .iter()
                .cloned()
                .map(|scenario| (scenario.id.clone(), scenario))
                .collect(),
            fixtures,
            maximum_requests: catalog.limits.max_requests_per_connection,
            requests: AtomicUsize::new(0),
            high_watermark: AtomicUsize::new(0),
            refusals: AtomicU64::new(0),
        }
    }

    pub fn request(
        &self,
        scenario_id: &str,
        request_fixture: &str,
    ) -> Result<SignerOutcome, DeterministicServiceError> {
        admit(
            &self.requests,
            &self.high_watermark,
            &self.refusals,
            self.maximum_requests,
            "signer requests",
        )?;
        let result = self.sign(scenario_id, request_fixture);
        self.requests.fetch_sub(1, Ordering::AcqRel);
        result
    }

    pub fn census(&self) -> ServiceCensus {
        ServiceCensus {
            active: self.requests.load(Ordering::Acquire),
            high_watermark: self.high_watermark.load(Ordering::Acquire),
            refusals: self.refusals.load(Ordering::Acquire),
        }
    }

    fn sign(
        &self,
        scenario_id: &str,
        request_fixture: &str,
    ) -> Result<SignerOutcome, DeterministicServiceError> {
        let scenario = self
            .scenarios
            .get(scenario_id)
            .ok_or_else(|| DeterministicServiceError::UnknownScenario(scenario_id.to_owned()))?;
        if scenario.request != request_fixture {
            return Err(DeterministicServiceError::UnexpectedRequest {
                expected: scenario.request.clone(),
                actual: request_fixture.to_owned(),
            });
        }
        match scenario.result.as_str() {
            "approved" => {
                let fixture = scenario.response_fixture.as_deref().ok_or_else(|| {
                    DeterministicServiceError::InvalidCatalog(
                        "approved signer needs response fixture".to_owned(),
                    )
                })?;
                Ok(SignerOutcome::Approved(self.fixtures.load(fixture)?))
            }
            "rejected" => Ok(SignerOutcome::Rejected),
            "invalid" => {
                let fixture = scenario.response_fixture.as_deref().ok_or_else(|| {
                    DeterministicServiceError::InvalidCatalog(
                        "invalid signer needs response fixture".to_owned(),
                    )
                })?;
                Ok(SignerOutcome::Invalid(self.fixtures.load(fixture)?))
            }
            "unavailable" => Ok(SignerOutcome::Unavailable),
            other => Err(DeterministicServiceError::InvalidStep(other.to_owned())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignerOutcome {
    Approved(Vec<u8>),
    Rejected,
    Invalid(Vec<u8>),
    Unavailable,
}

#[cfg(test)]
mod tests;
