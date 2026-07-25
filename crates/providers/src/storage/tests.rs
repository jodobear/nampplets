use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    ActivitySink, BridgeLimits, DispatchOutcome, ProviderActivity, ProviderRegistry, SessionContext,
};
use nmp_native_runtime_core::{
    ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, Principal, ResourceClass,
    ResourceLimits, ResourceTracker, Sensitivity,
};
use nmp_native_runtime_store::StoreLimits;
use serde_json::json;
use tempfile::TempDir;

use super::*;

#[derive(Debug)]
struct NoActivity;

impl ActivitySink for NoActivity {
    fn record(&self, _fact: ProviderActivity) {}
}

struct Rig {
    _directory: TempDir,
    provider: StorageProvider,
    resources: Arc<ResourceTracker>,
}

impl Rig {
    fn new(limits: StorageProviderLimits) -> Self {
        let directory = TempDir::new().unwrap();
        let store_limits = StoreLimits {
            maximum_kv_keys_per_scope: limits.maximum_keys_per_scope,
            maximum_kv_bytes_per_scope: limits.maximum_scope_bytes,
            maximum_value_bytes: limits.maximum_value_bytes,
            ..StoreLimits::default()
        };
        let store = Arc::new(
            RuntimeStore::open(directory.path().join("runtime.db"), store_limits).unwrap(),
        );
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        Self {
            _directory: directory,
            provider: StorageProvider::new(store, limits).unwrap(),
            resources,
        }
    }

    fn call(
        &self,
        principal: Principal,
        session: u64,
        action: &str,
        id: Option<&str>,
        payload: Value,
    ) -> Result<Value, ProviderError> {
        let work = self
            .resources
            .admit(
                SessionId(session),
                Some(Capability::new("storage").unwrap()),
                ResourceClass::ProviderCall,
            )
            .unwrap();
        let call = self.provider.call(ProviderRequest {
            principal,
            session: SessionId(session),
            action: Arc::from(action),
            correlation_id: id.map(Arc::from),
            payload,
            work,
        })?;
        assert!(!call.is_active());
        Ok(call.response.unwrap().decode().unwrap())
    }
}

fn principal(hash: char) -> Principal {
    Principal::new("a".repeat(64), "napplet", hash.to_string().repeat(64)).unwrap()
}

#[test]
fn pinned_descriptor_has_no_placeholder_actions() {
    let rig = Rig::new(StorageProviderLimits::default());
    assert_eq!(rig.provider.descriptor().domain.as_str(), "storage");
    assert_eq!(
        rig.provider.descriptor().actions,
        ["get", "keys", "remove", "set"]
            .into_iter()
            .map(Arc::from)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        rig.provider.descriptor().protocol_versions,
        BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)])
    );
}

#[test]
fn descriptor_matches_pinned_validator_inventory() {
    let rig = Rig::new(StorageProviderLimits::default());
    let inventory: Value = serde_json::from_str(include_str!(
        "../../../../conformance/envelopes/inventory.json"
    ))
    .unwrap();
    let entries = inventory["entries"].as_array().unwrap();
    let outbound = entries
        .iter()
        .filter(|entry| {
            entry["domain"] == "storage"
                && entry["direction"] == "napplet-to-shell"
                && entry["validator"] == "pinned-conformance"
        })
        .map(|entry| {
            entry["type"]
                .as_str()
                .unwrap()
                .strip_prefix("storage.")
                .unwrap()
        })
        .map(Arc::from)
        .collect::<BTreeSet<_>>();
    let inbound = entries
        .iter()
        .filter(|entry| {
            entry["domain"] == "storage"
                && entry["direction"] == "shell-to-napplet"
                && entry["validator"] == "pinned-conformance"
        })
        .map(|entry| entry["type"].as_str().unwrap())
        .collect::<BTreeSet<_>>();

    assert_eq!(rig.provider.descriptor().actions, outbound);
    assert_eq!(
        inbound,
        BTreeSet::from([
            "storage.get.result",
            "storage.keys.result",
            "storage.remove.result",
            "storage.set.result"
        ])
    );
}

#[test]
fn get_set_remove_and_ordered_keys_match_pinned_results() {
    let rig = Rig::new(StorageProviderLimits::default());
    let owner = principal('b');
    assert_eq!(
        rig.call(
            owner.clone(),
            1,
            "get",
            Some("g0"),
            json!({"key":"missing"})
        )
        .unwrap(),
        json!({"type":"storage.get.result","id":"g0","value":null})
    );
    assert_eq!(
        rig.call(
            owner.clone(),
            1,
            "set",
            Some("s1"),
            json!({"key":"z","value":"last"})
        )
        .unwrap(),
        json!({"type":"storage.set.result","id":"s1"})
    );
    rig.call(
        owner.clone(),
        2,
        "set",
        Some("s2"),
        json!({"key":"a","value":"first","scope":"shared"}),
    )
    .unwrap();
    assert_eq!(
        rig.call(owner.clone(), 1, "keys", Some("k1"), json!({}))
            .unwrap(),
        json!({"type":"storage.keys.result","id":"k1","keys":["a","z"]})
    );
    assert_eq!(
        rig.call(owner.clone(), 1, "remove", Some("r1"), json!({"key":"a"}))
            .unwrap(),
        json!({"type":"storage.remove.result","id":"r1"})
    );
    assert_eq!(
        rig.call(owner, 1, "get", Some("g1"), json!({"key":"a"}))
            .unwrap(),
        json!({"type":"storage.get.result","id":"g1","value":null})
    );
}

