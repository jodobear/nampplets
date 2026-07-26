mod support;

use cucumber::{World, given, then, when};
use nmp_native_performance_harness::{ComparisonArtifact, ResultArtifact, RunState};
use serde_json::Value;
use support::*;

#[derive(Debug, Default, World)]
struct PerformanceWorld {
    cold: Option<ResultArtifact>,
    warm: Option<ResultArtifact>,
    result: Option<ResultArtifact>,
    comparison: Option<Value>,
    validator: Option<Value>,
}

#[given("real cold and warm ResourceTracker evidence")]
fn given_cold_and_warm(world: &mut PerformanceWorld) {
    world.cold = Some(resource_exemplar(RunState::Cold));
    world.warm = Some(resource_exemplar(RunState::Warm));
}

#[then("the two runs retain distinct state and comparison identity")]
fn then_states_are_distinct(world: &mut PerformanceWorld) {
    let cold = world.cold.as_ref().unwrap();
    let warm = world.warm.as_ref().unwrap();
    assert_eq!(cold.identity.state, RunState::Cold);
    assert_eq!(warm.identity.state, RunState::Warm);
    assert_ne!(cold.comparison_key, warm.comparison_key);
}

#[then("their observed comparison is refused with a state mismatch")]
fn then_state_mismatch(world: &mut PerformanceWorld) {
    let comparison = ComparisonArtifact::observed(
        "cold-vs-warm",
        world.cold.as_ref().unwrap(),
        world.warm.as_ref().unwrap(),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(&comparison.canonical_bytes().unwrap()).unwrap();
    assert_eq!(
        value["producer_summary"]["mismatch_codes"],
        serde_json::json!(["state_mismatch"])
    );
    assert!(
        validator_report(&comparison.canonical_bytes().unwrap())["ok"]
            .as_bool()
            .unwrap()
    );
}

#[given("ordered raw integer samples with durations 1, 2, and 3 nanoseconds")]
fn given_raw_samples(world: &mut PerformanceWorld) {
    world.result = Some(deterministic_result(RunState::Warm));
}

#[when("the Rust producer summarizes the samples")]
fn when_summarized(_world: &mut PerformanceWorld) {}

#[then("nearest-rank percentiles and exact population variance are reproduced")]
fn then_exact_statistics(world: &mut PerformanceWorld) {
    let distribution = &world
        .result
        .as_ref()
        .unwrap()
        .producer_summary
        .distributions[0];
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
        validator_report(&world.result.as_ref().unwrap().canonical_bytes().unwrap())["ok"]
            .as_bool()
            .unwrap()
    );
}

#[given("a measured ResourceTracker capacity sibling")]
fn given_capacity_sibling(world: &mut PerformanceWorld) {
    world.result = Some(resource_exemplar(RunState::Warm));
}

#[then("SessionCapacity remains a semantic refusal and no deadline is recorded")]
fn then_semantic_refusal(world: &mut PerformanceWorld) {
    let summary = &world.result.as_ref().unwrap().producer_summary;
    assert_eq!(summary.outcome_counts.refused, 1);
    assert_eq!(summary.outcome_counts.deadline_exceeded, 0);
    assert_eq!(summary.refusal_groups[0].domain, "runtime.resource_tracker");
    assert_eq!(
        summary.refusal_groups[0].code,
        "session_capacity:session_7:limit_1"
    );
}

#[given("two comparable artifacts without a ratified confidence method")]
fn given_comparable_artifacts(world: &mut PerformanceWorld) {
    let baseline = deterministic_result(RunState::Warm);
    let comparison = ComparisonArtifact::observed("observed-only", &baseline, &baseline).unwrap();
    world.comparison =
        Some(serde_json::from_slice(&comparison.canonical_bytes().unwrap()).unwrap());
}

#[then("confidence remains not_evaluated with no_ratified_method")]
fn then_not_evaluated(world: &mut PerformanceWorld) {
    let comparison = world.comparison.as_ref().unwrap();
    assert_eq!(comparison["confidence"]["disposition"], "not_evaluated");
    assert_eq!(
        comparison["confidence"]["reason"]["code"],
        "no_ratified_method"
    );
}

#[given("a validator-accepted diagnostic result")]
fn given_diagnostic(world: &mut PerformanceWorld) {
    let result = diagnostic_result();
    let bytes = result.canonical_bytes().unwrap();
    assert!(validator_report(&bytes)["ok"].as_bool().unwrap());
    world.result = Some(result);
}

#[when("the diagnostic result attempts to add a ratification claim")]
fn when_ratification_injected(world: &mut PerformanceWorld) {
    let mut value: Value =
        serde_json::from_slice(&world.result.as_ref().unwrap().canonical_bytes().unwrap()).unwrap();
    value["ratification"] = serde_json::json!({"status": "ratified"});
    world.validator = Some(validator_report(&canonical(&value)));
}

#[then("the authoritative validator refuses the claim")]
fn then_ratification_refused(world: &mut PerformanceWorld) {
    let report = world.validator.as_ref().unwrap();
    assert!(!report["ok"].as_bool().unwrap());
    assert_eq!(report["errors"][0]["code"], "schema_violation");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    PerformanceWorld::run("tests/features/performance_evidence.feature").await;
}
