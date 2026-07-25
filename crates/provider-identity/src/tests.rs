use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use nmp_native_nap_bridge::{
    BridgeLimits, MemoryActivitySink, ProviderPushObserver, ProviderRegistry, SessionContext,
    SourceWindowId,
};
use nmp_native_runtime_core::{
    Cancellation, ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceClass,
    ResourceLimits, ResourceTracker, Sensitivity,
};

use super::*;
use crate::types::MAX_SAFE_JSON_INTEGER;

#[derive(Debug)]
struct FakeObservation {
    closed: AtomicBool,
}

impl AccountObservationHandle for FakeObservation {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
struct FakeSource {
    identity: Mutex<FrozenIdentity>,
    listener: Mutex<Option<Arc<dyn IdentityChangeListener>>>,
    observation: Arc<FakeObservation>,
    reads: AtomicUsize,
    retarget: AtomicBool,
}

impl FakeSource {
    fn new(identity: FrozenIdentity) -> Arc<Self> {
        Arc::new(Self {
            identity: Mutex::new(identity),
            listener: Mutex::new(None),
            observation: Arc::new(FakeObservation {
                closed: AtomicBool::new(false),
            }),
            reads: AtomicUsize::new(0),
            retarget: AtomicBool::new(false),
        })
    }

    fn change(&self, identity: FrozenIdentity) {
        *self.identity.lock() = identity.clone();
        if let Some(listener) = self.listener.lock().as_ref() {
            listener.changed(identity);
        }
    }
}

impl IdentityDataPlane for FakeSource {
    fn freeze_public_identity(&self) -> Result<FrozenIdentity, IdentityDataError> {
        Ok(self.identity.lock().clone())
    }

    fn read_public_identity(
        &self,
        frozen_identity: &FrozenIdentity,
        query: IdentityQuery,
        cancellation: &Cancellation,
        _limits: IdentityReadLimits,
    ) -> Result<IdentityRead, IdentityDataError> {
        if cancellation.is_cancelled() {
            return Err(IdentityDataError::Cancelled);
        }
        self.reads.fetch_add(1, Ordering::Relaxed);
        let value = match query {
            IdentityQuery::Relays => IdentityValue::Relays(BTreeMap::from([(
                "wss://relay.example".to_owned(),
                RelayPermission {
                    read: true,
                    write: false,
                },
            )])),
            IdentityQuery::Profile => IdentityValue::Profile(Some(ProfileData {
                name: Some("Alice".to_owned()),
                ..ProfileData::default()
            })),
            IdentityQuery::Follows => IdentityValue::Follows(vec!["b".repeat(64)]),
            IdentityQuery::List { list_type } => IdentityValue::List(vec![list_type.to_string()]),
            IdentityQuery::Zaps => IdentityValue::Zaps(vec![ZapReceipt {
                event_id: "event".to_owned(),
                sender: "c".repeat(64),
                amount: 21_000,
                content: Some("hello".to_owned()),
            }]),
            IdentityQuery::Mutes => IdentityValue::Mutes(vec!["d".repeat(64)]),
            IdentityQuery::Blocked => IdentityValue::Blocked(vec!["e".repeat(64)]),
            IdentityQuery::Badges => IdentityValue::Badges(vec![Badge {
                id: "badge".to_owned(),
                name: Some("Early".to_owned()),
                description: None,
                image: None,
                thumbs: Some(vec!["https://example/thumb.png".to_owned()]),
                awarded_by: "f".repeat(64),
            }]),
        };
        let returned_identity = if self.retarget.load(Ordering::Acquire) {
            connected_identity(frozen_identity.generation.saturating_add(1), "9".repeat(64))
        } else {
            frozen_identity.clone()
        };
        Ok(IdentityRead {
            frozen_identity: returned_identity,
            value: encode_identity_value(&value, 64 * 1024),
            scoped_evidence: BoundedJson::from_value(
                &json!({
                    "sources": [{"relay": "wss://relay.example", "status": "requesting"}],
                    "shortfall": [],
                }),
                4096,
            )
            .unwrap(),
        })
    }

