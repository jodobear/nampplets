use super::*;

#[tokio::test]
async fn data_url_is_decoded_sniffed_and_projected_as_base64_bstr() {
    let mut rig = rig();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "data-1",
            "url": format!("data:text/html;base64,{}", STANDARD.encode(PNG)),
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    assert_eq!(value["type"], "resource.bytes.result");
    assert_eq!(value["mime"], "image/png");
    assert_eq!(
        STANDARD.decode(value["blob"].as_str().unwrap()).unwrap(),
        PNG
    );
    assert!(rig.network.requests.lock().is_empty());
}

#[tokio::test]
async fn data_url_scheme_and_base64_follow_the_pinned_web_semantics() {
    let mut rig = rig();
    let bytes = [PNG, b"x"].concat();
    let encoded = STANDARD.encode(&bytes);
    let encoded = encoded.replacen("", "%0A", 1).replace('=', "%3D");
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "data-normalized",
            "url": format!("DATA:image/png;BASE64,{encoded}"),
        }),
    );
    let value = terminal(&mut rig, outcome).await;
    assert_eq!(value["type"], "resource.bytes.result");
    assert_eq!(
        STANDARD.decode(value["blob"].as_str().unwrap()).unwrap(),
        bytes
    );

    let unpadded = STANDARD.encode(&bytes).trim_end_matches('=').to_owned();
    let outcome = dispatch(
        &rig,
        json!({
            "type": "resource.bytes",
            "id": "data-unpadded",
            "url": format!("data:image/png;base64,{unpadded}"),
        }),
    );
    assert_eq!(
        terminal(&mut rig, outcome).await["type"],
        "resource.bytes.result"
    );
    assert!(rig.network.requests.lock().is_empty());
}
