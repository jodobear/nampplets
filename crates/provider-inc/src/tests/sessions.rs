//! Session, subscription, and emit behaviour.

use super::*;
use crate::*;

#[test]
fn descriptor_is_complete_and_registerable_only_with_valid_limits() {
    let provider = IncProvider::new(
        Arc::new(AllowAllIncAcl),
        Arc::new(NoopIncActivity),
        IncProviderLimits::default(),
    )
    .unwrap();
    assert_eq!(provider.descriptor.domain.as_str(), DOMAIN);
    assert!(provider.descriptor.sensitive);
    assert_eq!(
        provider.descriptor.protocol_versions,
        BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)])
    );
    assert_eq!(
        provider.descriptor.actions,
        [
            "emit",
            "subscribe",
            "unsubscribe",
            "channel.open",
            "channel.list",
            "channel.broadcast",
            "channel.emit",
            "channel.close",
        ]
        .into_iter()
        .map(Arc::from)
        .collect()
    );
    let limits = IncProviderLimits {
        maximum_total_channels: 0,
        ..IncProviderLimits::default()
    };
    assert_eq!(
        IncProvider::new(Arc::new(AllowAllIncAcl), Arc::new(NoopIncActivity), limits).unwrap_err(),
        IncProviderBuildError::InvalidLimits
    );
}

#[test]
fn topics_use_exact_pinned_envelopes_and_source_derived_sender() {
    let mut harness = default_harness();
    harness.open(1, "reader", 'a');
    harness.open(2, "writer", 'b');
    harness.dispatch(
        1,
        json!({"type":"inc.subscribe","id":"sub-1","topic":"profile:open"}),
    );
    assert_eq!(
        harness
            .dispatch(
                1,
                json!({"type":"inc.subscribe","id":"sub-2","topic":"profile:open"})
            )
            .unwrap(),
        json!({"type":"inc.subscribe.result","id":"sub-2"})
    );
    harness.dispatch(
        2,
        json!({
            "type":"inc.emit",
            "topic":"profile:open",
            "payload":{"pubkey":"abc"},
        }),
    );
    assert_eq!(
        harness.drain(1),
        vec![json!({
            "type":"inc.event",
            "topic":"profile:open",
            "sender":"writer",
            "payload":{"pubkey":"abc"},
        })]
    );
    assert!(harness.drain(2).is_empty(), "sender must be excluded");

    harness.dispatch(1, json!({"type":"inc.unsubscribe","topic":"profile:open"}));
    harness.dispatch(
        2,
        json!({"type":"inc.emit","topic":"profile:open","payload":null}),
    );
    assert!(harness.drain(1).is_empty());

    assert!(matches!(
        harness.dispatch_error(
            2,
            json!({
                "type":"inc.emit",
                "topic":"profile:open",
                "sender":"spoofed",
                "sessionId":1,
            })
        ),
        BridgeError::Provider(ProviderError::InvalidPayload { .. })
    ));
}

#[test]
fn topic_acl_capacity_and_payload_bounds_fail_closed() {
    let limits = IncProviderLimits {
        maximum_subscriptions_per_session: 1,
        maximum_total_subscriptions: 2,
        maximum_payload_bytes: 64,
        maximum_response_bytes: 512,
        maximum_json_depth: 2,
        maximum_container_items: 2,
        maximum_string_bytes: 64,
        maximum_topic_bytes: 32,
        ..IncProviderLimits::default()
    };
    let mut harness = Harness::new(limits, 64);
    harness.open(1, "one", 'a');
    harness.open(2, "two", 'b');

    harness.dispatch(1, json!({"type":"inc.subscribe","id":"a","topic":"first"}));
    assert_eq!(
        harness
            .dispatch(1, json!({"type":"inc.subscribe","id":"b","topic":"second"}))
            .unwrap(),
        json!({
            "type":"inc.subscribe.result",
            "id":"b",
            "error":"subscription capacity is full",
        })
    );
    harness.acl.deny_topics.store(true, Ordering::Release);
    assert_eq!(
        harness
            .dispatch(2, json!({"type":"inc.subscribe","id":"c","topic":"first"}))
            .unwrap(),
        json!({
            "type":"inc.subscribe.result",
            "id":"c",
            "error":"topic rejected by ACL",
        })
    );
    assert!(matches!(
        harness.dispatch_error(
            1,
            json!({
                "type":"inc.emit",
                "topic":"first",
                "payload":{"a":{"b":{"c":1}}},
            })
        ),
        BridgeError::Provider(ProviderError::InvalidPayload { .. })
    ));
    assert!(matches!(
        harness.dispatch_error(
            1,
            json!({"type":"inc.emit","id":"forbidden","topic":"first"})
        ),
        BridgeError::Provider(ProviderError::InvalidPayload { .. })
    ));
    assert_eq!(
        harness.provider.census(),
        IncCensus {
            sessions: 2,
            ready_sessions: 2,
            subscriptions: 1,
            channels: 0,
        }
    );
}

