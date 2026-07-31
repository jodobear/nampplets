use serde_json::{Value, json};

use super::*;

#[derive(Debug)]
struct AllowSpecificIntent;

impl IntentPolicy for AllowSpecificIntent {
    fn evaluate(&self, _request: &IntentPolicyRequest) -> IntentPolicyDecision {
        IntentPolicyDecision {
            allow: true,
            allow_specific_handler: true,
            confirmation_required: true,
            reveal_candidates: true,
        }
    }

    fn allow_discovery(&self, _caller: &Principal, _archetype: &str) -> bool {
        true
    }
}

#[test]
fn omitted_legacy_identity_fields_are_defaulted_before_native_dispatch() {
    let rig = Rig::new(Arc::new(CancelIntentChoice));
    let handler = principal("note-viewer", 'c');
    rig.provider
        .register_handler(handler.clone(), vec![note_declaration()])
        .unwrap();
    rig.provider.set_default("note", Some(handler)).unwrap();
    let _ = rig.observer.drain(16).unwrap();

    assert_eq!(
        rig.dispatch(json!({
            "type":"intent.invoke",
            "id":"invoke-defaults-1",
            "request":{"archetype":"note"}
        }))
        .unwrap(),
        None
    );

    let requests = rig.dispatcher.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].action.as_ref(), "open");
    assert_eq!(requests[0].convention.as_deref(), Some("napplet:note/open"));
}

#[test]
fn explicit_default_handler_value_routes_the_declared_default() {
    let rig = Rig::new(Arc::new(CancelIntentChoice));
    let handler = principal("note-viewer", 'c');
    rig.provider
        .register_handler(handler.clone(), vec![note_declaration()])
        .unwrap();
    rig.provider
        .set_default("note", Some(handler.clone()))
        .unwrap();
    let _ = rig.observer.drain(16).unwrap();

    assert_eq!(
        rig.dispatch(json!({
            "type":"intent.invoke",
            "id":"invoke-explicit-default-1",
            "request":{"archetype":"note","handler":"default"}
        }))
        .unwrap(),
        None
    );
    let requests = rig.dispatcher.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].handler, handler);
}

#[test]
fn non_string_handler_values_are_rejected_before_default_resolution() {
    let rig = Rig::new(Arc::new(CancelIntentChoice));
    let handler = principal("note-viewer", 'c');
    rig.provider
        .register_handler(handler.clone(), vec![note_declaration()])
        .unwrap();
    rig.provider.set_default("note", Some(handler)).unwrap();
    let _ = rig.observer.drain(16).unwrap();

    for invalid_handler in [
        json!(7),
        Value::Null,
        json!({"dTag":"note-viewer"}),
        json!(["note-viewer"]),
        json!(true),
    ] {
        assert!(
            rig.dispatch(json!({
                "type":"intent.invoke",
                "id":"invoke-invalid-handler",
                "request":{"archetype":"note","handler":invalid_handler}
            }))
            .is_err()
        );
    }
    assert!(rig.dispatcher.requests.lock().is_empty());
}

#[test]
fn declared_explicit_handler_routes_when_policy_allows_specific_targeting() {
    let rig = Rig::new_with_policy(Arc::new(CancelIntentChoice), Arc::new(AllowSpecificIntent));
    let handler = principal("note-viewer", 'c');
    rig.provider
        .register_handler(handler.clone(), vec![note_declaration()])
        .unwrap();
    let _ = rig.observer.drain(16).unwrap();

    assert_eq!(
        rig.dispatch(json!({
            "type":"intent.invoke",
            "id":"invoke-specific-handler-1",
            "request":{"archetype":"note","handler":"note-viewer"}
        }))
        .unwrap(),
        None
    );
    let requests = rig.dispatcher.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].handler, handler);
}

