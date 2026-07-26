//! Channel lifecycle, ACL cleanup, and native push behaviour.

use super::*;

#[test]
fn channels_open_list_emit_broadcast_and_close_with_exact_envelopes() {
    let mut harness = default_harness();
    harness.open(1, "alpha", 'a');
    harness.open(2, "beta", 'b');
    harness.open(3, "gamma", 'c');
    let first = harness.open_channel(1, "beta", "open-1");
    let second = harness.open_channel(1, "gamma", "open-2");

    assert_eq!(
        harness
            .dispatch(1, json!({"type":"inc.channel.list","id":"list-1"}))
            .unwrap(),
        json!({
            "type":"inc.channel.list.result",
            "id":"list-1",
            "channels":[
                {"id":first,"peer":"beta"},
                {"id":second,"peer":"gamma"},
            ],
        })
    );
    harness.dispatch(
        1,
        json!({
            "type":"inc.channel.emit",
            "channelId":first,
            "payload":{"command":"play"},
        }),
    );
    assert_eq!(
        harness.drain(2),
        vec![json!({
            "type":"inc.channel.event",
            "channelId":first,
            "sender":"alpha",
            "payload":{"command":"play"},
        })]
    );

    harness.dispatch(
        1,
        json!({"type":"inc.channel.broadcast","payload":"shutdown"}),
    );
    assert_eq!(
        harness.drain(2),
        vec![json!({
            "type":"inc.channel.event",
            "channelId":first,
            "sender":"alpha",
            "payload":"shutdown",
        })]
    );
    assert_eq!(
        harness.drain(3),
        vec![json!({
            "type":"inc.channel.event",
            "channelId":second,
            "sender":"alpha",
            "payload":"shutdown",
        })]
    );

    harness.dispatch(1, json!({"type":"inc.channel.close","channelId":first}));
    assert_eq!(
        harness.drain(1),
        vec![json!({"type":"inc.channel.closed","channelId":first})]
    );
    assert_eq!(
        harness.drain(2),
        vec![json!({"type":"inc.channel.closed","channelId":first})]
    );
    assert_eq!(harness.provider.census().channels, 1);
}

#[test]
fn channel_target_and_ownership_checks_refuse_ambiguity_and_hijacking() {
    let mut harness = default_harness();
    harness.open(1, "alpha", 'a');
    harness.open(2, "duplicate", 'b');
    harness.open(3, "duplicate", 'c');
    harness.open(4, "outsider", 'd');
    assert_eq!(
        harness
            .dispatch(
                1,
                json!({"type":"inc.channel.open","id":"missing","target":"none"})
            )
            .unwrap(),
        json!({
            "type":"inc.channel.open.result",
            "id":"missing",
            "error":"target not found",
        })
    );
    assert_eq!(
        harness
            .dispatch(
                1,
                json!({"type":"inc.channel.open","id":"ambiguous","target":"duplicate"})
            )
            .unwrap(),
        json!({
            "type":"inc.channel.open.result",
            "id":"ambiguous",
            "error":"target is ambiguous",
        })
    );

    let channel = harness.open_channel(1, "outsider", "ok");
    assert!(matches!(
        harness.dispatch_error(
            2,
            json!({
                "type":"inc.channel.emit",
                "channelId":channel,
                "payload":"hijack",
            })
        ),
        BridgeError::Provider(ProviderError::Denied { .. })
    ));
    assert!(harness.drain(4).is_empty());
}

#[test]
fn channel_acl_is_checked_on_open_and_dynamic_revocation_closes_once() {
    let mut harness = default_harness();
    harness.open(1, "alpha", 'a');
    harness.open(2, "beta", 'b');
    harness.dispatch(
        1,
        json!({"type":"inc.subscribe","id":"sub","topic":"state"}),
    );
    let channel = harness.open_channel(1, "beta", "open");
    harness.acl.deny_topics.store(true, Ordering::Release);
    harness.acl.deny_channels.store(true, Ordering::Release);
    let cleanup = harness.provider.enforce_acl();
    assert_eq!(
        cleanup,
        IncAclCleanup {
            subscriptions_removed: 1,
            channels_closed: 1,
            close_notifications_delivered: 2,
            close_notifications_refused: 0,
        }
    );
    assert_eq!(
        harness.drain(1),
        vec![json!({
            "type":"inc.channel.closed",
            "channelId":channel,
            "reason":REASON_ACL_REVOKED,
        })]
    );
    assert_eq!(
        harness.drain(2),
        vec![json!({
            "type":"inc.channel.closed",
            "channelId":channel,
            "reason":REASON_ACL_REVOKED,
        })]
    );
    assert_eq!(harness.provider.census().subscriptions, 0);
    assert_eq!(harness.provider.census().channels, 0);

    assert_eq!(
        harness
            .dispatch(
                1,
                json!({"type":"inc.channel.open","id":"denied","target":"beta"})
            )
            .unwrap(),
        json!({
            "type":"inc.channel.open.result",
            "id":"denied",
            "error":"channel rejected by ACL",
        })
    );
    let second = harness.provider.enforce_acl();
    assert_eq!(second.channels_closed, 0);
    assert!(harness.drain(1).is_empty());
    assert!(harness.drain(2).is_empty());
}