#[test]
fn exact_build_and_instance_scopes_are_isolated() {
    let rig = Rig::new(StorageProviderLimits::default());
    let first = principal('b');
    let update = principal('c');
    rig.call(
        first.clone(),
        10,
        "set",
        Some("s"),
        json!({"key":"draft","value":"one","scope":"instance"}),
    )
    .unwrap();

    assert_eq!(
        rig.call(
            first.clone(),
            11,
            "get",
            Some("g"),
            json!({"key":"draft","scope":"instance"})
        )
        .unwrap()["value"],
        Value::Null
    );
    assert_eq!(
        rig.call(
            update,
            10,
            "get",
            Some("g"),
            json!({"key":"draft","scope":"instance"})
        )
        .unwrap()["value"],
        Value::Null
    );
    assert_eq!(
        rig.call(
            first,
            10,
            "get",
            Some("g"),
            json!({"key":"draft","scope":"instance"})
        )
        .unwrap()["value"],
        "one"
    );
}

#[test]
fn quota_and_response_bounds_are_observable_without_mutation() {
    let limits = StorageProviderLimits {
        maximum_value_bytes: 4,
        maximum_scope_bytes: 5,
        maximum_keys_per_scope: 2,
        maximum_response_bytes: 256,
        ..StorageProviderLimits::default()
    };
    let rig = Rig::new(limits);
    let owner = principal('b');
    let oversized = rig
        .call(
            owner.clone(),
            1,
            "set",
            Some("s1"),
            json!({"key":"a","value":"12345"}),
        )
        .unwrap();
    assert_eq!(oversized["type"], "storage.set.result");
    assert!(oversized["error"].as_str().unwrap().contains("4 byte"));
    assert_eq!(
        rig.call(owner.clone(), 1, "keys", Some("k"), json!({}))
            .unwrap()["keys"],
        json!([])
    );

    rig.call(
        owner.clone(),
        1,
        "set",
        Some("s2"),
        json!({"key":"a","value":"1234"}),
    )
    .unwrap();
    let over_scope = rig
        .call(
            owner.clone(),
            1,
            "set",
            Some("s3"),
            json!({"key":"b","value":"12"}),
        )
        .unwrap();
    assert!(over_scope["error"].as_str().unwrap().contains("5 byte"));
    assert_eq!(
        rig.call(owner, 1, "keys", Some("k2"), json!({})).unwrap()["keys"],
        json!(["a"])
    );
}

#[test]
fn malformed_and_unknown_payloads_are_refused_exactly() {
    let rig = Rig::new(StorageProviderLimits::default());
    let owner = principal('b');
    for (action, id, payload) in [
        ("get", Some("x"), json!({"key": 7})),
        ("get", Some("x"), json!({"key":"k","extra":true})),
        ("get", Some("x"), json!({"key":"k","scope":"global"})),
        ("set", None, json!({"key":"k","value":"v"})),
        ("unknown", Some("x"), json!({})),
    ] {
        assert!(matches!(
            rig.call(owner.clone(), 1, action, id, payload),
            Err(ProviderError::InvalidPayload { .. })
        ));
    }
}

#[test]
fn registry_revoke_denies_future_calls_and_no_work_leaks() {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(
        RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default()).unwrap(),
    );
    let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
    let grants =
        Arc::new(GrantLedger::new(GrantLimits::default(), Arc::clone(&resources)).unwrap());
    let storage = Capability::new("storage").unwrap();
    let owner = principal('b');
    grants
        .set(
            owner.clone(),
            storage.clone(),
            Sensitivity::Ordinary,
            GrantDecision::AllowExactBuild,
        )
        .unwrap();
    let mut registry = ProviderRegistry::new(
        BridgeLimits::default(),
        Arc::clone(&resources),
        Arc::clone(&grants),
        Arc::new(NoActivity),
    )
    .unwrap();
    registry
        .register(Arc::new(
            StorageProvider::new(store, StorageProviderLimits::default()).unwrap(),
        ))
        .unwrap();
    assert_eq!(
        registry
            .advertised_domains()
            .into_iter()
            .map(|domain| domain.to_string())
            .collect::<Vec<_>>(),
        ["storage"]
    );
    let context = SessionContext {
        id: SessionId(7),
        principal: owner.clone(),
        profile: ExecutionProfile::Legacy,
    };
    registry.open_session(&context, 0).unwrap();
    let required = BTreeSet::from([storage.clone()]);
    let plan = registry
        .negotiate(&owner, ExecutionProfile::Legacy, &required)
        .unwrap();
    let handled = registry
        .dispatch(&context, &plan, br#"{"type":"storage.keys","id":"k"}"#, 0)
        .unwrap();
    assert!(matches!(handled, DispatchOutcome::Handled(_)));
    assert_eq!(resources.census().admitted, 0);

    assert_eq!(registry.revoke(&owner, &storage), 0);
    assert!(
        registry
            .dispatch(&context, &plan, br#"{"type":"storage.keys","id":"k2"}"#, 1,)
            .is_err()
    );
    assert_eq!(resources.census().admitted, 0);
}

#[test]
fn provider_call_work_is_always_completed_for_storage() {
    let rig = Rig::new(StorageProviderLimits::default());
    let before = rig.resources.census();
    rig.call(principal('b'), 1, "keys", Some("k"), json!({}))
        .unwrap();
    let after = rig.resources.census();
    assert_eq!(before.admitted, 0);
    assert_eq!(after.admitted, 0);
    assert_eq!(
        after.by_class.get(&ResourceClass::ProviderCall),
        before.by_class.get(&ResourceClass::ProviderCall)
    );
}
