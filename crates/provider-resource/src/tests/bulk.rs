use super::*;

#[tokio::test]
async fn bulk_preserves_order_and_per_item_failure() {
    let mut rig = rig();
    let first = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
    let third = format!("data:image/webp;base64,{}", STANDARD.encode(WEBP));
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "bulk",
            "urls": [first, "http://blocked.example/a", third],
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["ok"], true);
    assert_eq!(items[1]["ok"], false);
    assert_eq!(items[1]["error"], "unsupported-scheme");
    assert_eq!(items[2]["ok"], true);
}

#[tokio::test]
async fn bulk_enforces_its_byte_ceiling_per_item_without_discarding_later_siblings() {
    let limits = ResourceProviderLimits {
        maximum_response_bytes: 64,
        maximum_svg_bytes: 64,
        maximum_bulk_response_bytes: 24,
        maximum_blob_bytes_per_request: 128,
        ..ResourceProviderLimits::default()
    };
    let mut rig = rig_with_limits(limits);
    let full = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
    let small = format!(
        "data:image/png;base64,{}",
        STANDARD.encode(b"\x89PNG\r\n\x1a\n")
    );
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytesMany",
            "id": "bulk-bytes",
            "urls": [full.clone(), full, small],
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["ok"], true);
    assert_eq!(items[1]["ok"], false);
    assert_eq!(items[1]["error"], "quota-exceeded");
    assert_eq!(items[2]["ok"], true);
}
