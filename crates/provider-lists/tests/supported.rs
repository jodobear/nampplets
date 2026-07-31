use serde_json::json;

mod support;

use nmp_native_provider_lists::*;
use support::*;

#[test]
fn supported_answers_from_the_pinned_catalog_without_touching_the_source() {
    let (provider, source) = provider_with(Vec::new());
    let answer = response(&provider, "supported", json!({}));

    assert_eq!(answer["type"], "lists.supported.result");
    assert_eq!(answer["id"], "request-1");
    let lists = answer["lists"].as_array().unwrap();
    assert_eq!(lists.len(), SUPPORTED_LISTS.len());
    // Answering "which lists work here" must never require an account or a
    // relay read; it is a fact about this build.
    assert_eq!(source.reads(), 0);
}

#[test]
fn every_advertised_list_uses_the_exact_package_support_shape() {
    let (provider, _) = provider_with(Vec::new());
    let answer = response(&provider, "supported", json!({}));

    for list in answer["lists"].as_array().unwrap() {
        let kind = u16::try_from(list["kind"].as_u64().unwrap()).unwrap();
        let pinned = supported_list(kind).expect("advertised kind is in the catalog");
        assert_eq!(list["type"], pinned.list_type);
        assert_eq!(list["addressable"], pinned.addressable);
        assert_eq!(list["privateItems"], false);
        assert!(list.get("name").is_none());
        assert!(list.get("parameterized").is_none());
        assert!(list.get("itemTypes").is_none());
        let advertised = list["supportedItemTypes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let pinned_types = pinned
            .item_types
            .iter()
            .map(|tag| semantic_item_type(*tag).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(advertised, pinned_types);
        assert!(
            !advertised.is_empty(),
            "a list with no item type is unusable"
        );
    }
}

#[test]
fn parameterized_addressing_matches_the_replaceable_kind_range() {
    for list in SUPPORTED_LISTS {
        let expected = (30_000..40_000).contains(&list.kind);
        assert_eq!(
            list.addressable, expected,
            "kind {} addressing disagrees with its replaceable range",
            list.kind
        );
    }
}

#[test]
fn supported_refuses_an_unexpected_payload_field() {
    let (provider, _) = provider_with(Vec::new());
    let error = provider
        .call(request("supported", json!({"kind": 3})))
        .unwrap_err();

    assert!(matches!(
        error,
        nmp_native_nap_bridge::ProviderError::InvalidPayload { .. }
    ));
}

#[test]
fn an_unknown_action_is_refused_rather_than_ignored() {
    let (provider, _) = provider_with(Vec::new());
    let error = provider.call(request("replace", json!({}))).unwrap_err();

    assert!(matches!(
        error,
        nmp_native_nap_bridge::ProviderError::InvalidPayload { .. }
    ));
}

#[test]
fn the_descriptor_advertises_exactly_the_pinned_action_set() {
    let (provider, _) = provider_with(Vec::new());
    let descriptor = provider.descriptor();

    let actions = descriptor
        .actions
        .iter()
        .map(|action| action.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(actions, vec!["add", "remove", "supported"]);
    assert!(descriptor.sensitive, "list membership is social-graph data");
    // The permission review reports "this app can't tell whether that works
    // here" for any capability with no registered descriptor. Advertising a
    // definite verdict is the whole difference.
    assert_eq!(
        descriptor.platform_availability,
        nmp_native_nap_bridge::ProviderPlatformAvailability::Available
    );
    assert_eq!(descriptor.domain.as_str(), DOMAIN);
    assert_eq!(
        descriptor
            .protocol_versions
            .iter()
            .map(|version| version.as_ref())
            .collect::<Vec<_>>(),
        vec![PINNED_NAP_PROTOCOL]
    );
    assert_eq!(
        descriptor
            .dependencies
            .iter()
            .map(|dependency| dependency.as_str())
            .collect::<Vec<_>>(),
        vec!["identity", "relay"]
    );
}

#[test]
fn zero_limits_are_refused_at_construction() {
    let source: Arc<dyn ListsDataPlane> = FakeSource::new(Vec::new());
    let error = ListsProvider::new(
        source,
        ListsProviderLimits {
            maximum_request_items: 0,
            ..ListsProviderLimits::default()
        },
    )
    .unwrap_err();

    assert_eq!(error, ListsProviderBuildError::InvalidLimits);
}

fn exact_terminal_limits() -> (String, usize, ListsProviderLimits) {
    let maximum_id_bytes = 32;
    let id = "\u{0001}".repeat(maximum_id_bytes);
    let success = json!({
        "type": "lists.remove.result",
        "id": &id,
        "ok": true,
        "removed": 1,
        "skipped": 1,
    });
    let failure = json!({
        "type": "lists.remove.result",
        "id": &id,
        "ok": false,
        "removed": 0,
        "skipped": 0,
        "error": "list-unavailable",
    });
    let minimum = [success, failure]
        .iter()
        .map(|value| serde_json::to_vec(value).unwrap().len())
        .max()
        .unwrap();
    let limits = ListsProviderLimits {
        maximum_response_bytes: minimum,
        maximum_correlation_id_bytes: maximum_id_bytes,
        maximum_request_items: 1,
        ..ListsProviderLimits::default()
    };
    (id, minimum, limits)
}

#[test]
fn exact_terminal_bound_preserves_a_delivered_mutation_without_event_id() {
    let (id, minimum, limits) = exact_terminal_limits();
    let source: Arc<dyn ListsDataPlane> = FakeSource::new(vec![entry("b")]);
    let provider = ListsProvider::new(source, limits).unwrap();
    let (_registry, observer) = opened_session(provider.clone());
    let mut request = request(
        "remove",
        json!({"list": follows(), "items": [p(&pubkey("b"))]}),
    );
    request.correlation_id = Some(Arc::from(id.as_str()));
    let mut result = provider.call(request).unwrap();
    let (_write, completion, _work) = result.take_write_proposal().unwrap().into_parts();
    completion
        .into_receipt_sink()
        .push_latest(receipt("delivered"))
        .unwrap();
    let pushed = drain(&observer);
    assert_eq!(pushed.len(), 1);
    assert_eq!(
        pushed[0],
        json!({
            "type": "lists.remove.result",
            "id": id,
            "ok": true,
            "removed": 1,
            "skipped": 0,
        })
    );
    assert!(serde_json::to_vec(&pushed[0]).unwrap().len() <= minimum);
}

#[test]
fn exact_terminal_bound_preserves_a_system_refusal_and_refuses_one_less() {
    let (id, minimum, limits) = exact_terminal_limits();
    let source: Arc<dyn ListsDataPlane> = FakeSource::new(vec![entry("b")]);
    let provider = ListsProvider::new(source, limits).unwrap();
    let (_registry, _observer) = opened_session(provider.clone());
    let mut request = request(
        "remove",
        json!({"list": follows(), "items": [p(&pubkey("b"))]}),
    );
    request.correlation_id = Some(Arc::from(id.as_str()));
    let mut result = provider.call(request).unwrap();
    let response = result
        .take_write_proposal()
        .unwrap()
        .refuse_system(Arc::from("upstream failure ".repeat(minimum)))
        .expect("admitted terminal bound always carries the typed refusal");
    assert_eq!(response.byte_len(), minimum);
    assert_eq!(
        response.decode().unwrap(),
        json!({
            "type": "lists.remove.result",
            "id": id,
            "ok": false,
            "removed": 0,
            "skipped": 0,
            "error": "list-unavailable",
        })
    );

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

/// The catalog is a compatibility surface: a napplet matches on these names.
#[test]
fn catalog_types_and_kinds_are_unique() {
    let mut kinds = std::collections::BTreeSet::new();
    let mut list_types = std::collections::BTreeSet::new();
    for list in SUPPORTED_LISTS {
        assert!(kinds.insert(list.kind), "duplicate kind {}", list.kind);
        assert!(
            list_types.insert(list.list_type),
            "duplicate type {}",
            list.list_type
        );
    }
}
