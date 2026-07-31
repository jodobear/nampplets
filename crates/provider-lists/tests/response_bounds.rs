use serde_json::{Value, json};

mod support;

use nmp_native_provider_lists::*;
use support::*;

fn correlated_response(provider: &ListsProvider, action: &str, payload: Value, id: &str) -> Value {
    let mut request = request(action, payload);
    request.correlation_id = Some(Arc::from(id));
    provider
        .call(request)
        .expect("an admitted response bound completes the call")
        .response
        .expect("the call has an immediate correlated response")
        .decode()
        .unwrap()
}

fn exact_catalog_bound(maximum_id_bytes: usize, id: &str) -> usize {
    let (provider, _) = provider_with(Vec::new());
    let (_registry, _observer) = opened_session(provider.clone());
    let candidates = [
        correlated_response(&provider, "supported", json!({}), id),
        correlated_response(
            &provider,
            "add",
            json!({"list": {"kind": u16::MAX}, "items": [p(&pubkey("b"))]}),
            id,
        ),
        correlated_response(
            &provider,
            "remove",
            json!({"list": {"kind": u16::MAX}, "items": [p(&pubkey("b"))]}),
            id,
        ),
    ];
    assert_eq!(id.len(), maximum_id_bytes);
    candidates
        .iter()
        .map(|response| serde_json::to_vec(response).unwrap().len())
        .max()
        .unwrap()
}

#[test]
fn exact_catalog_bound_admits_every_bounded_catalog_response_and_refuses_one_less() {
    let maximum_id_bytes = 32;
    let id = "\u{0001}".repeat(maximum_id_bytes);
    let minimum = exact_catalog_bound(maximum_id_bytes, &id);
    let limits = ListsProviderLimits {
        maximum_response_bytes: minimum,
        maximum_correlation_id_bytes: maximum_id_bytes,
        ..ListsProviderLimits::default()
    };

    let source: Arc<dyn ListsDataPlane> = FakeSource::new(Vec::new());
    let provider = ListsProvider::new(source, limits).unwrap();
    let (_registry, _observer) = opened_session(provider.clone());
    for (action, payload) in [
        ("supported", json!({})),
        (
            "add",
            json!({"list": {"kind": u16::MAX}, "items": [p(&pubkey("b"))]}),
        ),
        (
            "remove",
            json!({"list": {"kind": u16::MAX}, "items": [p(&pubkey("b"))]}),
        ),
    ] {
        let answer = correlated_response(&provider, action, payload, &id);
        assert!(serde_json::to_vec(&answer).unwrap().len() <= minimum);
        let catalog = answer
            .get(if action == "supported" {
                "lists"
            } else {
                "supported"
            })
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(catalog.len(), SUPPORTED_LISTS.len());
    }

    let source: Arc<dyn ListsDataPlane> = FakeSource::new(Vec::new());
    assert_eq!(
        ListsProvider::new(
            source,
            ListsProviderLimits {
                maximum_response_bytes: minimum - 1,
                ..limits
            }
        )
        .unwrap_err(),
        ListsProviderBuildError::InvalidLimits
    );
}

#[test]
fn oversized_unsupported_type_keeps_a_correlated_typed_catalog_refusal() {
    let maximum_id_bytes = 32;
    let id = "\u{0001}".repeat(maximum_id_bytes);
    let minimum = exact_catalog_bound(maximum_id_bytes, &id);
    let limits = ListsProviderLimits {
        maximum_response_bytes: minimum,
        maximum_correlation_id_bytes: maximum_id_bytes,
        ..ListsProviderLimits::default()
    };
    let source: Arc<dyn ListsDataPlane> = FakeSource::new(Vec::new());
    let provider = ListsProvider::new(source, limits).unwrap();
    let (_registry, _observer) = opened_session(provider.clone());
    let catalog = correlated_response(&provider, "supported", json!({}), &id)["lists"].clone();

    let answer = correlated_response(
        &provider,
        "add",
        json!({
            "list": {"type": "x".repeat(minimum * 2)},
            "items": [p(&pubkey("b"))],
        }),
        &id,
    );

    assert_eq!(
        answer,
        json!({
            "type": "lists.add.result",
            "id": id,
            "ok": false,
            "added": 0,
            "skipped": 0,
            "error": "unsupported-list",
            "supported": catalog,
        })
    );
    assert!(serde_json::to_vec(&answer).unwrap().len() <= minimum);
}
