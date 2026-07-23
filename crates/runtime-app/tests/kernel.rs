use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use nmp_native_nap_bridge::{
    BridgeLimits, Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderRequest,
};
use nmp_native_providers::{
    ShellEnvironment, ShellEnvironmentError, ShellEnvironmentLimits, ShellEnvironmentSource,
    ShellProvider, ShellProviderLimits,
};
use nmp_native_runtime_app::{
    AppErrorCode, AppLimits, ExecutableArtifact, KernelClock, PlatformCommand, RuntimeApp,
    RuntimeAppConfig,
};
use nmp_native_runtime_core::{
    AccountRef, ApprovedWrite, BindingRequest, BoundedJson, Capability, ExecutionProfile,
    GrantDecision, GrantLimits, Principal, ResourceLimits, Sensitivity, SessionId,
};
use nmp_native_runtime_store::{InstalledBuild, RuntimeStore, StoreLimits, WorkspaceRecord};
use nmp_native_surface::BindingLimits;
use nmp_native_test_harness::FakeHostDataPlane;
use parking_lot::Mutex;
use serde_json::Value;
use tempfile::TempDir;

#[derive(Debug)]
struct TestClock(AtomicU64);

impl TestClock {
    fn new(now: u64) -> Self {
        Self(AtomicU64::new(now))
    }
}

impl KernelClock for TestClock {
    fn now_millis(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel)
    }
}

#[derive(Debug)]
struct TestArtifact {
    kind: u16,
    author: String,
    d_tag: String,
    aggregate: String,
}

impl ExecutableArtifact for TestArtifact {
    fn manifest_kind(&self) -> u16 {
        self.kind
    }

    fn manifest_author(&self) -> &str {
        &self.author
    }

    fn d_tag(&self) -> Option<&str> {
        Some(&self.d_tag)
    }

    fn aggregate_hash(&self) -> &str {
        &self.aggregate
    }

    fn contains_logical_path(&self, logical_path: &str) -> bool {
        logical_path == "/index.html"
    }
}

#[derive(Debug)]
struct CapturingProvider {
    descriptor: ProviderDescriptor,
    seen: Mutex<Vec<(Principal, SessionId, Value)>>,
    streaming: bool,
}

impl CapturingProvider {
    fn new(streaming: bool) -> Self {
        Self {
            descriptor: ProviderDescriptor {
                domain: canary(),
                protocol_versions: BTreeSet::from([Arc::from("internal-canary/1")]),
                actions: BTreeSet::from([Arc::from("ping")]),
                sensitive: false,
            },
            seen: Mutex::new(Vec::new()),
            streaming,
        }
    }
}

impl Provider for CapturingProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        self.seen
            .lock()
            .push((request.principal, request.session, request.payload));
        if self.streaming {
            Ok(ProviderCall::streaming(None, request.work))
        } else {
            Ok(ProviderCall::completed(None))
        }
    }
}

#[derive(Debug)]
struct FixedShellEnvironment {
    override_domains: Option<BTreeSet<Capability>>,
}

impl ShellEnvironmentSource for FixedShellEnvironment {
    fn environment(
        &self,
        _principal: &Principal,
        _session: SessionId,
        offered_domains: &BTreeSet<Capability>,
    ) -> Result<ShellEnvironment, ShellEnvironmentError> {
        ShellEnvironment::new(
            self.override_domains
                .as_ref()
                .unwrap_or(offered_domains)
                .iter()
                .cloned(),
            [Arc::from("settings")],
            ShellEnvironmentLimits::default(),
        )
    }
}

struct Rig {
    _directory: TempDir,
    store: Arc<RuntimeStore>,
    host: Arc<FakeHostDataPlane>,
    provider: Arc<CapturingProvider>,
    shell_provider: Arc<ShellProvider>,
    app: Arc<RuntimeApp>,
}

