use super::*;

#[tokio::test]
async fn raw_svg_is_only_delivered_after_bounded_no_network_rasterization() {
    let mut rig = rig();
    let svg = b"<?xml version=\"1.0\"?><svg><script>alert(1)</script></svg>";
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "svg",
            "url": format!("data:image/png;base64,{}", STANDARD.encode(svg)),
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    assert_eq!(value["mime"], "image/webp");
    assert_eq!(
        STANDARD.decode(value["blob"].as_str().unwrap()).unwrap(),
        WEBP
    );
    let calls = rig.rasterizer.calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source.as_ref(), svg);
    assert_eq!(calls[0].maximum_dimension, 4_096);
}

#[tokio::test]
async fn blossom_hash_is_verified_before_mime_delivery() {
    let mut rig = rig();
    let digest = hex::encode(Sha256::digest(PNG));
    rig.network.respond(
        &format!("https://blossom.example/files/{digest}"),
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
            "id": "blossom-ok",
            "url": format!("blossom:sha256:{digest}"),
        }),
    );
    assert_eq!(
        terminal(&mut rig, outcome).await["type"],
        "resource.bytes.result"
    );

    let wrong = "0".repeat(64);
    rig.network.respond(
        &format!("https://blossom.example/files/{wrong}"),
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
            "id": "blossom-bad",
            "url": format!("blossom:sha256:{wrong}"),
        }),
    );
    assert_eq!(terminal(&mut rig, outcome).await["error"], "decode-failed");
}
