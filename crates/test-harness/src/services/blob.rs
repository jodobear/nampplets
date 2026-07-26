use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use super::{
    BlobScenario, FixtureLoader, ManualClock, ScenarioCatalog, ServiceLimits,
    admission::{DeterministicServiceError, ServiceCensus, admit},
};

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