    fn observe_public_identity(
        &self,
        listener: Arc<dyn IdentityChangeListener>,
    ) -> Result<AccountObservation, IdentityDataError> {
        *self.listener.lock() = Some(listener);
        Ok(AccountObservation {
            current: self.identity.lock().clone(),
            observation: self.observation.clone(),
        })
    }
}

#[derive(Debug, Default)]
struct FakeDiagnostics {
    facts: Mutex<Vec<IdentityDiagnostic>>,
}

impl IdentityDiagnosticsSink for FakeDiagnostics {
    fn record(&self, fact: IdentityDiagnostic) {
        self.facts.lock().push(fact);
    }
}

fn principal() -> Principal {
    Principal::new("1".repeat(64), "profile", "2".repeat(64)).unwrap()
}

fn connected_identity(generation: u64, pubkey: String) -> FrozenIdentity {
    FrozenIdentity {
        generation,
        account: Some(nmp_native_runtime_core::AccountRef(Arc::from(pubkey))),
    }
}

fn signed_out_identity(generation: u64) -> FrozenIdentity {
    FrozenIdentity {
        generation,
        account: None,
    }
}

fn provider() -> (Arc<IdentityProvider>, Arc<FakeSource>, Arc<FakeDiagnostics>) {
    let source = FakeSource::new(connected_identity(1, "a".repeat(64)));
    let diagnostics = Arc::new(FakeDiagnostics::default());
    let source_dyn: Arc<dyn IdentityDataPlane> = source.clone();
    let diagnostics_dyn: Arc<dyn IdentityDiagnosticsSink> = diagnostics.clone();
    let provider = IdentityProvider::connect(
        source_dyn,
        diagnostics_dyn,
        IdentityProviderLimits::default(),
    )
    .unwrap();
    (provider, source, diagnostics)
}

fn opened_session(provider: Arc<IdentityProvider>) -> (ProviderRegistry, ProviderPushObserver) {
    let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
    let grants =
        Arc::new(GrantLedger::new(GrantLimits::default(), Arc::clone(&resources)).unwrap());
    let activity = Arc::new(MemoryActivitySink::bounded(32));
    let mut registry = ProviderRegistry::new(
        BridgeLimits::default(),
        resources,
        Arc::clone(&grants),
        activity,
    )
    .unwrap();
    let domain = Capability::new(DOMAIN).unwrap();
    grants
        .set(
            principal(),
            domain,
            Sensitivity::Sensitive,
            GrantDecision::AllowExactBuild,
        )
        .unwrap();
    registry.register(provider).unwrap();
    let context = SessionContext {
        id: SessionId(7),
        principal: principal(),
        profile: ExecutionProfile::Legacy,
    };
    let plan = registry
        .negotiate(
            &context.principal,
            context.profile,
            &BTreeSet::from([Capability::new(DOMAIN).unwrap()]),
        )
        .unwrap();
    let observer = registry
        .open_session_bound(&context, &plan, SourceWindowId(11), 0)
        .unwrap();
    registry.mark_session_ready(context.id).unwrap();
    (registry, observer)
}

fn drain(observer: &ProviderPushObserver) -> Vec<Value> {
    observer
        .drain(16)
        .unwrap()
        .pushes
        .into_iter()
        .map(|push| push.envelope.decode().unwrap())
        .collect()
}

fn request(action: &str, payload: Value) -> ProviderRequest {
    let resources = ResourceTracker::new(ResourceLimits::default()).unwrap();
    let work = resources
        .admit(
            SessionId(7),
            Some(Capability::new(DOMAIN).unwrap()),
            ResourceClass::ProviderCall,
        )
        .unwrap();
    ProviderRequest {
        principal: principal(),
        session: SessionId(7),
        action: Arc::from(action),
        correlation_id: Some(Arc::from("request-1")),
        payload,
        work,
    }
}

fn response(provider: &IdentityProvider, action: &str, payload: Value) -> Value {
    provider
        .call(request(action, payload))
        .unwrap()
        .response
        .unwrap()
        .decode()
        .unwrap()
}

#[test]
fn descriptor_covers_every_pinned_request_action() {
    let (provider, _, _) = provider();
    assert_eq!(
        provider.descriptor().actions,
        [
            "getPublicKey",
            "getRelays",
            "getProfile",
            "getFollows",
            "getList",
            "getZaps",
            "getMutes",
            "getBlocked",
            "getBadges",
        ]
        .into_iter()
        .map(Arc::from)
        .collect()
    );
    assert_eq!(
        provider.descriptor().protocol_versions,
        BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)])
    );
}

