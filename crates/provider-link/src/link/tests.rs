use nmp_native_nap_bridge::{
    ActivitySink, BridgeLimits, DispatchOutcome, ProviderActivity, ProviderPushObserver,
    ProviderRegistry, SessionContext, SourceWindowId,
};
use nmp_native_runtime_core::{
    ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceLimits, ResourceTracker,
    Sensitivity,
};
use serde_json::{Value, json};

use super::url::validate_external_url;
use super::*;

#[derive(Debug, Default)]
struct FakeOpener {
    requests: Mutex<Vec<NativeLinkOpenRequest>>,
    cancelled: Mutex<Vec<Arc<str>>>,
    start_error: Mutex<Option<NativeLinkStartError>>,
}

impl NativeLinkOpener for FakeOpener {
    fn try_open(&self, request: NativeLinkOpenRequest) -> Result<Arc<str>, NativeLinkStartError> {
        let handle: Arc<str> = Arc::from(format!("link-{}", request.token.0));
        self.requests.lock().push(request);
        if let Some(error) = self.start_error.lock().take() {
            return Err(error);
        }
        Ok(handle)
    }

    fn cancel(&self, native_handle: &str) {
        self.cancelled.lock().push(Arc::from(native_handle));
    }
}

#[derive(Debug)]
struct NoBridgeActivity;

impl ActivitySink for NoBridgeActivity {
    fn record(&self, _fact: ProviderActivity) {}
}

#[derive(Debug)]
struct DenyAllLinks;

impl LinkPolicy for DenyAllLinks {
    fn evaluate(&self, _request: &LinkPolicyRequest) -> LinkPolicyDecision {
        LinkPolicyDecision::Deny
    }
}

struct Rig {
    provider: Arc<LinkProvider>,
    opener: Arc<FakeOpener>,
    registry: ProviderRegistry,
    context: SessionContext,
    plan: nmp_native_nap_bridge::InjectionPlan,
    observer: ProviderPushObserver,
}

impl Rig {
    fn new() -> Self {
        Self::with_policy(Arc::new(AllowExternalWebLinks))
    }

    fn with_policy(policy: Arc<dyn LinkPolicy>) -> Self {
        let opener = Arc::new(FakeOpener::default());
        let provider = Arc::new(
            LinkProvider::new(
                policy,
                opener.clone(),
                Arc::new(NoopLinkActivity),
                LinkProviderLimits::default(),
            )
            .unwrap(),
        );
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        let grants = Arc::new(GrantLedger::new(GrantLimits::default(), resources.clone()).unwrap());
        let mut registry = ProviderRegistry::new(
            BridgeLimits::default(),
            resources,
            grants.clone(),
            Arc::new(NoBridgeActivity),
        )
        .unwrap();
        registry.register(provider.clone()).unwrap();
        let context = SessionContext {
            id: SessionId(7),
            principal: principal("caller", 'b'),
            profile: ExecutionProfile::Legacy,
        };
        let capability = Capability::new(LINK_DOMAIN).unwrap();
        grants
            .set(
                context.principal.clone(),
                capability.clone(),
                Sensitivity::Sensitive,
                GrantDecision::AllowExactBuild,
            )
            .unwrap();
        let plan = registry
            .negotiate(
                &context.principal,
                context.profile,
                &BTreeSet::from([capability]),
            )
            .unwrap();
        let observer = registry
            .open_session_bound(&context, &plan, SourceWindowId(77), 0)
            .unwrap();
        registry.mark_session_ready(context.id).unwrap();
        Self {
            provider,
            opener,
            registry,
            context,
            plan,
            observer,
        }
    }

    fn dispatch(&self, envelope: Value) -> Result<Option<Value>, String> {
        match self
            .registry
            .dispatch(
                &self.context,
                &self.plan,
                &serde_json::to_vec(&envelope).unwrap(),
                1,
            )
            .map_err(|error| error.to_string())?
        {
            DispatchOutcome::Handled(call) => Ok(call
                .response
                .map(|response| response.decode().expect("bounded JSON"))),
            DispatchOutcome::IgnoredUnknown => Err("unexpected unknown action".to_owned()),
        }
    }
}

fn principal(d_tag: &str, hash: char) -> Principal {
    Principal::new("a".repeat(64), d_tag, hash.to_string().repeat(64)).unwrap()
}

#[test]
fn external_open_requires_confirmation_and_completes_by_push() {
    let rig = Rig::new();
    assert_eq!(
        rig.dispatch(json!({
            "type":"link.open",
            "id":"open-1",
            "url":"https://example.com/path",
            "options":{"label":"Read post"}
        }))
        .unwrap(),
        None
    );
    let requests = rig.opener.requests.lock();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].confirmation_required);
    assert_eq!(
        requests[0].normalized_url.as_ref(),
        "https://example.com/path"
    );
    assert_eq!(requests[0].label.as_deref(), Some("Read post"));
    let token = requests[0].token;
    drop(requests);

    rig.provider
        .complete(token, NativeLinkOutcome::Opened)
        .unwrap();
    let pushes = rig.observer.drain(8).unwrap().pushes;
    assert_eq!(pushes.len(), 1);
    assert_eq!(
        pushes[0].envelope.decode().unwrap(),
        json!({"type":"link.open.result","id":"open-1","status":"opened"})
    );
    assert_eq!(rig.provider.pending_count(), 0);
}

