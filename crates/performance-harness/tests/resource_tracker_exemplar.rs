mod support;

use nmp_native_performance_harness::{ComparisonArtifact, RunState};
use serde_json::Value;
use support::*;

#[test]
fn cold_and_warm_real_resource_tracker_artifacts_validate_separately() {
    let cold = resource_exemplar(RunState::Cold);
    let warm = resource_exemplar(RunState::Warm);
    assert_ne!(cold.comparison_key, warm.comparison_key);
    assert!(
        validator_report(&cold.canonical_bytes().unwrap())["ok"]
            .as_bool()
            .unwrap()
    );
    assert!(
        validator_report(&warm.canonical_bytes().unwrap())["ok"]
            .as_bool()
            .unwrap()
    );

    let comparison = ComparisonArtifact::observed("cold-vs-warm", &cold, &warm).unwrap();
    let value: Value = serde_json::from_slice(&comparison.canonical_bytes().unwrap()).unwrap();
    assert_eq!(value["producer_summary"]["disposition"], "incomparable");
    assert_eq!(
        value["producer_summary"]["mismatch_codes"],
        serde_json::json!(["state_mismatch"])
    );
}

#[test]
fn real_session_capacity_refusal_keeps_ownership_and_never_becomes_deadline() {
    let artifact = resource_exemplar(RunState::Warm);
    assert_eq!(artifact.producer_summary.outcome_counts.refused, 1);
    assert_eq!(
        artifact.producer_summary.outcome_counts.deadline_exceeded,
        0
    );
    assert_eq!(artifact.producer_summary.refusal_groups.len(), 1);
    let group = &artifact.producer_summary.refusal_groups[0];
    assert_eq!(group.domain, "runtime.resource_tracker");
    assert_eq!(group.code, "session_capacity:session_7:limit_1");
}

#[test]
fn raw_samples_reproduce_nearest_rank_and_exact_population_variance() {
    let artifact = deterministic_result(RunState::Warm);
    let distribution = &artifact.producer_summary.distributions[0];
    assert_eq!(
        (
            distribution.p50_ns,
            distribution.p95_ns,
            distribution.p99_ns
        ),
        (2, 3, 3)
    );
    assert_eq!(distribution.population_variance_ns2.numerator, "6");
    assert_eq!(distribution.population_variance_ns2.denominator, "9");
    assert!(
        validator_report(&artifact.canonical_bytes().unwrap())["ok"]
            .as_bool()
            .unwrap()
    );
}
