
use nmp_native_nap_bridge::{
    ActivitySink, BridgeLimits, ProviderActivity, ProviderPushObserver, ProviderRegistry,
    SessionContext, SourceWindowId,
};
use nmp_native_runtime_core::{
    ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceClass, ResourceLimits,
    ResourceTracker, Sensitivity,
};
use parking_lot::RwLock;

use super::*;

#[derive(Debug)]
struct HostTheme {
    current: RwLock<Option<ThemeSnapshot>>,
}

impl ThemeSource for HostTheme {
    fn current(&self) -> Option<ThemeSnapshot> {
        self.current.read().clone()
    }
}

#[derive(Debug)]
struct NoActivity;

impl ActivitySink for NoActivity {
    fn record(&self, _fact: ProviderActivity) {}
}

fn value(title: &str) -> Value {
    json!({
        "colors": {
            "background": "#000000",
            "text": "#ffffff",
            "primary": "#ff9900"
        },
        "title": title
    })
}

fn principal(hash: char) -> Principal {
    Principal::new("a".repeat(64), "theme-app", hash.to_string().repeat(64)).unwrap()
}

fn call(
    provider: &ThemeProvider,
    resources: &ResourceTracker,
    principal: Principal,
    session: u64,
    id: Option<&str>,
    payload: Value,
) -> Result<ProviderCall, ProviderError> {
    provider.call(ProviderRequest {
        principal,
        session: SessionId(session),
        action: Arc::from("get"),
        correlation_id: id.map(Arc::from),
        payload,
        work: resources
            .admit(
                SessionId(session),
                Some(Capability::new(THEME_DOMAIN).unwrap()),
                ResourceClass::ProviderCall,
            )
            .unwrap(),
    })
}

fn registry(
    provider: Arc<ThemeProvider>,
) -> (ProviderRegistry, Arc<GrantLedger>, Arc<ResourceTracker>) {
    let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
    let grants = Arc::new(GrantLedger::new(GrantLimits::default(), resources.clone()).unwrap());
    let mut registry = ProviderRegistry::new(
        BridgeLimits::default(),
        resources.clone(),
        grants.clone(),
        Arc::new(NoActivity),
    )
    .unwrap();
    registry.register(provider).unwrap();
    (registry, grants, resources)
}

fn open(
    registry: &ProviderRegistry,
    grants: &GrantLedger,
    principal: &Principal,
    session: u64,
) -> Result<ProviderPushObserver, nmp_native_nap_bridge::BridgeError> {
    let capability = Capability::new(THEME_DOMAIN).unwrap();
    grants
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
    let plan = registry.negotiate(
        principal,
        ExecutionProfile::Legacy,
        &BTreeSet::from([capability]),
    )?;
    registry.open_session_bound(
        &context,
        &plan,
        SourceWindowId(session.saturating_add(1_000)),
        0,
    )
}

#[test]
fn get_descriptor_and_projection_match_the_pinned_contract() {
    let limits = ThemeProviderLimits::default();
    let source = Arc::new(HostTheme {
        current: RwLock::new(Some(
            ThemeSnapshot::from_value(&value("Host theme"), limits).unwrap(),
        )),
    });
    let provider = ThemeProvider::new(source, limits).unwrap();
    assert_eq!(provider.descriptor.domain.as_str(), THEME_DOMAIN);
    assert_eq!(
        provider.descriptor.actions,
        BTreeSet::from([Arc::from("get")])
    );
    assert_eq!(
        provider.descriptor.protocol_versions,
        BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)])
    );
    let resources = ResourceTracker::new(ResourceLimits::default()).unwrap();
    assert_eq!(
        call(
            &provider,
            &resources,
            principal('b'),
            1,
            Some("theme-1"),
            json!({})
        )
        .unwrap()
        .response
        .unwrap()
        .decode()
        .unwrap(),
        json!({
            "type":"theme.get.result",
            "id":"theme-1",
            "theme":value("Host theme")
        })
    );
}

#[test]
fn change_is_automatically_targeted_to_every_declaring_ready_session_only() {
    let limits = ThemeProviderLimits::default();
    let source = Arc::new(HostTheme {
        current: RwLock::new(None),
    });
    let provider = Arc::new(ThemeProvider::new(source, limits).unwrap());
    let (registry, grants, _resources) = registry(provider.clone());
    let first = principal('b');
    let second = principal('c');
    let first_pushes = open(&registry, &grants, &first, 1).unwrap();
    let second_pushes = open(&registry, &grants, &second, 2).unwrap();
    let snapshot = ThemeSnapshot::from_value(&value("Changed"), limits).unwrap();
    assert_eq!(provider.publish_changed(&snapshot).unwrap().attempted, 0);
    registry.mark_session_ready(SessionId(1)).unwrap();
    registry.mark_session_ready(SessionId(2)).unwrap();
    assert_eq!(
        provider.publish_changed(&snapshot).unwrap(),
        ProviderPushReport {
            attempted: 2,
            delivered: 2,
            refused: 0
        }
    );
    for observer in [first_pushes, second_pushes] {
        let pushes = observer.drain(2).unwrap().pushes;
        assert_eq!(pushes.len(), 1);
        assert_eq!(
            pushes[0].envelope.decode().unwrap(),
            json!({"type":"theme.changed","theme":value("Changed")})
        );
    }
}

#[test]
fn teardown_revoke_identity_and_capacity_are_explicit() {
    let limits = ThemeProviderLimits {
        maximum_declaring_ready_sessions: 1,
        ..ThemeProviderLimits::default()
    };
    let source = Arc::new(HostTheme {
        current: RwLock::new(None),
    });
    let provider = Arc::new(ThemeProvider::new(source, limits).unwrap());
    let (registry, grants, _resources) = registry(provider.clone());
    let first = principal('b');
    open(&registry, &grants, &first, 1).unwrap();
    assert!(open(&registry, &grants, &first, 2).is_err());
    assert_eq!(
        registry.revoke(&first, &Capability::new(THEME_DOMAIN).unwrap()),
        0
    );
    open(&registry, &grants, &first, 2).unwrap();
    registry.close_session(SessionId(2));
    open(&registry, &grants, &first, 3).unwrap();
}

#[test]
fn invalid_oversized_or_malformed_get_is_refused() {
    let limits = ThemeProviderLimits {
        maximum_theme_bytes: 128,
        maximum_response_bytes: 256,
        maximum_string_bytes: 16,
        ..ThemeProviderLimits::default()
    };
    assert_eq!(
        ThemeSnapshot::from_value(
            &json!({"colors":{"background":"#000","text":"#fff"}}),
            limits
        ),
        Err(ThemeError::InvalidColors)
    );
    assert!(matches!(
        ThemeSnapshot::from_value(
            &json!({
                "colors":{
                    "background":"#000",
                    "text":"#fff",
                    "primary":"#f90"
                },
                "title":"this title exceeds sixteen"
            }),
            limits
        ),
        Err(ThemeError::InvalidField("title"))
    ));
    let source = Arc::new(HostTheme {
        current: RwLock::new(None),
    });
    let provider = ThemeProvider::new(source, limits).unwrap();
    let resources = ResourceTracker::new(ResourceLimits::default()).unwrap();
    assert!(matches!(
        call(&provider, &resources, principal('b'), 1, None, json!({})),
        Err(ProviderError::InvalidPayload { .. })
    ));
    assert!(matches!(
        call(
            &provider,
            &resources,
            principal('b'),
            1,
            Some("id"),
            json!({"extra":true})
        ),
        Err(ProviderError::InvalidPayload { .. })
    ));
}
