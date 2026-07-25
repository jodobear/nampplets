use super::*;

#[test]
fn per_napplet_concurrency_and_rate_refuse_without_queueing() {
    let limits = ResourceProviderLimits {
        maximum_requests_per_napplet_per_minute: 1,
        maximum_in_flight_urls_per_napplet: 1,
        ..ResourceProviderLimits::default()
    };
    let rig = rig_with_limits(limits);
    rig.network.set_blocking();
    let first = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "first",
            "url": "https://images.example/first",
        }),
    );
    let DispatchOutcome::Handled(first) = first else {
        panic!("request must be handled");
    };
    rig.network.wait_started();
    let DispatchOutcome::Handled(second) = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "second",
            "url": "https://images.example/second",
        }),
    ) else {
        panic!("request must be handled");
    };
    assert!(!second.is_active());
    assert_eq!(
        second.response.unwrap().decode().unwrap()["error"],
        "blocked-by-policy"
    );
    rig.registry.close_session(rig.context.id);
    rig.network.wait_finished();
    assert!(first.operation().unwrap().is_cancelled());
}