#[test]
fn protocol_field_is_accepted_as_a_convention_alias() {
    let rig = Rig::new(Arc::new(CancelIntentChoice));
    let handler = principal("note-viewer", 'c');
    rig.provider
        .register_handler(handler.clone(), vec![note_declaration()])
        .unwrap();
    rig.provider
        .set_default("note", Some(handler.clone()))
        .unwrap();
    let _ = rig.observer.drain(16).unwrap();

    assert_eq!(
        rig.dispatch(json!({
            "type":"intent.invoke",
            "id":"invoke-protocol-1",
            "request":{
                "archetype":"note",
                "action":"open",
                "protocol":"napplet:note/open",
                "payload":{"target":"abc"}
            }
        }))
        .unwrap(),
        None
    );
    let requests = rig.dispatcher.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].handler, handler);
}

#[test]
fn explicit_alternate_convention_routes_by_handler_declaration() {
    let rig = Rig::new(Arc::new(CancelIntentChoice));
    let legacy = principal("note-viewer", 'c');
    let alternate = principal("article-reader", 'd');
    rig.provider
        .register_handler(legacy.clone(), vec![note_declaration()])
        .unwrap();
    rig.provider
        .register_handler(
            alternate.clone(),
            vec![IntentHandlerDeclaration {
                archetype: Arc::from("note"),
                title: Some(Arc::from("Article Reader")),
                actions: BTreeSet::from([Arc::from("open")]),
                conventions: BTreeSet::from([Arc::from("napplet:article/read")]),
            }],
        )
        .unwrap();
    rig.provider.set_default("note", Some(legacy)).unwrap();
    let _ = rig.observer.drain(16).unwrap();

    assert_eq!(
        rig.dispatch(json!({
            "type":"intent.invoke",
            "id":"invoke-alternate-1",
            "request":{
                "archetype":"note",
                "action":"open",
                "convention":"napplet:article/read",
                "payload":{"target":"abc"}
            }
        }))
        .unwrap(),
        None
    );
    let requests = rig.dispatcher.requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].handler, alternate);
    assert_eq!(requests[0].archetype.as_ref(), "note");
    assert_eq!(requests[0].action.as_ref(), "open");
    assert_eq!(
        requests[0].convention.as_deref(),
        Some("napplet:article/read")
    );
}

#[test]
fn undeclared_choice_and_specific_target_execute_nothing() {
    let rig = Rig::new(Arc::new(FixedChoice(Arc::from("spoofed"))));
    rig.provider
        .register_handler(principal("note-viewer", 'c'), vec![note_declaration()])
        .unwrap();
    let _ = rig.observer.drain(8).unwrap();
    let choose = rig
        .dispatch(json!({
            "type":"intent.invoke",
            "id":"choose-1",
            "request":{"archetype":"note","handler":"choose"}
        }))
        .unwrap()
        .unwrap();
    assert_eq!(choose["result"]["handled"], false);
    assert_eq!(choose["result"]["error"], "no handler");

    let specific = rig
        .dispatch(json!({
            "type":"intent.invoke",
            "id":"specific-1",
            "request":{"archetype":"note","handler":"note-viewer"}
        }))
        .unwrap()
        .unwrap();
    assert_eq!(specific["result"]["error"], "invoke denied");
    assert!(rig.dispatcher.requests.lock().is_empty());
}

#[test]
fn malformed_convention_and_behavior_are_rejected_before_dispatch() {
    let rig = Rig::new(Arc::new(CancelIntentChoice));
    for request in [
        json!({"archetype":"note","convention":"https://example.com"}),
        json!({"archetype":"note","convention":"napplet:note"}),
        json!({"archetype":"note","convention":"napplet:note/open/extra"}),
        json!({
            "archetype":"note",
            "action":"open",
            "convention":"napplet:note/open",
            "protocol":"napplet:note/edit"
        }),
        json!({"archetype":"note","convention":"napplet:note/open?draft=true"}),
        json!({"archetype":"note","convention":"napplet:note/open#fragment"}),
        json!({"archetype":"Note"}),
        json!({"archetype":"note","behavior":{"focus":"yes"}}),
        json!({"archetype":"note","unknown":true}),
    ] {
        assert!(
            rig.dispatch(json!({
                "type":"intent.invoke",
                "id":"bad",
                "request":request
            }))
            .is_err()
        );
    }
    assert!(rig.dispatcher.requests.lock().is_empty());
}
