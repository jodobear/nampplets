
use nmp_native_nap_bridge::{
    ActivitySink, BridgeLimits, ProviderActivity, ProviderPushObserver, ProviderRegistry,
    SessionContext, SourceWindowId,
};
use nmp_native_runtime_core::{
    ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceClass, ResourceLimits,
    ResourceTracker, Sensitivity,
};
use nmp_native_runtime_store::StoreLimits;
use parking_lot::Mutex;
use tempfile::TempDir;

use super::*;
#[derive(Debug, Default)]
struct Settings(Mutex<Vec<SettingsRequest>>);

impl SettingsExecutor for Settings {
    fn try_open(&self, request: SettingsRequest) -> Result<(), SettingsExecutorError> {
        self.0.lock().push(request);
        Ok(())
    }
}

#[derive(Debug)]
struct NoActivity;

impl ActivitySink for NoActivity {
    fn record(&self, _fact: ProviderActivity) {}
}

struct Rig {
    _directory: TempDir,
    provider: Arc<ConfigProvider>,
    registry: ProviderRegistry,
    grants: Arc<GrantLedger>,
    settings: Arc<Settings>,
    resources: Arc<ResourceTracker>,
}

impl Rig {
    fn new(limits: ConfigProviderLimits) -> Self {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(
            RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default())
                .unwrap(),
        );
        let settings = Arc::new(Settings::default());
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        let grants = Arc::new(GrantLedger::new(GrantLimits::default(), resources.clone()).unwrap());
        let provider = Arc::new(ConfigProvider::new(store, settings.clone(), limits).unwrap());
        let mut registry = ProviderRegistry::new(
            BridgeLimits::default(),
            resources.clone(),
            grants.clone(),
            Arc::new(NoActivity),
        )
        .unwrap();
        registry.register(provider.clone()).unwrap();
        Self {
            _directory: directory,
            provider,
            registry,
            grants,
            settings,
            resources,
        }
    }

    fn open(
        &self,
        principal: &Principal,
        session: u64,
    ) -> Result<ProviderPushObserver, nmp_native_nap_bridge::BridgeError> {
        let capability = Capability::new(CONFIG_DOMAIN).unwrap();
        self.grants
            .set(
                principal.clone(),
                capability.clone(),
                Sensitivity::Ordinary,
                GrantDecision::AllowExactBuild,
            )
            .unwrap();
        let context = SessionContext {
            id: SessionId(session),
            principal: principal.clone(),
            profile: ExecutionProfile::Legacy,
        };
        let plan = self.registry.negotiate(
            principal,
            ExecutionProfile::Legacy,
            &BTreeSet::from([capability]),
        )?;
        self.registry.open_session_bound(
            &context,
            &plan,
            SourceWindowId(session.saturating_add(1_000)),
            0,
        )
    }

    fn call(
        &self,
        principal: Principal,
        session: u64,
        action: &str,
        id: Option<&str>,
        payload: Value,
    ) -> Result<Option<Value>, ProviderError> {
        let call = self.provider.call(ProviderRequest {
            principal,
            session: SessionId(session),
            action: Arc::from(action),
            correlation_id: id.map(Arc::from),
            payload,
            work: self
                .resources
                .admit(
                    SessionId(session),
                    Some(Capability::new(CONFIG_DOMAIN).unwrap()),
                    ResourceClass::ProviderCall,
                )
                .unwrap(),
        })?;
        Ok(call
            .response
            .map(|response| response.decode().expect("valid response")))
    }
}

fn principal(hash: char) -> Principal {
    Principal::new("a".repeat(64), "config-app", hash.to_string().repeat(64)).unwrap()
}

fn schema() -> Value {
    json!({
        "$schema":"http://json-schema.org/draft-07/schema#",
        "type":"object",
        "properties":{
            "theme":{
                "type":"string",
                "enum":["light","dark"],
                "default":"dark",
                "x-napplet-section":"appearance"
            },
            "size":{"type":"integer","minimum":10,"maximum":20,"default":12},
            "nested":{
                "type":"object",
                "properties":{"enabled":{"type":"boolean"}},
                "default":{"enabled":true}
            },
            "token":{"type":"string","x-napplet-secret":true}
        },
        "additionalProperties":false
    })
}

#[test]
fn descriptor_covers_every_pinned_outbound_action() {
    let rig = Rig::new(ConfigProviderLimits::default());
    assert_eq!(
        rig.provider.descriptor.actions,
        [
            "get",
            "openSettings",
            "registerSchema",
            "subscribe",
            "unsubscribe"
        ]
        .into_iter()
        .map(Arc::from)
        .collect()
    );
    assert_eq!(
        rig.provider.descriptor.protocol_versions,
        BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)])
    );
}

