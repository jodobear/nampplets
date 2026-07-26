#![allow(dead_code)]

use std::{collections::BTreeMap, io::Write, process::Command};

use nmp_native_performance_harness::{
    AttemptOutcome, AttemptPhase, BuildIdentity, Environment, EvidenceIdentity, FixtureIdentity,
    Harness, MeasurementAvailability, Protocol, Refusal, ResultArtifact, RunState, Sample,
    SystemMonotonicClock,
};
use nmp_native_runtime_core::{
    ResourceClass, ResourceLimits, ResourceRefusal, ResourceTracker, SessionId,
};
use serde_json::Value;
use tempfile::NamedTempFile;

pub const BENCHMARK_ID: &str = "rust.runtime-core.resource-admission.v1";
const SESSION: SessionId = SessionId(7);

pub fn resource_exemplar(state: RunState) -> ResultArtifact {
    let identity = identity(state, 4);
    let warm_tracker = (state == RunState::Warm).then(tracker);
    Harness::new(SystemMonotonicClock::start())
        .run(
            format!("resource-admission-{:?}", state).to_ascii_lowercase(),
            identity,
            build(),
            |phase| {
                let capacity = matches!(phase, AttemptPhase::Measured { sequence: 3 });
                match &warm_tracker {
                    Some(tracker) => exercise(tracker, capacity),
                    None => exercise(&tracker(), capacity),
                }
            },
        )
        .unwrap()
}

fn exercise(tracker: &ResourceTracker, capacity: bool) -> AttemptOutcome {
    if capacity {
        let held = tracker
            .admit(SESSION, None, ResourceClass::ProviderCall)
            .unwrap();
        let refusal = tracker
            .admit(SESSION, None, ResourceClass::ProviderCall)
            .unwrap_err();
        assert_eq!(
            refusal,
            ResourceRefusal::SessionCapacity {
                session: SESSION,
                capacity: 1,
            }
        );
        assert_eq!(tracker.census().admitted, 1);
        drop(held);
        assert_eq!(tracker.census().admitted, 0);
        AttemptOutcome::Refused(Refusal {
            domain: "runtime.resource_tracker".to_owned(),
            code: "session_capacity:session_7:limit_1".to_owned(),
        })
    } else {
        let lease = tracker
            .admit(SESSION, None, ResourceClass::ProviderCall)
            .unwrap();
        assert_eq!(tracker.census().admitted, 1);
        drop(lease);
        assert_eq!(tracker.census().admitted, 0);
        AttemptOutcome::Success
    }
}

fn tracker() -> ResourceTracker {
    ResourceTracker::new(ResourceLimits {
        global: 2,
        per_session: 1,
        per_class: BTreeMap::from([
            (ResourceClass::ProviderCall, 2),
            (ResourceClass::Subscription, 2),
            (ResourceClass::ResourceStream, 2),
            (ResourceClass::StateDelivery, 2),
            (ResourceClass::Action, 2),
            (ResourceClass::WebView, 2),
        ]),
    })
    .unwrap()
}

pub fn deterministic_result(state: RunState) -> ResultArtifact {
    ResultArtifact::new(
        "deterministic-statistics",
        identity(state, 3),
        build(),
        [1, 2, 3]
            .into_iter()
            .enumerate()
            .map(|(sequence, duration_ns)| Sample::Success {
                sequence,
                duration_ns,
                cpu_time_ns: None,
                peak_rss_bytes: None,
            })
            .collect(),
    )
    .unwrap()
}

pub fn diagnostic_result() -> ResultArtifact {
    ResultArtifact::new(
        "diagnostic-deadline",
        identity(RunState::Warm, 1),
        build(),
        vec![Sample::DeadlineExceeded {
            sequence: 0,
            duration_ns: 50,
            cpu_time_ns: None,
            peak_rss_bytes: None,
        }],
    )
    .unwrap()
}

fn identity(state: RunState, sample_count: usize) -> EvidenceIdentity {
    EvidenceIdentity {
        benchmark_id: BENCHMARK_ID.to_owned(),
        state,
        reset_scopes: vec!["resource_tracker".to_owned()],
        fixture: FixtureIdentity {
            id: "resource-admission-v1".to_owned(),
            sha256: "0".repeat(64),
            cardinality: sample_count as u64,
        },
        protocol: Protocol {
            warmup_count: 2,
            sample_count,
            per_sample_deadline_ns: 1_000_000_000,
            run_deadline_ns: 10_000_000_000,
            outlier_policy: "tukey_upper_3_iqr_v1".to_owned(),
        },
        build_mode: if cfg!(debug_assertions) {
            "debug-diagnostic-nonparallel"
        } else {
            "release-nonparallel"
        }
        .to_owned(),
        toolchain: "rustc-workspace-1.85-plus".to_owned(),
        environment: Environment::bounded(
            "ordinary-test-host",
            std::env::consts::OS,
            std::env::consts::ARCH,
            "unknown",
            "unknown",
            MeasurementAvailability::Unavailable,
        )
        .unwrap(),
    }
}

fn build() -> BuildIdentity {
    BuildIdentity {
        source_revision: "9".repeat(40),
        artifact_locator: "memory:resource-tracker-exemplar".to_owned(),
        source_provenance: "git:https://github.com/pablof7z/nampplets".to_owned(),
    }
}

pub fn validator_report(bytes: &[u8]) -> Value {
    let mut artifact = NamedTempFile::new().unwrap();
    artifact.write_all(bytes).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("python3")
        .arg(root.join("scripts/ci/validate_performance_evidence.py"))
        .arg(artifact.path())
        .current_dir(root)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap()
}

pub fn canonical(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
