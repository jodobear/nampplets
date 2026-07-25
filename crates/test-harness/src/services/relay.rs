use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use super::{
    ManualClock, RelayScenario, ScenarioCatalog, ServiceLimits,
    admission::{DeterministicServiceError, ServiceCensus, admit},
};

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