impl Rig {
    fn new(streaming: bool) -> Self {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(
            RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default())
                .unwrap(),
        );
        let host = Arc::new(FakeHostDataPlane::new(16));
        let provider = Arc::new(CapturingProvider::new(streaming));
        let (app, shell_provider) = open_app(Arc::clone(&store), host.clone(), provider.clone());
        Self {
            _directory: directory,
            store,
            host,
            provider,
            shell_provider,
            app,
        }
    }

    fn install(&self, principal: Principal) {
        self.app.dispatch(PlatformCommand::InstallVerified {
            build: InstalledBuild {
                principal: principal.clone(),
                title: Arc::from("Test napplet"),
                manifest_metadata: json(serde_json::json!({"kind": 34128})),
            },
            artifact: Arc::new(TestArtifact {
                kind: 35_129,
                author: principal.manifest_author().to_owned(),
                d_tag: principal.d_tag().to_owned(),
                aggregate: principal.aggregate_hash().to_owned(),
            }),
        });
    }

    fn allow_runtime(&self, principal: Principal) {
        self.app.dispatch(PlatformCommand::SetGrant {
            principal,
            capability: canary(),
            sensitivity: Sensitivity::Ordinary,
            decision: GrantDecision::AllowExactBuild,
        });
    }

    fn launch(&self, principal: Principal) -> SessionId {
        self.app.dispatch(PlatformCommand::Launch {
            principal,
            profile: ExecutionProfile::Legacy,
            required_domains: BTreeSet::from([canary()]),
        });
        self.app.snapshot().sessions.last().unwrap().id
    }

    fn ready(&self, session: SessionId) {
        self.app.dispatch(PlatformCommand::MappedEnvelope {
            session,
            bytes: ready(),
        });
    }
}

fn open_app(
    store: Arc<RuntimeStore>,
    host: Arc<FakeHostDataPlane>,
    provider: Arc<CapturingProvider>,
) -> (Arc<RuntimeApp>, Arc<ShellProvider>) {
    open_app_with_shell_source(
        store,
        host,
        provider,
        Arc::new(FixedShellEnvironment {
            override_domains: None,
        }),
    )
}

fn open_app_with_shell_domains(
    store: Arc<RuntimeStore>,
    host: Arc<FakeHostDataPlane>,
    provider: Arc<CapturingProvider>,
    shell_domains: BTreeSet<Capability>,
) -> (Arc<RuntimeApp>, Arc<ShellProvider>) {
    open_app_with_shell_source(
        store,
        host,
        provider,
        Arc::new(FixedShellEnvironment {
            override_domains: Some(shell_domains),
        }),
    )
}

fn open_app_with_shell_source(
    store: Arc<RuntimeStore>,
    host: Arc<FakeHostDataPlane>,
    provider: Arc<CapturingProvider>,
    shell_environment: Arc<dyn ShellEnvironmentSource>,
) -> (Arc<RuntimeApp>, Arc<ShellProvider>) {
    let data_plane: Arc<dyn nmp_native_runtime_core::HostDataPlane> = host;
    let provider: Arc<dyn Provider> = provider;
    let shell_provider =
        Arc::new(ShellProvider::new(shell_environment, ShellProviderLimits::default()).unwrap());
    let app = RuntimeApp::open(RuntimeAppConfig {
        limits: AppLimits::default(),
        resource_limits: ResourceLimits::default(),
        grant_limits: GrantLimits::default(),
        bridge_limits: BridgeLimits::default(),
        binding_limits: BindingLimits::default(),
        store,
        data_plane,
        clock: Arc::new(TestClock::new(1_000)),
        shell_provider: shell_provider.clone(),
        providers: vec![provider],
    })
    .unwrap();
    (app, shell_provider)
}

fn principal(hash: char) -> Principal {
    Principal::new("a".repeat(64), "test-napplet", hash.to_string().repeat(64)).unwrap()
}

fn shell() -> Capability {
    Capability::new("shell").unwrap()
}

fn canary() -> Capability {
    Capability::new("canary").unwrap()
}

fn json(value: Value) -> BoundedJson {
    BoundedJson::from_value(&value, 16 * 1024).unwrap()
}

fn ping(payload: Value) -> Arc<[u8]> {
    Arc::from(
        serde_json::to_vec(&serde_json::json!({
            "type": "canary.ping",
            "id": "request-1",
            "payload": payload
        }))
        .unwrap(),
    )
}