#[test]
fn every_pinned_action_uses_the_exact_flat_result_envelope() {
    let (provider, _, diagnostics) = provider();
    assert_eq!(
        response(&provider, "getPublicKey", json!({})),
        json!({
            "type": "identity.getPublicKey.result",
            "id": "request-1",
            "pubkey": "a".repeat(64),
        })
    );
    assert_eq!(
        response(&provider, "getRelays", json!({}))["type"],
        "identity.getRelays.result"
    );
    assert_eq!(
        response(&provider, "getProfile", json!({}))["profile"]["displayName"],
        Value::Null
    );
    assert_eq!(
        response(&provider, "getFollows", json!({}))["pubkeys"][0],
        "b".repeat(64)
    );
    assert_eq!(
        response(&provider, "getList", json!({"listType": "bookmarks"}))["entries"][0],
        "bookmarks"
    );
    assert_eq!(
        response(&provider, "getZaps", json!({}))["zaps"][0]["amount"],
        21_000
    );
    assert_eq!(
        response(&provider, "getMutes", json!({}))["pubkeys"][0],
        "d".repeat(64)
    );
    assert_eq!(
        response(&provider, "getBlocked", json!({}))["pubkeys"][0],
        "e".repeat(64)
    );
    assert_eq!(
        response(&provider, "getBadges", json!({}))["badges"][0]["awardedBy"],
        "f".repeat(64)
    );
    assert_eq!(
        diagnostics
            .facts
            .lock()
            .iter()
            .filter(|fact| matches!(fact, IdentityDiagnostic::Read { .. }))
            .count(),
        8
    );
}

#[test]
fn malformed_flat_payloads_are_refused_before_source_work() {
    let (provider, source, _) = provider();
    assert!(matches!(
        provider.call(request("getRelays", json!({"payload": {}}))),
        Err(ProviderError::InvalidPayload { .. })
    ));
    assert!(matches!(
        provider.call(request("getList", json!({"list_type": "bookmarks"}))),
        Err(ProviderError::InvalidPayload { .. })
    ));
    assert_eq!(source.reads.load(Ordering::Relaxed), 0);
}

#[test]
fn query_is_frozen_and_source_retargeting_fails_closed() {
    let (provider, source, _) = provider();
    source.retarget.store(true, Ordering::Release);
    let result = response(&provider, "getFollows", json!({}));
    assert_eq!(result["pubkeys"], json!([]));
    assert_eq!(
        result["error"],
        IdentityDataError::InvalidSourceData.to_string()
    );
}

#[test]
fn cancellation_returns_the_pinned_default_and_error_shape() {
    let (provider, _, _) = provider();
    let request = request("getBadges", json!({}));
    request.work.cancellation().cancel();
    let result = provider
        .call(request)
        .unwrap()
        .response
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(result["badges"], json!([]));
    assert_eq!(result["error"], IdentityDataError::Cancelled.to_string());
}

