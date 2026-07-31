use serde_json::json;

mod support;

use nmp_native_provider_lists::*;
use support::*;

#[test]
fn released_package_type_selector_and_item_shape_propose_the_exact_write() {
    let (provider, source) = provider_with(Vec::new());
    let (_registry, _observer) = opened_session(provider.clone());

    let mut result = call(
        &provider,
        "add",
        json!({
            "list": {"type": "mute-list"},
            "items": [{"itemType": "pubkey", "value": pubkey("b")}],
        }),
    );

    assert!(result.take_write_proposal().is_some());
    assert_eq!(
        source.read_selectors(),
        vec![ListSelector {
            kind: 10_000,
            identifier: None,
        }]
    );
    assert_eq!(source.drafted(), vec![vec![entry("b")]]);
}

#[test]
fn exactly_one_selector_coordinate_is_required() {
    let (provider, source) = provider_with(Vec::new());
    let (_registry, _observer) = opened_session(provider.clone());

    for list in [json!({}), json!({"kind": 10000, "type": "mute-list"})] {
        let answer = response(
            &provider,
            "add",
            json!({"list": list, "items": [p(&pubkey("b"))]}),
        );
        assert_eq!(answer["error"], "invalid-list-ref");
        assert_eq!(
            answer["reason"],
            "list must contain exactly one of kind or type"
        );
    }
    assert_eq!(source.reads(), 0);
}

#[test]
fn unknown_type_returns_machine_error_and_supported_candidates() {
    let (provider, source) = provider_with(Vec::new());
    let (_registry, _observer) = opened_session(provider.clone());

    let answer = response(
        &provider,
        "add",
        json!({
            "list": {"type": "not-a-list"},
            "items": [p(&pubkey("b"))],
        }),
    );

    assert_eq!(answer["error"], "unsupported-list");
    assert_eq!(
        answer["reason"],
        "this runtime does not service list type not-a-list"
    );
    assert_eq!(
        answer["supported"].as_array().unwrap().len(),
        SUPPORTED_LISTS.len()
    );
    assert_eq!(source.reads(), 0);
}

#[test]
fn private_items_are_typed_unsupported_before_any_read() {
    let (provider, source) = provider_with(Vec::new());
    let (_registry, _observer) = opened_session(provider.clone());

    let answer = response(
        &provider,
        "add",
        json!({
            "list": {"type": "mute-list"},
            "items": [{
                "itemType": "pubkey",
                "value": pubkey("b"),
                "visibility": "private",
            }],
        }),
    );

    assert_eq!(answer["error"], "private-items-unsupported");
    assert!(
        answer["reason"]
            .as_str()
            .unwrap()
            .contains("private list items")
    );
    assert_eq!(source.reads(), 0);
}

#[test]
fn only_add_with_explicit_create_true_may_create_a_missing_list() {
    let (provider, source) = provider_with(Vec::new());
    source.set_exists(false);
    let (_registry, _observer) = opened_session(provider.clone());

    let mut created = call(
        &provider,
        "add",
        json!({
            "list": follows(),
            "items": [p(&pubkey("b"))],
            "options": {"create": true},
        }),
    );
    assert!(created.take_write_proposal().is_some());

    for (action, options) in [
        ("add", None),
        ("add", Some(json!({"create": false}))),
        ("remove", Some(json!({"create": true}))),
    ] {
        let mut payload = json!({
            "list": follows(),
            "items": [p(&pubkey("c"))],
        });
        if let Some(options) = options {
            payload["options"] = options;
        }
        let answer = response(&provider, action, payload);
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["error"], "list-not-found");
    }
}

#[test]
fn metadata_options_are_typed_unsupported() {
    let (provider, source) = provider_with(Vec::new());
    let (_registry, _observer) = opened_session(provider.clone());

    let answer = response(
        &provider,
        "add",
        json!({
            "list": follows(),
            "items": [p(&pubkey("b"))],
            "options": {"create": true, "title": "Friends"},
        }),
    );

    assert_eq!(answer["error"], "unsupported");
    assert!(answer["reason"].as_str().unwrap().contains("title"));
    assert_eq!(source.reads(), 0);
}

#[test]
fn remove_without_visibility_refuses_when_private_items_cannot_be_decrypted() {
    let (provider, source) = provider_with(vec![entry("b")]);
    source.set_retained_content("nip44-ciphertext");
    let (_registry, _observer) = opened_session(provider.clone());

    let answer = response(
        &provider,
        "remove",
        json!({"list": follows(), "items": [p(&pubkey("b"))]}),
    );

    assert_eq!(answer["error"], "decrypt-failed");
    assert!(source.drafted().is_empty());
}

#[test]
fn explicit_public_remove_preserves_unreadable_private_content() {
    let (provider, source) = provider_with(vec![entry("b")]);
    source.set_retained_content("nip44-ciphertext");
    let (_registry, _observer) = opened_session(provider.clone());

    let mut result = call(
        &provider,
        "remove",
        json!({
            "list": follows(),
            "items": [{
                "itemType": "pubkey",
                "value": pubkey("b"),
                "visibility": "public",
            }],
        }),
    );

    assert!(result.take_write_proposal().is_some());
}