fn ready() -> Arc<[u8]> {
    Arc::from(serde_json::to_vec(&serde_json::json!({"type": "shell.ready"})).unwrap())
}

fn mapped(value: Value) -> Arc<[u8]> {
    Arc::from(serde_json::to_vec(&value).unwrap())
}

#[test]
fn nap_shell_gates_capabilities_and_emits_exactly_one_uncorrelated_init() {
    let rig = Rig::new(false);
    let principal = principal('b');
    rig.install(principal.clone());
    rig.allow_runtime(principal.clone());
    let session = rig.launch(principal);
    assert_eq!(
        rig.app.snapshot().session_domains,
        vec![nmp_native_runtime_app::SessionDomainView {
            session,
            domains: vec![canary(), shell()],
        }]
    );

    for unknown in [
        serde_json::json!({"type": "future.unknown"}),
        serde_json::json!({"type": "canary.future"}),
    ] {
        rig.app.dispatch(PlatformCommand::MappedEnvelope {
            session,
            bytes: mapped(unknown),
        });
    }
    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ping(serde_json::json!({})),
    });
    assert!(rig.provider.seen.lock().is_empty());
    assert!(!rig.shell_provider.is_ready(session));
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::Bridge
    );
    assert_eq!(
        rig.app
            .events_after(0)
            .events
            .into_iter()
            .filter(|item| matches!(
                item.event,
                nmp_native_runtime_app::PlatformEvent::EnvelopeIgnored { .. }
            ))
            .count(),
        2,
        "unknown well-formed messages remain forward-compatible before readiness"
    );

    rig.ready(session);
    assert!(rig.shell_provider.is_ready(session));
    let first_init = rig
        .app
        .events_after(0)
        .events
        .into_iter()
        .filter_map(|item| match item.event {
            nmp_native_runtime_app::PlatformEvent::EnvelopeHandled {
                session: handled_session,
                response: Some(response),
                ..
            } if handled_session == session => Some(response.decode().unwrap()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        first_init,
        vec![serde_json::json!({
            "type": "shell.init",
            "capabilities": {"domains": ["canary", "shell"]},
            "services": ["settings"]
        })]
    );
    assert!(
        first_init[0].get("id").is_none(),
        "shell.init is uncorrelated"
    );

    rig.ready(session);
    let init_count = rig
        .app
        .events_after(0)
        .events
        .into_iter()
        .filter(|item| {
            matches!(
                &item.event,
                nmp_native_runtime_app::PlatformEvent::EnvelopeHandled {
                    response: Some(response),
                    ..
                } if response.decode().unwrap()["type"] == "shell.init"
            )
        })
        .count();
    assert_eq!(init_count, 1, "a replay must not resend shell.init");

    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ping(serde_json::json!({})),
    });
    assert_eq!(rig.provider.seen.lock().len(), 1);
}

#[test]
fn nap_shell_rejects_id_extra_fields_and_payload_identity_claims() {
    let rig = Rig::new(false);
    let real = principal('b');
    let forged = principal('c');
    rig.install(real.clone());
    rig.allow_runtime(real.clone());
    let session = rig.launch(real);

    for invalid in [
        serde_json::json!({"type": "shell.ready", "id": "forbidden"}),
        serde_json::json!({"type": "shell.ready", "id": null}),
        serde_json::json!({"type": "shell.ready", "capabilities": ["storage"]}),
        serde_json::json!({
            "type": "shell.ready",
            "principal": forged,
            "session": 9_999
        }),
    ] {
        rig.app.dispatch(PlatformCommand::MappedEnvelope {
            session,
            bytes: mapped(invalid),
        });
        assert!(!rig.shell_provider.is_ready(session));
        assert_eq!(
            rig.app.snapshot().recent_errors.last().unwrap().code,
            AppErrorCode::Bridge
        );
    }

    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ping(serde_json::json!({})),
    });
    assert!(
        rig.provider.seen.lock().is_empty(),
        "invalid readiness never opens another capability"
    );
}

