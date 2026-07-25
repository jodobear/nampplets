use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Deserialize;

mod admission;
mod blob;
mod relay;
mod signer;

use admission::validate_fixture_name;
pub use admission::{DeterministicServiceError, ServiceCensus};
pub use blob::{BlobBody, BlobResponse, BlobScenarioService};
pub use relay::{RelayAction, RelayConnection, RelayScenarioService};
pub use signer::{SignerOutcome, SignerScenarioService};

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

#[cfg(test)]
mod tests;