#[test]
fn peer_stop_crash_and_capability_revoke_clean_channels_deterministically() {
    let mut harness = default_harness();
    harness.open(1, "alpha", 'a');
    harness.open(2, "beta", 'b');
    harness.open(3, "gamma", 'c');
    let stopped = harness.open_channel(1, "beta", "open-1");
    harness.registry.close_session(SessionId(2));
    assert_eq!(
        harness.drain(1),
        vec![json!({
            "type":"inc.channel.closed",
            "channelId":stopped,
            "reason":REASON_PEER_DESTROYED,
        })]
    );
    harness.registry.close_session(SessionId(2));
    assert!(harness.drain(1).is_empty());

    let revoked = harness.open_channel(1, "gamma", "open-2");
    let gamma = harness.endpoints[&SessionId(3)].context.principal.clone();
    harness
        .registry
        .revoke(&gamma, &Capability::new(DOMAIN).unwrap());
    assert_eq!(
        harness.drain(1),
        vec![json!({
            "type":"inc.channel.closed",
            "channelId":revoked,
            "reason":REASON_ACL_REVOKED,
        })]
    );
    assert_eq!(harness.provider.census().channels, 0);
    assert_eq!(harness.provider.census().sessions, 1);

    harness
        .registry
        .close_session_with_reason(SessionId(1), ProviderSessionEnd::Crashed);
    assert_eq!(
        harness.provider.census(),
        IncCensus {
            sessions: 0,
            ready_sessions: 0,
            subscriptions: 0,
            channels: 0,
        }
    );
}

#[test]
fn slow_peer_is_fail_closed_by_bounded_push_lane() {
    let mut harness = Harness::new(IncProviderLimits::default(), 1);
    harness.open(1, "reader", 'a');
    harness.open(2, "writer", 'b');
    harness.dispatch(
        1,
        json!({"type":"inc.subscribe","id":"sub","topic":"events"}),
    );
    harness.dispatch(2, json!({"type":"inc.emit","topic":"events","payload":1}));
    harness.dispatch(2, json!({"type":"inc.emit","topic":"events","payload":2}));
    let batch = harness.endpoints[&SessionId(1)].observer.drain(4).unwrap();
    assert!(batch.closed);
    assert_eq!(
        batch.termination,
        Some(ProviderPushTermination::Backpressure)
    );
    assert!(
        batch.pushes.is_empty(),
        "termination clears queued payloads"
    );
    assert!(
        harness.activity.facts.lock().iter().any(|fact| {
            fact.outcome == IncActivityOutcome::Refused(IncRefusal::PushBackpressure)
        })
    );
}

#[test]
fn channel_id_collisions_are_bounded_and_never_overwrite() {
    let limits = IncProviderLimits::default();
    let acl: Arc<dyn IncAcl> = Arc::new(AllowAllIncAcl);
    let activity: Arc<dyn IncActivitySink> = Arc::new(NoopIncActivity);
    let ids: Arc<dyn ChannelIdGenerator> = Arc::new(FakeIds::new(std::iter::repeat_n("c-same", 9)));
    let provider =
        IncProvider::with_channel_ids(acl, activity, ids, limits).expect("valid provider");
    assert_eq!(
        provider
            .next_unique_channel_id(&IncState::default())
            .unwrap()
            .as_ref(),
        "c-same"
    );
    let mut state = IncState::default();
    state.channels.insert(
        Arc::from("c-same"),
        IncChannel {
            first: SessionId(1),
            second: SessionId(2),
        },
    );
    assert!(provider.next_unique_channel_id(&state).is_none());
    assert_eq!(state.channels.len(), 1);
}

#[test]
fn secure_ids_are_opaque_and_within_the_wire_bound() {
    let id = SecureChannelIdGenerator.next_id().unwrap();
    assert!(id.starts_with("c-"));
    assert_eq!(id.len(), 34);
    assert!(valid_opaque_id(
        &id,
        IncProviderLimits::default().maximum_channel_id_bytes
    ));
}

#[test]
fn native_push_delivers_to_an_already_subscribed_session_with_emits_exact_envelope() {
    let mut harness = default_harness();
    harness.open(1, "handler", 'a');
    harness.dispatch(
        1,
        json!({"type":"inc.subscribe","id":"sub-1","topic":"napplet:nip29-group/open"}),
    );
    let payload = BoundedJson::from_value(&json!({"group":"abc"}), 4_096).unwrap();
    harness
        .provider
        .native_push(
            SessionId(1),
            "napplet:nip29-group/open",
            "caller-d-tag",
            &payload,
        )
        .unwrap();
    assert_eq!(
        harness.drain(1),
        vec![json!({
            "type":"inc.event",
            "topic":"napplet:nip29-group/open",
            "sender":"caller-d-tag",
            "payload":{"group":"abc"},
        })]
    );
}

#[test]
fn native_push_refuses_a_session_that_has_not_subscribed_to_the_topic() {
    let mut harness = default_harness();
    harness.open(1, "handler", 'a');
    let payload = BoundedJson::from_value(&Value::Null, 4_096).unwrap();
    assert!(matches!(
        harness.provider.native_push(
            SessionId(1),
            "napplet:nip29-group/open",
            "caller-d-tag",
            &payload
        ),
        Err(IncNativePushError::NotSubscribed)
    ));
}

#[test]
fn native_push_refuses_an_unknown_session() {
    let harness = default_harness();
    let payload = BoundedJson::from_value(&Value::Null, 4_096).unwrap();
    assert!(matches!(
        harness.provider.native_push(
            SessionId(99),
            "napplet:nip29-group/open",
            "caller-d-tag",
            &payload
        ),
        Err(IncNativePushError::UnknownSession)
    ));
}