#[test]
fn unsafe_schemes_credentials_and_private_hosts_are_denied_without_execution() {
    let rig = Rig::new();
    for (index, (url, error)) in [
        ("javascript:alert(1)", "unsupported-scheme"),
        ("file:///etc/passwd", "unsupported-scheme"),
        ("not an absolute url", "invalid-url"),
        ("https://user:pass@example.com/", "blocked-by-policy"),
        ("http://localhost:8080/", "blocked-by-policy"),
        ("http://127.0.0.1/", "blocked-by-policy"),
        ("http://192.168.1.2/", "blocked-by-policy"),
        ("https://printer.local/", "blocked-by-policy"),
        ("https://intranet/", "blocked-by-policy"),
    ]
    .into_iter()
    .enumerate()
    {
        let result = rig
            .dispatch(json!({
                "type":"link.open",
                "id":format!("bad-{index}"),
                "url":url
            }))
            .unwrap()
            .unwrap();
        assert_eq!(
            result,
            json!({
                "type":"link.open.result",
                "id":format!("bad-{index}"),
                "status":"denied",
                "error":error
            })
        );
    }
    assert!(rig.opener.requests.lock().is_empty());
}

#[test]
fn product_policy_denial_returns_the_pinned_denied_result_without_native_work() {
    let rig = Rig::with_policy(Arc::new(DenyAllLinks));
    assert_eq!(
        rig.dispatch(json!({
            "type":"link.open",
            "id":"policy-denied",
            "url":"https://example.com/"
        }))
        .unwrap()
        .unwrap(),
        json!({
            "type":"link.open.result",
            "id":"policy-denied",
            "status":"denied",
            "error":"blocked-by-policy"
        })
    );
    assert!(rig.opener.requests.lock().is_empty());
}

#[test]
fn label_matches_the_pinned_optional_bounded_display_hint() {
    let rig = Rig::new();
    let maximum = "🙂".repeat(1_024);
    assert_eq!(maximum.len(), 4 * 1_024);
    assert_eq!(
        rig.dispatch(json!({
            "type":"link.open",
            "id":"label-maximum",
            "url":"https://example.com/",
            "options":{"label":maximum}
        }))
        .unwrap(),
        None
    );
    assert_eq!(
        rig.opener.requests.lock()[0].label.as_deref(),
        Some(maximum.as_str())
    );

    let error = rig
        .dispatch(json!({
            "type":"link.open",
            "id":"label-over-maximum",
            "url":"https://example.com/",
            "options":{"label":"🙂".repeat(1_025)}
        }))
        .unwrap_err();
    assert!(
        error.contains("`options.label` exceeds the configured byte limit"),
        "{error}"
    );
    let error = rig
        .dispatch(json!({
            "type":"link.open",
            "id":"unknown-option",
            "url":"https://example.com/",
            "options":{"target":"_blank"}
        }))
        .unwrap_err();
    assert!(
        error.contains("`options` may contain only `label`"),
        "{error}"
    );
    assert_eq!(rig.opener.requests.lock().len(), 1);
}

#[test]
fn native_cancellation_denies_and_native_failure_rejects_without_new_statuses() {
    let rig = Rig::new();
    rig.dispatch(json!({
        "type":"link.open",
        "id":"cancelled",
        "url":"https://example.com/cancelled"
    }))
    .unwrap();
    let cancelled = rig.opener.requests.lock()[0].token;
    rig.provider
        .complete(cancelled, NativeLinkOutcome::Cancelled)
        .unwrap();

    rig.dispatch(json!({
        "type":"link.open",
        "id":"failed",
        "url":"https://example.com/failed"
    }))
    .unwrap();
    let failed = rig.opener.requests.lock()[1].token;
    rig.provider
        .complete(failed, NativeLinkOutcome::Failed)
        .unwrap();

    let pushes = rig.observer.drain(8).unwrap().pushes;
    assert_eq!(pushes.len(), 2);
    assert_eq!(
        pushes[0].envelope.decode().unwrap(),
        json!({
            "type":"link.open.result",
            "id":"cancelled",
            "status":"denied",
            "error":"user-denied"
        })
    );
    assert_eq!(
        pushes[1].envelope.decode().unwrap(),
        json!({
            "type":"link.open.result",
            "id":"failed",
            "error":"native-open-failed"
        })
    );
}

#[test]
fn native_start_failure_rejects_without_an_invented_terminal_status() {
    let rig = Rig::new();
    *rig.opener.start_error.lock() = Some(NativeLinkStartError::Unavailable);
    assert_eq!(
        rig.dispatch(json!({
            "type":"link.open",
            "id":"unavailable",
            "url":"https://example.com/"
        }))
        .unwrap()
        .unwrap(),
        json!({
            "type":"link.open.result",
            "id":"unavailable",
            "error":"native link opener is unavailable"
        })
    );
    assert_eq!(rig.provider.pending_count(), 0);
}

#[test]
fn teardown_cancels_exact_pending_native_operation() {
    let rig = Rig::new();
    rig.dispatch(json!({
        "type":"link.open",
        "id":"open-1",
        "url":"https://example.com/"
    }))
    .unwrap();
    let cancellation = rig.opener.requests.lock()[0].cancellation.clone();
    rig.registry.close_session(rig.context.id);
    assert!(cancellation.is_cancelled());
    assert_eq!(
        rig.opener.cancelled.lock().as_slice(),
        &[Arc::from("link-1")]
    );
    assert_eq!(rig.provider.pending_count(), 0);
}

#[test]
fn normalized_public_ipv6_and_https_are_accepted() {
    assert!(validate_external_url("https://example.com", 1024).is_ok());
    assert!(validate_external_url("http://[2606:4700:4700::1111]/", 1024).is_ok());
    assert!(validate_external_url("http://[::1]/", 1024).is_err());
    assert!(validate_external_url("http://[fe80::1]/", 1024).is_err());
}
