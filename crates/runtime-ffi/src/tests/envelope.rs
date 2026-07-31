use super::*;

#[test]
fn session_change_is_projected_as_typed_lifecycle_evidence() {
    let principal = Principal::new("a".repeat(64), "app", "b".repeat(64)).unwrap();
    let event = PlatformEvent::SessionChanged(nmp_native_runtime_core::SessionSnapshot {
        id: SessionId(5),
        principal,
        profile: nmp_native_runtime_core::ExecutionProfile::Renderer,
        state: nmp_native_runtime_core::SessionState::Stopped,
    });

    let projected = project_event(10, &event);
    assert_eq!(projected.kind, "session-changed");
    assert_eq!(projected.detail, "stopped");
    assert_eq!(projected.session_id, Some(5));
    assert_eq!(projected.response_json, None);
}

#[test]
fn envelope_response_is_projected_as_exact_machine_readable_json() {
    let response =
        BoundedJson::from_raw(r#"{"type":"shell.init","capabilities":{}}"#, 1_024).unwrap();
    let event = PlatformEvent::EnvelopeHandled {
        session: SessionId(7),
        operation: None,
        response: Some(response.clone()),
    };

    let projected = project_event(11, &event);
    assert_eq!(projected.kind, "envelope-handled");
    assert_eq!(projected.session_id, Some(7));
    assert_eq!(projected.response_json.as_deref(), Some(response.as_str()));
}

#[test]
fn provider_push_is_projected_as_exact_machine_readable_json() {
    let envelope =
        BoundedJson::from_raw(r#"{"type":"identity.changed","pubkey":"abc"}"#, 1_024).unwrap();
    let event = PlatformEvent::ProviderPush {
        session: SessionId(9),
        source_window: nmp_native_nap_bridge::SourceWindowId(3),
        provider_sequence: 4,
        domain: Capability::new("identity").unwrap(),
        envelope: envelope.clone(),
    };

    let projected = project_event(12, &event);
    assert_eq!(projected.kind, "provider-push");
    assert_eq!(projected.session_id, Some(9));
    assert_eq!(projected.response_json.as_deref(), Some(envelope.as_str()));
}