#[test]
fn changed_push_has_no_id_and_sign_out_uses_empty_pubkey() {
    let (provider, source, _) = provider();
    let (_registry, observer) = opened_session(Arc::clone(&provider));
    let initial = drain(&observer);
    source.change(connected_identity(2, "b".repeat(64)));
    let changed = drain(&observer);
    source.change(signed_out_identity(3));
    let signed_out = drain(&observer);
    let messages = [initial, changed, signed_out].concat();
    assert_eq!(
        messages[0],
        json!({"type": "identity.changed", "pubkey": "a".repeat(64)})
    );
    assert_eq!(
        messages[1],
        json!({"type": "identity.changed", "pubkey": "b".repeat(64)})
    );
    assert_eq!(
        messages[2],
        json!({"type": "identity.changed", "pubkey": ""})
    );
    assert!(messages.iter().all(|message| message.get("id").is_none()));
}

#[test]
fn stale_change_is_ignored_and_session_close_removes_the_push_lane() {
    let (provider, source, _) = provider();
    let (registry, observer) = opened_session(Arc::clone(&provider));
    assert_eq!(drain(&observer).len(), 1);
    source.change(connected_identity(1, "c".repeat(64)));
    assert!(drain(&observer).is_empty());
    registry.close_session(SessionId(7));
    assert_eq!(provider.active_sessions(), 0);
    assert!(observer.drain(16).unwrap().closed);
    source.change(connected_identity(2, "d".repeat(64)));
    assert!(observer.drain(16).unwrap().pushes.is_empty());
}

#[test]
fn close_is_idempotent_and_closes_the_change_observation() {
    let (provider, source, _) = provider();
    let (_registry, _observer) = opened_session(Arc::clone(&provider));
    provider.close();
    provider.close();
    assert!(source.observation.closed.load(Ordering::Acquire));
    assert_eq!(provider.active_sessions(), 0);
}

#[test]
fn unsafe_source_values_are_bounded_before_crossing_the_bridge() {
    let (provider, source, _) = provider();
    *source.identity.lock() = connected_identity(1, "a".repeat(64));
    let limits = IdentityProviderLimits {
        maximum_items: 1,
        ..IdentityProviderLimits::default()
    };
    assert_eq!(limits.maximum_items, 1);

    let excessive = IdentityValue::Follows(vec!["b".repeat(64), "c".repeat(64)]);
    assert!(validate_value(&IdentityQuery::Follows, &excessive, limits).is_err());
    let unsafe_amount = IdentityValue::Zaps(vec![ZapReceipt {
        event_id: "event".to_owned(),
        sender: "b".repeat(64),
        amount: MAX_SAFE_JSON_INTEGER + 1,
        content: None,
    }]);
    assert!(validate_value(&IdentityQuery::Zaps, &unsafe_amount, limits).is_err());

    let global_claim =
        BoundedJson::from_value(&json!({"sources": [], "synced": true}), 1024).unwrap();
    assert!(validate_evidence(&global_claim, limits).is_err());
    drop(provider);
}

#[test]
fn compiled_contract_matches_the_pinned_inventory_and_tarball_hash() {
    let lock = include_str!("../../../compatibility.lock");
    assert!(lock.contains("nap = \"0.28.0\""));
    assert!(lock.contains(&format!("nap = \"{PINNED_NPM_TARBALL_SHA256}\"")));

    let inventory: Value = serde_json::from_str(include_str!(
        "../../../conformance/envelopes/inventory.json"
    ))
    .unwrap();
    let identity_types = inventory["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["domain"] == DOMAIN)
        .map(|entry| entry["type"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        identity_types,
        BTreeSet::from([
            "identity.changed",
            "identity.getBadges",
            "identity.getBadges.result",
            "identity.getBlocked",
            "identity.getBlocked.result",
            "identity.getFollows",
            "identity.getFollows.result",
            "identity.getList",
            "identity.getList.result",
            "identity.getMutes",
            "identity.getMutes.result",
            "identity.getProfile",
            "identity.getProfile.result",
            "identity.getPublicKey",
            "identity.getPublicKey.result",
            "identity.getRelays",
            "identity.getRelays.result",
            "identity.getZaps",
            "identity.getZaps.result",
        ])
    );
}