#[test]
fn good_morning_native_actions_receive_exact_trusted_origins_despite_sender_exclusion() {
    let native = Arc::new(FakeNativeActions::bounded(4));
    let native_dyn: Arc<dyn IncNativeActionSink> = native.clone();
    let mut harness = Harness::with_native_actions(IncProviderLimits::default(), 64, native_dyn);
    harness.open(1, "good-morning", 'a');
    for (index, topic) in [NOTE_OPEN_TOPIC, PROFILE_OPEN_TOPIC, COMPOSE_OPEN_TOPIC]
        .into_iter()
        .enumerate()
    {
        harness.dispatch(
            1,
            json!({"type":"inc.subscribe","id":format!("self-{index}"),"topic":topic}),
        );
    }

    let event_id = "1".repeat(64);
    let author = "a".repeat(64);
    let note_payload = json!({
        "target":{
            "type":"event",
            "id":event_id,
            "kind":1,
            "pubkey":author,
        },
        "behavior":{"focus":true},
        "source":{"napplet":"good-morning"},
    });
    let profile_payload = json!({"pubkey":author});
    let compose_payload = json!({
        "source":{"napplet":"good-morning"},
        "intent":"reply",
        "replyTo":{
            "id":event_id,
            "pubkey":author,
            "kind":1,
            "content":"GM",
            "created_at":1_750_000_000_u64,
        },
    });
    for (topic, payload) in [
        (NOTE_OPEN_TOPIC, note_payload.clone()),
        (PROFILE_OPEN_TOPIC, profile_payload.clone()),
        (COMPOSE_OPEN_TOPIC, compose_payload.clone()),
    ] {
        harness.dispatch(
            1,
            json!({"type":"inc.emit","topic":topic,"payload":payload}),
        );
    }

    assert!(
        harness.drain(1).is_empty(),
        "sender remains excluded from ordinary inter-session pub/sub"
    );
    let actions = native.drain();
    assert_eq!(actions.len(), 3);
    assert_eq!(
        actions.iter().map(|action| action.kind).collect::<Vec<_>>(),
        vec![
            IncNativeActionKind::NoteOpen,
            IncNativeActionKind::ProfileOpen,
            IncNativeActionKind::ComposeOpen,
        ]
    );
    for action in &actions {
        assert_eq!(action.origin.session, SessionId(1));
        assert_eq!(action.origin.source_window, SourceWindowId(101));
        assert_eq!(action.origin.principal.d_tag(), "good-morning");
        assert_eq!(
            action.kind.topic(),
            match action.kind {
                IncNativeActionKind::NoteOpen => NOTE_OPEN_TOPIC,
                IncNativeActionKind::ProfileOpen => PROFILE_OPEN_TOPIC,
                IncNativeActionKind::ComposeOpen => COMPOSE_OPEN_TOPIC,
            }
        );
    }
    assert_eq!(actions[0].payload.decode().unwrap(), note_payload);
    assert_eq!(actions[1].payload.decode().unwrap(), profile_payload);
    assert_eq!(actions[2].payload.decode().unwrap(), compose_payload);
}

#[test]
fn native_action_topics_reject_malformed_or_authority_widening_payloads() {
    let native = Arc::new(FakeNativeActions::bounded(8));
    let native_dyn: Arc<dyn IncNativeActionSink> = native.clone();
    let mut harness = Harness::with_native_actions(IncProviderLimits::default(), 64, native_dyn);
    harness.open(1, "good-morning", 'a');
    let event_id = "1".repeat(64);
    let author = "a".repeat(64);
    let invalid = [
        json!({"type":"inc.emit","topic":PROFILE_OPEN_TOPIC}),
        json!({
            "type":"inc.emit",
            "topic":PROFILE_OPEN_TOPIC,
            "payload":{"pubkey":"ABC","sessionId":2},
        }),
        json!({
            "type":"inc.emit",
            "topic":NOTE_OPEN_TOPIC,
            "payload":{
                "target":{"type":"event","id":event_id,"relay":"wss://attacker.test"},
            },
        }),
        json!({
            "type":"inc.emit",
            "topic":COMPOSE_OPEN_TOPIC,
            "payload":{
                "source":{"napplet":"good-morning"},
                "intent":"reply",
                "replyTo":{"id":event_id,"pubkey":author,"kind":-1},
            },
        }),
    ];
    for message in invalid {
        assert!(matches!(
            harness.dispatch_error(1, message),
            BridgeError::Provider(ProviderError::InvalidPayload { .. })
        ));
    }
    assert!(native.drain().is_empty());
}

