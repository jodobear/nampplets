use super::*;

#[tokio::test]
async fn https_resolution_is_pinned_and_redirect_is_rechecked() {
    let mut rig = rig();
    rig.network.respond(
        "https://images.example/a",
        RawHttpsResponse {
            status: 302,
            location: Some(Arc::from("https://cdn.example/b#redirect-fragment")),
            body: Vec::new(),
        },
    );
    rig.network.respond(
        "https://cdn.example/b",
        RawHttpsResponse {
            status: 200,
            location: None,
            body: PNG.to_vec(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "https-1",
            "url": "https://images.example/a#napplet-local-fragment",
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    assert_eq!(value["type"], "resource.bytes.result");
    let requests = rig.network.requests.lock();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.as_ref(), "https://images.example/a");
    assert_eq!(requests[1].url.as_ref(), "https://cdn.example/b");
    assert_eq!(requests[0].host.as_ref(), "images.example");
    assert_eq!(requests[1].host.as_ref(), "cdn.example");
    assert_eq!(
        requests[0].approved_addresses.as_ref(),
        &["1.1.1.1".parse::<IpAddr>().unwrap()]
    );
}

#[tokio::test]
async fn private_or_link_local_dns_answers_fail_before_transport() {
    for address in ["127.0.0.1", "10.0.0.1", "169.254.169.254", "::1", "fc00::1"] {
        let mut rig = rig();
        rig.network.set_addresses(&[address]);
        let outcome = dispatch(
            &rig,
            json!({
                "type": "resource.bytes",
                "id": format!("blocked-{address}"),
                "url": "https://images.example/a",
            }),
        );
        let value = terminal(&mut rig, outcome).await;
        assert_eq!(value["error"], "blocked-by-policy");
        assert!(rig.network.requests.lock().is_empty());
    }
}

#[tokio::test]
async fn oversized_dns_answer_set_is_refused_before_transport() {
    let limits = ResourceProviderLimits {
        maximum_resolved_addresses: 1,
        ..ResourceProviderLimits::default()
    };
    let mut rig = rig_with_limits(limits);
    rig.network.set_addresses(&["1.1.1.1", "8.8.8.8"]);
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "dns-cap",
            "url": "https://images.example/a",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "too-large");
    assert!(rig.network.requests.lock().is_empty());
}

#[tokio::test]
async fn http_redirect_and_credentialed_url_are_refused() {
    let mut rig = rig();
    rig.network.respond(
        "https://images.example/a",
        RawHttpsResponse {
            status: 302,
            location: Some(Arc::from("http://127.0.0.1/admin")),
            body: Vec::new(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "redirect-1",
            "url": "https://images.example/a",
        }),
    );
    assert_eq!(
        terminal(&mut rig, outcome).await["error"],
        "blocked-by-policy"
    );

    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "credential-1",
            "url": "https://user:secret@images.example/a",
        }),
    );
    assert_eq!(
        terminal(&mut rig, outcome).await["error"],
        "blocked-by-policy"
    );
}

#[tokio::test]
async fn redirect_limit_and_missing_location_are_typed_failures() {
    let limits = ResourceProviderLimits {
        maximum_redirects: 1,
        ..ResourceProviderLimits::default()
    };
    let mut limited_rig = rig_with_limits(limits);
    limited_rig.network.respond(
        "https://images.example/a",
        RawHttpsResponse {
            status: 302,
            location: Some(Arc::from("https://images.example/b")),
            body: Vec::new(),
        },
    );
    limited_rig.network.respond(
        "https://images.example/b",
        RawHttpsResponse {
            status: 302,
            location: Some(Arc::from("https://images.example/c")),
            body: Vec::new(),
        },
    );
    let outcome = dispatch(
        &limited_rig,
        json!({
            "type": "resource.bytes",
            "id": "redirect-cap",
            "url": "https://images.example/a",
        }),
    );
    assert_eq!(
        terminal(&mut limited_rig, outcome).await["error"],
        "blocked-by-policy"
    );
    assert_eq!(limited_rig.network.requests.lock().len(), 2);

    let mut rig = rig();
    rig.network.respond(
        "https://images.example/missing-location",
        RawHttpsResponse {
            status: 302,
            location: None,
            body: Vec::new(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "missing-location",
            "url": "https://images.example/missing-location",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "network-error");
}

#[tokio::test]
async fn oversize_and_mime_confusion_are_typed_failures() {
    let limits = ResourceProviderLimits {
        maximum_response_bytes: 64,
        maximum_svg_bytes: 64,
        maximum_bulk_response_bytes: 128,
        maximum_blob_bytes_per_request: 128,
        ..ResourceProviderLimits::default()
    };
    let mut rig = rig_with_limits(limits);
    rig.network.respond(
        "https://images.example/large",
        RawHttpsResponse {
            status: 200,
            location: None,
            body: vec![0_u8; 65],
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "large",
            "url": "https://images.example/large",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "too-large");

    rig.network.respond(
        "https://images.example/html",
        RawHttpsResponse {
            status: 200,
            location: None,
            body: b"<html>attacker-labelled image/png</html>".to_vec(),
        },
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "mime",
            "url": "https://images.example/html",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "decode-failed");
}

#[tokio::test]
async fn network_timeout_remains_distinct_from_other_network_failure() {
    let mut rig = rig();
    rig.network
        .fail("https://images.example/slow", ResourceNetworkError::Timeout);
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "timeout",
            "url": "https://images.example/slow",
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "timeout");
}