#[test]
fn nap_shell_state_is_closed_and_never_reused_by_a_relaunch() {
    let rig = Rig::new(false);
    let principal = principal('b');
    rig.install(principal.clone());
    rig.allow_runtime(principal.clone());
    let first = rig.launch(principal.clone());
    rig.ready(first);
    assert!(rig.shell_provider.is_ready(first));

    rig.app.dispatch(PlatformCommand::Stop { session: first });
    assert!(!rig.shell_provider.is_ready(first));
    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session: first,
        bytes: ready(),
    });
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::UnknownSession
    );
    assert!(!rig.shell_provider.is_ready(first));

    let second = rig.launch(principal);
    assert!(second.0 > first.0);
    assert!(!rig.shell_provider.is_ready(second));
    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session: second,
        bytes: ping(serde_json::json!({})),
    });
    assert!(rig.provider.seen.lock().is_empty());
    rig.ready(second);
    assert!(rig.shell_provider.is_ready(second));

    rig.app.dispatch(PlatformCommand::Close);
    assert!(!rig.shell_provider.is_ready(second));
}

#[test]
fn launch_is_refused_when_shell_environment_differs_from_the_session_plan() {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(
        RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default()).unwrap(),
    );
    let host = Arc::new(FakeHostDataPlane::new(16));
    let provider = Arc::new(CapturingProvider::new(false));
    let (app, shell_provider) =
        open_app_with_shell_domains(store, host, provider.clone(), BTreeSet::from([shell()]));
    let principal = principal('b');
    app.dispatch(PlatformCommand::InstallVerified {
        build: InstalledBuild {
            principal: principal.clone(),
            title: Arc::from("Test napplet"),
            manifest_metadata: json(serde_json::json!({"kind": 34128})),
        },
        artifact: Arc::new(TestArtifact {
            kind: 35_129,
            author: principal.manifest_author().to_owned(),
            d_tag: principal.d_tag().to_owned(),
            aggregate: principal.aggregate_hash().to_owned(),
        }),
    });
    app.dispatch(PlatformCommand::SetGrant {
        principal: principal.clone(),
        capability: canary(),
        sensitivity: Sensitivity::Ordinary,
        decision: GrantDecision::AllowExactBuild,
    });
    app.dispatch(PlatformCommand::Launch {
        principal,
        profile: ExecutionProfile::Legacy,
        required_domains: BTreeSet::from([canary()]),
    });
    assert!(app.snapshot().sessions.is_empty());
    assert!(!shell_provider.is_ready(SessionId(1)));
    assert_eq!(
        app.snapshot().recent_errors.last().unwrap().detail.as_ref(),
        "shell environment does not equal the exact negotiated capability set"
    );
    assert!(!app.events_after(0).events.into_iter().any(|item| matches!(
        item.event,
        nmp_native_runtime_app::PlatformEvent::EnvelopeHandled {
            response: Some(_),
            ..
        }
    )));
}

#[test]
fn stop_crash_and_revoke_return_session_resources_without_closing_binding() {
    let rig = Rig::new(true);
    let principal = principal('b');
    rig.install(principal.clone());
    rig.allow_runtime(principal.clone());
    rig.app.dispatch(PlatformCommand::OpenBinding {
        request: BindingRequest {
            workspace_binding_id: Arc::from("feed"),
            family: Arc::from("event.collection"),
            schema: Arc::from("nostr.events.collection/1"),
            parameters: json(serde_json::json!({"authors": [principal.manifest_author()]})),
            maximum_rows: 50,
            maximum_frame_bytes: 64 * 1024,
        },
    });

    let first = rig.launch(principal.clone());
    assert_eq!(rig.app.snapshot().resources.admitted, 1);
    rig.ready(first);
    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session: first,
        bytes: ping(serde_json::json!({})),
    });
    assert_eq!(rig.app.snapshot().resources.admitted, 2);

    rig.app.dispatch(PlatformCommand::Revoke {
        principal: principal.clone(),
        capability: canary(),
    });
    assert_eq!(rig.app.snapshot().resources.admitted, 1);
    rig.app.dispatch(PlatformCommand::Crash {
        session: first,
        reason: Arc::from("web-content-process-exited"),
    });
    assert_eq!(rig.app.snapshot().resources.admitted, 0);
    assert_eq!(rig.host.binding_count(), 1);
    assert!(rig.app.binding("feed").is_some());

    rig.allow_runtime(principal.clone());
    let second = rig.launch(principal);
    assert!(second.0 > first.0);
    rig.app.dispatch(PlatformCommand::Stop { session: second });
    assert_eq!(rig.app.snapshot().resources.admitted, 0);
    assert_eq!(rig.host.binding_count(), 1);

    rig.app.dispatch(PlatformCommand::Close);
    assert_eq!(rig.host.binding_count(), 0);
}

