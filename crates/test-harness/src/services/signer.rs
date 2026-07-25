use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use super::{
    FixtureLoader, ScenarioCatalog, SignerScenario,
    admission::{DeterministicServiceError, ServiceCensus, admit},
};

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