#[test]
fn native_action_capacity_and_closed_state_refuse_before_pubsub_delivery() {
    let native = Arc::new(FakeNativeActions::bounded(1));
    let native_dyn: Arc<dyn IncNativeActionSink> = native.clone();
    let mut harness = Harness::with_native_actions(IncProviderLimits::default(), 64, native_dyn);
    harness.open(1, "good-morning", 'a');
    harness.open(2, "observer", 'b');
    harness.dispatch(
        2,
        json!({"type":"inc.subscribe","id":"sub","topic":PROFILE_OPEN_TOPIC}),
    );
    let first = json!({"pubkey":"a".repeat(64)});
    let second = json!({"pubkey":"b".repeat(64)});
    harness.dispatch(
        1,
        json!({"type":"inc.emit","topic":PROFILE_OPEN_TOPIC,"payload":first}),
    );
    assert!(matches!(
        harness.dispatch_error(
            1,
            json!({"type":"inc.emit","topic":PROFILE_OPEN_TOPIC,"payload":second})
        ),
        BridgeError::Provider(ProviderError::Failed { .. })
    ));
    assert_eq!(
        harness.drain(2),
        vec![json!({
            "type":"inc.event",
            "topic":PROFILE_OPEN_TOPIC,
            "sender":"good-morning",
            "payload":first,
        })],
        "the refused action must not be partially delivered to subscribers"
    );
    assert!(harness.activity.facts.lock().iter().any(|fact| {
        fact.outcome == IncActivityOutcome::Refused(IncRefusal::NativeActionBackpressure)
    }));

    native.drain();
    native.closed.store(true, Ordering::Release);
    assert!(matches!(
        harness.dispatch_error(
            1,
            json!({
                "type":"inc.emit",
                "topic":PROFILE_OPEN_TOPIC,
                "payload":{"pubkey":"c".repeat(64)},
            })
        ),
        BridgeError::Provider(ProviderError::Failed { .. })
    ));
    assert!(harness.activity.facts.lock().iter().any(|fact| {
        fact.outcome == IncActivityOutcome::Refused(IncRefusal::NativeActionClosed)
    }));
    assert!(harness.drain(2).is_empty());
}

#[test]
fn native_action_teardown_purges_pending_work_once_for_close_and_revoke() {
    let native = Arc::new(FakeNativeActions::bounded(4));
    let native_dyn: Arc<dyn IncNativeActionSink> = native.clone();
    let mut harness = Harness::with_native_actions(IncProviderLimits::default(), 64, native_dyn);
    harness.open(1, "good-morning", 'a');
    harness.dispatch(
        1,
        json!({
            "type":"inc.emit",
            "topic":PROFILE_OPEN_TOPIC,
            "payload":{"pubkey":"a".repeat(64)},
        }),
    );
    assert_eq!(native.pending.lock().len(), 1);
    harness
        .registry
        .close_session_with_reason(SessionId(1), ProviderSessionEnd::Crashed);
    harness
        .registry
        .close_session_with_reason(SessionId(1), ProviderSessionEnd::Crashed);
    assert!(native.pending.lock().is_empty());
    assert_eq!(
        native.ended.lock().as_slice(),
        &[(
            IncNativeActionOrigin {
                principal: harness.endpoints[&SessionId(1)].context.principal.clone(),
                session: SessionId(1),
                source_window: SourceWindowId(101),
            },
            IncNativeActionSessionEnd::Closed(ProviderSessionEnd::Crashed),
        )]
    );

    harness.open(2, "other", 'b');
    harness.dispatch(
        2,
        json!({
            "type":"inc.emit",
            "topic":PROFILE_OPEN_TOPIC,
            "payload":{"pubkey":"b".repeat(64)},
        }),
    );
    let other = harness.endpoints[&SessionId(2)].context.principal.clone();
    harness
        .registry
        .revoke(&other, &Capability::new(DOMAIN).unwrap());
    assert!(native.pending.lock().is_empty());
    assert_eq!(
        native.ended.lock().last().map(|(_, reason)| *reason),
        Some(IncNativeActionSessionEnd::Revoked)
    );
}
