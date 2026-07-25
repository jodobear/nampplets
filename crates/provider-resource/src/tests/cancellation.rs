use super::*;

#[test]
fn crash_or_revoke_cancels_blocking_fetch_and_drops_late_terminal() {
    let rig = rig();
    rig.network.set_blocking();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "slow",
            "url": "https://images.example/slow",
        }),
    );
    let DispatchOutcome::Handled(call) = outcome else {
        panic!("request must be handled");
    };
    assert!(call.is_active());
    rig.network.wait_started();
    rig.registry.close_session_with_reason(
        rig.context.id,
        nmp_native_nap_bridge::ProviderSessionEnd::Crashed,
    );
    rig.network.wait_finished();
    assert!(call.operation().unwrap().is_cancelled());
    assert_eq!(rig.provider.census().active_requests, 0);
    assert!(rig.observer.drain(8).unwrap().pushes.is_empty());
}

#[test]
fn explicit_cancel_is_idempotent_and_activity_facts_omit_urls() {
    let rig = rig();
    rig.network.set_blocking();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "cancel-me",
            "url": "https://secret-interest.example/avatar",
        }),
    );
    let DispatchOutcome::Handled(call) = outcome else {
        panic!("request must be handled");
    };
    rig.network.wait_started();
    let DispatchOutcome::Handled(cancel) = dispatch(
        &rig,
        json!({
            "type": "resource.cancel",
            "id": "cancel-me",
        }),
    ) else {
        panic!("cancel must be handled");
    };
    assert!(!cancel.is_active());
    rig.network.wait_finished();
    rig.activity.wait_for(
        ResourceActivityAction::Bytes,
        ResourceActivityOutcome::Cancelled,
    );
    assert!(call.operation().unwrap().is_cancelled());
    assert!(rig.observer.drain(8).unwrap().pushes.is_empty());
    let facts = rig.activity.0.lock();
    assert!(facts.iter().any(|fact| {
        fact.action == ResourceActivityAction::Cancel
            && fact.outcome == ResourceActivityOutcome::Cancelled
    }));
}

#[test]
fn bulk_cancel_drops_partial_results_and_releases_the_full_reservation() {
    let rig = rig();
    rig.network.set_blocking();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "cancel-bulk",
            "urls": [
                "https://images.example/one",
                "https://images.example/two",
            ],
        }),
    );
    let DispatchOutcome::Handled(call) = outcome else {
        panic!("request must be handled");
    };
    rig.network.wait_started();
    let DispatchOutcome::Handled(cancel) = dispatch(
        &rig,
        json!({
            "type": "resource.cancel",
            "id": "cancel-bulk",
        }),
    ) else {
        panic!("cancel must be handled");
    };
    assert!(!cancel.is_active());
    rig.network.wait_finished();
    rig.activity.wait_for(
        ResourceActivityAction::BytesMany,
        ResourceActivityOutcome::Cancelled,
    );
    assert!(call.operation().unwrap().is_cancelled());
    assert_eq!(rig.provider.census().active_requests, 0);
    assert_eq!(rig.provider.census().in_flight_urls, 0);
    assert!(rig.observer.drain(8).unwrap().pushes.is_empty());
}