#[test]
fn snapshot_manifest_without_d_tag_is_a_typed_identity_refusal() {
    let rig = Rig::new(false);
    let principal = principal('b');
    rig.app.dispatch(PlatformCommand::InstallVerified {
        build: InstalledBuild {
            principal: principal.clone(),
            title: Arc::from("Snapshot napplet"),
            manifest_metadata: json(serde_json::json!({"kind": 5129})),
        },
        artifact: Arc::new(TestArtifact {
            kind: 5_129,
            author: principal.manifest_author().to_owned(),
            d_tag: String::new(),
            aggregate: principal.aggregate_hash().to_owned(),
        }),
    });
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::UnsupportedManifestIdentity
    );
    assert!(rig.app.snapshot().sessions.is_empty());
    assert!(rig.store.installed_builds().unwrap().is_empty());
}

#[test]
fn exact_build_revoke_does_not_cancel_another_principals_operation() {
    let rig = Rig::new(true);
    let first_principal = principal('b');
    let second_principal = principal('c');
    for principal in [first_principal.clone(), second_principal.clone()] {
        rig.install(principal.clone());
        rig.allow_runtime(principal);
    }
    let first = rig.launch(first_principal.clone());
    let second = rig.launch(second_principal);
    for session in [first, second] {
        rig.ready(session);
        rig.app.dispatch(PlatformCommand::MappedEnvelope {
            session,
            bytes: ping(serde_json::json!({})),
        });
    }
    assert_eq!(rig.app.snapshot().resources.admitted, 4);

    rig.app.dispatch(PlatformCommand::Revoke {
        principal: first_principal,
        capability: canary(),
    });
    assert_eq!(
        rig.app.snapshot().resources.admitted,
        3,
        "only the revoked exact build's operation is cancelled"
    );
    rig.app.dispatch(PlatformCommand::Stop { session: first });
    rig.app.dispatch(PlatformCommand::Stop { session: second });
    assert_eq!(rig.app.snapshot().resources.admitted, 0);
}

#[test]
fn mapped_payload_identity_is_ignored_and_stale_session_is_refused() {
    let rig = Rig::new(false);
    let real = principal('b');
    let forged = principal('c');
    rig.install(real.clone());
    rig.allow_runtime(real.clone());
    let session = rig.launch(real.clone());
    rig.ready(session);

    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ping(serde_json::json!({
            "principal": forged,
            "session": 9_999,
            "profile": "hybrid"
        })),
    });
    let seen = rig.provider.seen.lock();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, real);
    assert_eq!(seen[0].1, session);
    drop(seen);

    rig.app.dispatch(PlatformCommand::Stop { session });
    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ping(serde_json::json!({})),
    });
    assert_eq!(rig.provider.seen.lock().len(), 1);
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::UnknownSession
    );

    rig.allow_runtime(real.clone());
    let replacement = rig.launch(real);
    assert!(
        replacement.0 > session.0,
        "session ids are never caller-reused"
    );
}

