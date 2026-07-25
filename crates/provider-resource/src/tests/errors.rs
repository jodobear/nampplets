use super::*;

#[test]
fn empty_or_over_cap_bulk_gets_one_top_level_error() {
    let rig = rig();
    let DispatchOutcome::Handled(call) = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "empty",
            "urls": [],
        }),
    ) else {
        panic!("request must be handled");
    };
    assert!(!call.is_active());
    assert_eq!(
        call.response.unwrap().decode().unwrap()["error"],
        "invalid-request"
    );

    let urls = (0..=ResourceProviderLimits::default().maximum_urls_per_bulk)
        .map(|index| format!("https://images.example/{index}"))
        .collect::<Vec<_>>();
    let DispatchOutcome::Handled(call) = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "over-cap",
            "urls": urls,
        }),
    ) else {
        panic!("request must be handled");
    };
    assert!(!call.is_active());
    let value = call.response.unwrap().decode().unwrap();
    assert_eq!(value["type"], "resource.bytesMany.error");
    assert_eq!(value["error"], "too-large");
}

#[test]
fn correlated_malformed_requests_get_typed_action_error_terminals() {
    let rig = rig();
    for (request, expected_type) in [
        (
            json!({
                "type": "resource.info",
                "id": "bad-info",
                "unexpected": true,
            }),
            "resource.info.error",
        ),
        (
            json!({
                "type": "resource.bytes",
                "id": "bad-bytes",
                "url": 42,
            }),
            "resource.bytes.error",
        ),
        (
            json!({
                "type": "resource.bytesMany",
                "id": "bad-many-shape",
                "urls": "not-an-array",
            }),
            "resource.bytesMany.error",
        ),
        (
            json!({
                "type": "resource.bytesMany",
                "id": "bad-many-item",
                "urls": ["https://images.example/a", 42],
            }),
            "resource.bytesMany.error",
        ),
    ] {
        let DispatchOutcome::Handled(call) = dispatch(&rig, request) else {
            panic!("well-correlated malformed request must be handled");
        };
        assert!(!call.is_active());
        let value = call.response.unwrap().decode().unwrap();
        assert_eq!(value["type"], expected_type);
        assert_eq!(value["error"], "invalid-request");
    }
}

#[test]
fn provider_rejects_invalid_configuration_before_advertisement() {
    let network: Arc<dyn ResourceNetwork> = Arc::new(FakeNetwork::default());
    let rasterizer: Arc<dyn SvgRasterizer> = Arc::new(FakeRasterizer::default());
    let clock: Arc<dyn ResourceClock> = Arc::new(FakeClock::default());
    let activity: Arc<dyn ResourceActivitySink> = Arc::new(FakeActivity::default());
    assert!(matches!(
        ResourceProvider::new(
            Arc::clone(&network),
            Arc::clone(&rasterizer),
            Arc::clone(&clock),
            Arc::clone(&activity),
            ResourceProviderLimits::default(),
            std::iter::empty::<Arc<str>>(),
        ),
        Err(ResourceProviderBuildError::MissingBlossomServer)
    ));
    assert!(matches!(
        ResourceProvider::new(
            network,
            rasterizer,
            clock,
            activity,
            ResourceProviderLimits::default(),
            ["http://localhost/"],
        ),
        Err(ResourceProviderBuildError::InvalidBlossomServer { .. })
    ));
}
