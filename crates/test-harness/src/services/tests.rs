use std::{path::Path, sync::Arc};

use super::*;

fn catalog() -> ScenarioCatalog {
    ScenarioCatalog::from_json(include_bytes!(
        "../../../../conformance/test-services/scenarios.json"
    ))
    .unwrap()
}

fn fixture_loader() -> Arc<dyn FixtureLoader> {
    Arc::new(FsFixtureLoader::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/napplet-corpus"),
        1024 * 1024,
    ))
}

#[test]
fn relay_script_is_manual_and_returns_resources_to_baseline() {
    let catalog = catalog();
    let clock = Arc::new(ManualClock::new(catalog.clock.initial_unix_seconds));
    let relay = RelayScenarioService::new(&catalog, Arc::clone(&clock));
    {
        let mut connection = relay.connect("expiry").unwrap();
        assert_eq!(connection.next_action().unwrap(), Some(RelayAction::Accept));
        assert!(matches!(
            connection.next_action().unwrap(),
            Some(RelayAction::Event(_))
        ));
        assert_eq!(
            connection.next_action().unwrap(),
            Some(RelayAction::ClockAdvanced {
                seconds: 61,
                now: catalog.clock.initial_unix_seconds + 61,
            })
        );
        assert_eq!(relay.census().active, 1);
    }
    assert_eq!(relay.census().active, 0);
}

#[test]
fn blob_corruption_and_slow_chunks_are_deterministic_and_finite() {
    let catalog = catalog();
    let clock = Arc::new(ManualClock::new(catalog.clock.initial_unix_seconds));
    let blobs = BlobScenarioService::new(&catalog, Arc::clone(&clock), fixture_loader());
    let mut verified = blobs.request("verified-index").unwrap().body;
    let mut corrupt = blobs.request("one-byte-corrupt").unwrap().body;
    assert_ne!(
        verified.next_chunk().unwrap().as_ref(),
        corrupt.next_chunk().unwrap().as_ref()
    );
    drop((verified, corrupt));

    let mut slow = blobs.request("slow-stream").unwrap().body;
    assert_eq!(slow.next_chunk().unwrap().len(), 1);
    assert_eq!(clock.now(), catalog.clock.initial_unix_seconds + 1);
    drop(slow);
    assert_eq!(blobs.census().active, 0);
}

#[test]
fn catalog_enforces_no_secret_key_policy() {
    let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../../conformance/test-services/scenarios.json"
    ))
    .unwrap();
    value["secrets"]["fixture_policy"] = serde_json::json!("contains-keys");
    assert!(matches!(
        ScenarioCatalog::from_json(&serde_json::to_vec(&value).unwrap()),
        Err(DeterministicServiceError::InvalidCatalog(_))
    ));
}

#[test]
fn signer_rejection_is_typed_and_never_loads_a_key() {
    let catalog = catalog();
    let signer = SignerScenarioService::new(&catalog, fixture_loader());
    assert_eq!(
        signer
            .request("user-rejected", "unsigned-public-note")
            .unwrap(),
        SignerOutcome::Rejected
    );
    assert_eq!(signer.census().active, 0);
}