#[test]
fn schema_register_get_defaults_and_exact_build_isolation() {
    let rig = Rig::new(ConfigProviderLimits::default());
    let owner = principal('b');
    rig.open(&owner, 1).unwrap();
    assert_eq!(
        rig.call(
            owner.clone(),
            1,
            "registerSchema",
            Some("r1"),
            json!({"schema":schema(),"version":1})
        )
        .unwrap()
        .unwrap(),
        json!({"type":"config.registerSchema.result","id":"r1","ok":true})
    );
    assert_eq!(
        rig.call(owner.clone(), 1, "get", Some("g1"), json!({}))
            .unwrap()
            .unwrap(),
        json!({
            "type":"config.values",
            "id":"g1",
            "values":{"theme":"dark","size":12,"nested":{"enabled":true}}
        })
    );
    let update = principal('c');
    rig.open(&update, 2).unwrap();
    assert_eq!(
        rig.call(update, 2, "get", Some("g2"), json!({}))
            .unwrap()
            .unwrap(),
        json!({
            "type":"config.schemaError",
            "code":"no-schema",
            "error":"no schema is registered"
        })
    );
}

#[test]
fn forbidden_schema_features_return_correlated_negative_ack() {
    let rig = Rig::new(ConfigProviderLimits::default());
    let owner = principal('b');
    rig.open(&owner, 1).unwrap();
    for (schema, code) in [
        (
            json!({"type":"object","properties":{"x":{"type":"string","pattern":"x"}}}),
            "pattern-not-allowed",
        ),
        (
            json!({"type":"object","properties":{"x":{"type":"string","$ref":"#"}}}),
            "ref-not-allowed",
        ),
        (
            json!({"type":"object","properties":{"x":{"type":"string","x-napplet-secret":true,"default":"bad"}}}),
            "secret-with-default",
        ),
    ] {
        let response = rig
            .call(
                owner.clone(),
                1,
                "registerSchema",
                Some("bad"),
                json!({"schema":schema}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(response["ok"], false);
        assert_eq!(response["code"], code);
    }
}

#[test]
fn subscribe_push_commit_and_teardown_are_exact_and_bounded() {
    let limits = ConfigProviderLimits {
        maximum_subscribed_sessions: 1,
        ..ConfigProviderLimits::default()
    };
    let rig = Rig::new(limits);
    let owner = principal('b');
    let observer = rig.open(&owner, 1).unwrap();
    rig.provider
        .register_manifest_schema(&owner, &schema(), Some(1))
        .unwrap();
    assert_eq!(
        rig.call(owner.clone(), 1, "subscribe", None, json!({}))
            .unwrap()
            .unwrap(),
        json!({
            "type":"config.values",
            "values":{"theme":"dark","size":12,"nested":{"enabled":true}}
        })
    );
    assert!(rig.open(&owner, 2).is_err());
    let report = rig
        .provider
        .commit_values(
            &owner,
            &json!({"theme":"light","size":14,"nested":{"enabled":false},"token":"secret"}),
        )
        .unwrap();
    assert_eq!(
        report,
        ProviderPushReport {
            attempted: 1,
            delivered: 1,
            refused: 0
        }
    );
    let push = observer.drain(8).unwrap().pushes.pop().unwrap();
    assert_eq!(push.session, SessionId(1));
    assert_eq!(
        push.envelope.decode().unwrap(),
        json!({
            "type":"config.values",
            "values":{"theme":"light","size":14,"nested":{"enabled":false},"token":"secret"}
        })
    );
    rig.registry.close_session(SessionId(1));
    assert_eq!(
        rig.provider
            .commit_values(
                &owner,
                &json!({"theme":"dark","size":13,"nested":{"enabled":true}})
            )
            .unwrap()
            .attempted,
        0
    );
}

#[test]
fn schema_change_drops_orphans_and_secret_values_before_delivery() {
    let rig = Rig::new(ConfigProviderLimits::default());
    let owner = principal('b');
    rig.open(&owner, 1).unwrap();
    rig.provider
        .register_manifest_schema(&owner, &schema(), Some(1))
        .unwrap();
    rig.provider
        .commit_values(
            &owner,
            &json!({"theme":"light","size":14,"nested":{"enabled":false},"token":"secret"}),
        )
        .unwrap();
    let next = json!({
        "type":"object",
        "properties":{"theme":{"type":"string","default":"dark"}},
        "additionalProperties":false
    });
    rig.provider
        .register_manifest_schema(&owner, &next, Some(2))
        .unwrap();
    assert_eq!(
        rig.call(owner, 1, "get", Some("g"), json!({}))
            .unwrap()
            .unwrap()["values"],
        json!({"theme":"light"})
    );
}

#[test]
fn settings_executor_receives_bounded_validated_data_not_a_native_handle() {
    let rig = Rig::new(ConfigProviderLimits::default());
    let owner = principal('b');
    rig.open(&owner, 9).unwrap();
    rig.provider
        .register_manifest_schema(&owner, &schema(), Some(1))
        .unwrap();
    assert!(
        rig.call(
            owner.clone(),
            9,
            "openSettings",
            None,
            json!({"section":"appearance"})
        )
        .unwrap()
        .is_none()
    );
    let request = rig.settings.0.lock().pop().unwrap();
    assert_eq!(request.principal, owner);
    assert_eq!(request.session, SessionId(9));
    assert_eq!(request.section.as_deref(), Some("appearance"));
    assert!(request.schema.byte_len() > 0);
    assert!(request.values.byte_len() > 0);
}
