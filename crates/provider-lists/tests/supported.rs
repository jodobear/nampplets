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