#[test]
fn accepted_durable_receipt_outlives_origin_session_and_keeps_frozen_account() {
    let rig = Rig::new(false);
    let principal = principal('b');
    rig.install(principal.clone());
    rig.allow_runtime(principal.clone());
    let session = rig.launch(principal.clone());
    let account = AccountRef(Arc::from("account-a"));

    rig.app.dispatch(PlatformCommand::ApproveWrite {
        write: ApprovedWrite {
            approval_id: Arc::from("approval-1"),
            origin_principal: principal,
            origin_session: session,
            account: account.clone(),
            draft: json(serde_json::json!({
                "author": "account-a",
                "kind": 1,
                "content": "hello"
            })),
        },
    });
    let receipt_id = rig.app.snapshot().receipts[0].receipt_id.clone();
    assert_eq!(rig.host.receipt_count(), 1);
    assert_eq!(
        rig.app
            .receipt(&receipt_id)
            .unwrap()
            .view()
            .unwrap()
            .latest
            .unwrap()
            .state
            .decode()
            .unwrap()["stage"],
        "accepted"
    );

    rig.app.dispatch(PlatformCommand::Stop { session });
    assert_eq!(rig.app.snapshot().resources.admitted, 0);
    assert!(
        rig.app.receipt(&receipt_id).is_some(),
        "receipt ownership belongs to the application, not its origin session"
    );
    let accepted_event = rig
        .app
        .events_after(0)
        .events
        .into_iter()
        .find_map(|item| match item.event {
            nmp_native_runtime_app::PlatformEvent::WriteAccepted {
                receipt_id,
                frozen_account,
            } => Some((receipt_id, frozen_account)),
            _ => None,
        })
        .unwrap();
    assert_eq!(accepted_event.0, receipt_id);
    assert_eq!(accepted_event.1, account);
}

#[test]
fn write_with_caller_selected_principal_is_refused_before_acceptance() {
    let rig = Rig::new(false);
    let real = principal('b');
    let forged = principal('c');
    rig.install(real.clone());
    rig.allow_runtime(real.clone());
    let session = rig.launch(real);

    rig.app.dispatch(PlatformCommand::ApproveWrite {
        write: ApprovedWrite {
            approval_id: Arc::from("forged-approval"),
            origin_principal: forged,
            origin_session: session,
            account: AccountRef(Arc::from("account-a")),
            draft: json(serde_json::json!({"kind": 1, "content": "forged"})),
        },
    });
    assert_eq!(rig.host.receipt_count(), 0);
    assert!(rig.app.snapshot().receipts.is_empty());
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::SessionIdentityMismatch
    );
}

#[test]
fn restoration_reattaches_only_explicit_receipt_ids_not_workspace_json() {
    let rig = Rig::new(false);
    let principal = principal('b');
    rig.install(principal.clone());
    rig.allow_runtime(principal.clone());
    let session = rig.launch(principal.clone());
    rig.app.dispatch(PlatformCommand::ApproveWrite {
        write: ApprovedWrite {
            approval_id: Arc::from("approval-1"),
            origin_principal: principal,
            origin_session: session,
            account: AccountRef(Arc::from("account-a")),
            draft: json(serde_json::json!({"kind": 1, "content": "restore me"})),
        },
    });
    let retained = rig.app.snapshot().receipts[0].receipt_id.clone();
    let injected = WriteReceiptIdForTest::value();
    rig.app.dispatch(PlatformCommand::SaveWorkspace {
        workspace: WorkspaceRecord {
            id: Arc::from("main"),
            definition: json(serde_json::json!({
                "slots": ["feed"],
                "receipt_id": injected.0.as_ref()
            })),
            retained_receipts: vec![retained.clone()],
        },
    });
    rig.app.dispatch(PlatformCommand::Close);

    let (restored, _) = open_app(
        Arc::clone(&rig.store),
        rig.host.clone(),
        rig.provider.clone(),
    );
    restored.dispatch(PlatformCommand::RestoreWorkspaces);
    let snapshot = restored.snapshot();
    assert_eq!(snapshot.workspaces.len(), 1);
    assert_eq!(snapshot.receipts.len(), 1);
    assert_eq!(snapshot.receipts[0].receipt_id, retained);
    assert!(restored.receipt(&injected).is_none());
}

struct WriteReceiptIdForTest;

impl WriteReceiptIdForTest {
    fn value() -> nmp_native_runtime_core::WriteReceiptId {
        nmp_native_runtime_core::WriteReceiptId(Arc::from("fake-receipt-999"))
    }
}
