mod support;

use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{ProviderPushError, ProviderSessionEnd};
use nmp_native_runtime_app::{
    AppErrorCode, InstalledBuildAvailability, PermissionPlatformAvailability, PlatformCommand,
    PlatformEvent,
};
use nmp_native_runtime_core::{
    AccountRef, ApprovedWrite, BindingRequest, Capability, CapabilityRequirement, ExecutionProfile,
    GrantDecision, Sensitivity, SessionId, SessionState, WriteReceiptId,
};
use nmp_native_runtime_store::{
    InstalledBuild, RuntimeStore, StoreLimits, UninstallCleanupPolicy, WorkspaceRecord,
};
use nmp_native_test_harness::FakeHostDataPlane;
use support::*;
use tempfile::TempDir;

#[test]
fn provider_lifecycle_is_source_bound_and_conflated_pushes_start_after_ready() {
    let rig = Rig::new(false);
    let principal = principal('b');
    rig.install(principal.clone());
    rig.allow_runtime(principal.clone());
    let session = rig.launch(principal.clone());

    let opened = rig.provider.opened.lock().clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].principal, principal);
    assert_eq!(opened[0].session, session);
    assert_eq!(rig.app.snapshot().provider_push_lanes.len(), 1);
    assert!(!rig.app.snapshot().provider_push_lanes[0].ready);
    assert_eq!(
        rig.app.snapshot().provider_push_lanes[0].source_window,
        opened[0].source_window
    );

    let sender = rig.provider.sender(session);
    let first = sender
        .push(
            "canary.state",
            serde_json::Map::from_iter([("value".to_owned(), serde_json::json!(1))]),
            Some("current"),
        )
        .unwrap();
    let second = sender
        .push(
            "canary.state",
            serde_json::Map::from_iter([("value".to_owned(), serde_json::json!(2))]),
            Some("current"),
        )
        .unwrap();
    assert!(second > first);
    assert!(!rig.app.events_after(0).events.into_iter().any(|event| {
        matches!(
            event.event,
            nmp_native_runtime_app::PlatformEvent::ProviderPush { .. }
        )
    }));

    rig.ready(session);
    let event = wait_for_event(&rig.app, |event| {
        matches!(
            event,
            nmp_native_runtime_app::PlatformEvent::ProviderPush {
                session: pushed_session,
                ..
            } if *pushed_session == session
        )
    });
    let nmp_native_runtime_app::PlatformEvent::ProviderPush {
        source_window,
        provider_sequence,
        domain,
        envelope,
        ..
    } = event
    else {
        unreachable!()
    };
    assert_eq!(source_window, opened[0].source_window);
    assert_eq!(provider_sequence, second);
    assert_eq!(domain, canary());
    assert_eq!(
        envelope.decode().unwrap(),
        serde_json::json!({"type": "canary.state", "value": 2})
    );
    assert_eq!(rig.provider.ready.lock().as_slice(), &[opened[0].clone()]);
    let lane = &rig.app.snapshot().provider_push_lanes[0];
    assert!(lane.ready);
    assert_eq!(lane.last_provider_sequence, Some(second));
    assert_eq!(lane.delivered_count, 1);

    rig.ready(session);
    assert_eq!(
        rig.provider.ready.lock().len(),
        1,
        "ready lifecycle is idempotent"
    );
    rig.app.dispatch(PlatformCommand::Stop { session });
    assert_eq!(
        rig.provider.closed.lock().as_slice(),
        &[(opened[0].clone(), ProviderSessionEnd::Stopped)]
    );
    assert_eq!(
        sender.push("canary.state", serde_json::Map::new(), None),
        Err(ProviderPushError::Closed)
    );
    assert_eq!(rig.app.snapshot().resources.admitted, 0);
}

#[test]
fn provider_push_authority_spoof_revoke_and_termination_fail_closed() {
    let rig = Rig::new(false);
    let principal = principal('b');
    rig.install(principal.clone());
    rig.allow_runtime(principal.clone());
    let session = rig.launch(principal.clone());
    let sender = rig.provider.sender(session);

    assert_eq!(
        sender.push(
            "canary.state",
            serde_json::Map::from_iter([(
                "principal".to_owned(),
                serde_json::json!(principal.clone())
            )]),
            None,
        ),
        Err(ProviderPushError::AuthorityField)
    );
    assert_eq!(
        sender.push("other.state", serde_json::Map::new(), None),
        Err(ProviderPushError::DomainMismatch)
    );

    rig.ready(session);
    rig.app.dispatch(PlatformCommand::Revoke {
        principal: principal.clone(),
        capability: canary(),
    });
    assert_eq!(
        sender.push("canary.state", serde_json::Map::new(), None),
        Err(ProviderPushError::Revoked)
    );
    assert_eq!(rig.provider.revoked.lock().len(), 1);
    assert_eq!(rig.provider.revoked.lock()[0].session, session);
    assert!(!rig.app.events_after(0).events.into_iter().any(|event| {
        matches!(
            event.event,
            nmp_native_runtime_app::PlatformEvent::ProviderPush { .. }
        )
    }));
    rig.app.dispatch(PlatformCommand::Crash {
        session,
        reason: Arc::from("test crash"),
    });
    assert_eq!(
        rig.provider.closed.lock().last().unwrap().1,
        ProviderSessionEnd::Crashed
    );
    assert_eq!(rig.app.snapshot().resources.admitted, 0);

    rig.allow_runtime(principal.clone());
    let replacement = rig.launch(principal);
    let replacement_sender = rig.provider.sender(replacement);
    rig.ready(replacement);
    replacement_sender.terminate(nmp_native_nap_bridge::ProviderPushTermination::ProviderFailure);
    let _ = wait_for_event(&rig.app, |event| {
        matches!(
            event,
            nmp_native_runtime_app::PlatformEvent::ProviderPushLaneClosed {
                session: closed_session,
                termination: Some(
                    nmp_native_nap_bridge::ProviderPushTermination::ProviderFailure
                ),
                ..
            } if *closed_session == replacement
        )
    });
    assert!(rig.app.snapshot().sessions.is_empty());
    assert_eq!(rig.app.snapshot().resources.admitted, 0);
    assert_eq!(
        rig.provider.closed.lock().last().unwrap().1,
        ProviderSessionEnd::Crashed
    );
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
    assert_eq!(
        rig.provider.closed.lock().last().unwrap().1,
        ProviderSessionEnd::RuntimeClosed
    );
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
            capability_requests: Vec::new(),
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
    assert_eq!(
        rig.app.snapshot().resources.admitted,
        3,
        "webview, provider delivery, and active provider operation are charged"
    );

    rig.app.dispatch(PlatformCommand::Revoke {
        principal: principal.clone(),
        capability: canary(),
    });
    assert_eq!(
        rig.app.snapshot().resources.admitted,
        2,
        "revocation cancels the domain operation while the session delivery lane remains"
    );
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
fn permission_review_is_exact_bounded_and_required_denial_blocks_launch() {
    let rig = Rig::new(false);
    let exact = principal('b');
    let missing = Capability::new("missing").unwrap();
    rig.install_with_requests(
        exact.clone(),
        vec![
            request(canary(), CapabilityRequirement::Required),
            request(missing.clone(), CapabilityRequirement::Optional),
        ],
    );

    let review = rig.app.permission_review(&exact).unwrap();
    assert_eq!(review.principal, exact);
    assert_eq!(review.capabilities.len(), 2);
    assert_eq!(review.capabilities[0].capability, canary());
    assert_eq!(
        review.capabilities[0].platform_availability,
        PermissionPlatformAvailability::Available
    );
    assert_eq!(
        review.capabilities[0].sensitivity,
        Some(Sensitivity::Ordinary)
    );
    assert_eq!(
        review.capabilities[1].platform_availability,
        PermissionPlatformAvailability::Unknown {
            reason: Arc::from(
                "no provider metadata is registered for this capability on this runtime"
            )
        }
    );
    assert_eq!(
        review.capabilities[1].requested_decision,
        Some(GrantDecision::Denied)
    );
    assert!(
        review.capabilities[1]
            .decision_options
            .iter()
            .all(|option| option.decision == GrantDecision::Denied || !option.valid)
    );

    rig.app.dispatch(PlatformCommand::ApplyPermissionBatch {
        principal: exact.clone(),
        decisions: vec![
            permission(canary(), GrantDecision::Denied),
            permission(missing, GrantDecision::Denied),
        ],
    });
    assert!(matches!(
        rig.app.events_after(0).events.last().unwrap().event,
        PlatformEvent::PermissionBatchApplied { .. }
    ));
    rig.app.dispatch(PlatformCommand::Launch {
        principal: exact,
        profile: ExecutionProfile::Legacy,
        required_domains: BTreeSet::from([canary()]),
    });
    assert!(rig.app.snapshot().sessions.is_empty());
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::Bridge
    );
}

#[test]
fn permission_batch_revokes_live_work_without_overwriting_ask_every_time() {
    let rig = Rig::new(true);
    let exact = principal('b');
    rig.install_with_requests(
        exact.clone(),
        vec![request(canary(), CapabilityRequirement::Required)],
    );
    rig.app.dispatch(PlatformCommand::ApplyPermissionBatch {
        principal: exact.clone(),
        decisions: vec![permission(canary(), GrantDecision::AllowExactBuild)],
    });
    let session = rig.launch(exact.clone());
    let sender = rig.provider.sender(session);
    rig.ready(session);
    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ping(serde_json::json!({})),
    });
    assert_eq!(rig.app.snapshot().resources.admitted, 3);

    rig.app.dispatch(PlatformCommand::ApplyPermissionBatch {
        principal: exact.clone(),
        decisions: vec![permission(canary(), GrantDecision::AskEveryTime)],
    });

    assert_eq!(rig.app.snapshot().resources.admitted, 2);
    assert_eq!(
        rig.app.permission_review(&exact).unwrap().capabilities[0].current_decision,
        GrantDecision::AskEveryTime
    );
    assert_eq!(
        rig.store.grant(&exact, &canary()).unwrap(),
        GrantDecision::AskEveryTime
    );
    assert_eq!(
        sender.push("canary.state", serde_json::Map::new(), None),
        Err(ProviderPushError::Revoked)
    );
    assert_eq!(rig.provider.revoked.lock().len(), 1);
}

#[test]
fn permission_batch_store_failure_changes_neither_ledger_nor_outcome() {
    let rig = Rig::new(false);
    let exact = principal('b');
    rig.install_with_requests(
        exact.clone(),
        vec![request(canary(), CapabilityRequirement::Required)],
    );
    rig.store
        .uninstall_exact_build(&exact, UninstallCleanupPolicy::RuntimeOwnedExactBuildState)
        .unwrap();

    rig.app.dispatch(PlatformCommand::ApplyPermissionBatch {
        principal: exact.clone(),
        decisions: vec![permission(canary(), GrantDecision::AllowExactBuild)],
    });

    assert_eq!(
        rig.app.permission_review(&exact).unwrap().capabilities[0].current_decision,
        GrantDecision::Denied
    );
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::Store
    );
    assert!(
        !rig.app
            .events_after(0)
            .events
            .iter()
            .any(|event| matches!(event.event, PlatformEvent::PermissionBatchApplied { .. }))
    );
}

#[test]
fn permission_batch_cannot_override_managed_host_policy() {
    let rig = Rig::new(false);
    let exact = principal('b');
    rig.install_with_requests(
        exact.clone(),
        vec![request(canary(), CapabilityRequirement::Required)],
    );
    rig.store
        .set_grant(&exact, &canary(), GrantDecision::Managed)
        .unwrap();
    let review = rig.app.permission_review(&exact).unwrap();
    assert_eq!(
        review.capabilities[0].current_decision,
        GrantDecision::Managed
    );
    assert_eq!(review.capabilities[0].requested_decision, None);

    rig.app.dispatch(PlatformCommand::ApplyPermissionBatch {
        principal: exact.clone(),
        decisions: vec![permission(canary(), GrantDecision::Denied)],
    });

    assert_eq!(
        rig.store.grant(&exact, &canary()).unwrap(),
        GrantDecision::Managed
    );
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::Grant
    );
}

#[test]
fn permission_batch_persists_and_dependency_policy_is_owner_validated() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("runtime.db");
    let store = Arc::new(RuntimeStore::open(&path, StoreLimits::default()).unwrap());
    let host = Arc::new(FakeHostDataPlane::new(16));
    let dependency = Capability::new("identity").unwrap();
    let provider = Arc::new(CapturingProvider::new(false).with_dependencies([dependency]));
    let (app, _) = open_app(Arc::clone(&store), host, provider);
    let exact = principal('b');
    app.dispatch(PlatformCommand::InstallVerified {
        build: InstalledBuild {
            principal: exact.clone(),
            title: Arc::from("Dependent napplet"),
            manifest_metadata: json(serde_json::json!({"kind": 35129})),
            capability_requests: vec![request(canary(), CapabilityRequirement::Required)],
        },
        artifact: Arc::new(TestArtifact {
            kind: 35_129,
            author: exact.manifest_author().to_owned(),
            d_tag: exact.d_tag().to_owned(),
            aggregate: exact.aggregate_hash().to_owned(),
        }),
    });
    app.dispatch(PlatformCommand::ApplyPermissionBatch {
        principal: exact.clone(),
        decisions: vec![permission(canary(), GrantDecision::AllowExactBuild)],
    });
    assert_eq!(
        app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::Grant
    );

    app.dispatch(PlatformCommand::SetGrant {
        principal: exact.clone(),
        capability: Capability::new("identity").unwrap(),
        sensitivity: Sensitivity::Sensitive,
        decision: GrantDecision::AllowExactBuild,
    });
    app.dispatch(PlatformCommand::ApplyPermissionBatch {
        principal: exact.clone(),
        decisions: vec![permission(canary(), GrantDecision::AllowExactBuild)],
    });
    assert_eq!(
        app.permission_review(&exact).unwrap().capabilities[0].current_decision,
        GrantDecision::AllowExactBuild
    );
    app.dispatch(PlatformCommand::Close);
    drop(app);
    drop(store);

    let reopened_store = Arc::new(RuntimeStore::open(&path, StoreLimits::default()).unwrap());
    let reopened_host = Arc::new(FakeHostDataPlane::new(16));
    let reopened_provider = Arc::new(
        CapturingProvider::new(false).with_dependencies([Capability::new("identity").unwrap()]),
    );
    let (reopened, _) = open_app(reopened_store, reopened_host, reopened_provider);
    assert_eq!(
        reopened.permission_review(&exact).unwrap().capabilities[0].current_decision,
        GrantDecision::AllowExactBuild
    );
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
            capability_requests: Vec::new(),
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
    let first_sender = rig.provider.sender(first);
    let second_sender = rig.provider.sender(second);
    assert_ne!(
        first_sender.source_window(),
        second_sender.source_window(),
        "each launch owns an opaque source-window identity"
    );
    for session in [first, second] {
        rig.ready(session);
        rig.app.dispatch(PlatformCommand::MappedEnvelope {
            session,
            bytes: ping(serde_json::json!({})),
        });
    }
    assert_eq!(rig.app.snapshot().resources.admitted, 6);

    rig.app.dispatch(PlatformCommand::Revoke {
        principal: first_principal,
        capability: canary(),
    });
    assert_eq!(
        rig.app.snapshot().resources.admitted,
        5,
        "only the revoked exact build's operation is cancelled"
    );
    assert_eq!(
        first_sender.push("canary.state", serde_json::Map::new(), None),
        Err(ProviderPushError::Revoked)
    );
    second_sender
        .push(
            "canary.state",
            serde_json::Map::from_iter([("owner".to_owned(), serde_json::json!("second"))]),
            None,
        )
        .unwrap();
    let _ = wait_for_event(&rig.app, |event| {
        matches!(
            event,
            nmp_native_runtime_app::PlatformEvent::ProviderPush {
                session: pushed_session,
                ..
            } if *pushed_session == second
        )
    });
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
fn library_filter_and_metadata_restore_are_bounded_and_offline_honest() {
    let rig = Rig::new(false);
    let first = principal('b');
    let second = principal('c');
    rig.install(first.clone());
    rig.install(second.clone());
    assert_eq!(rig.app.snapshot().library.total_installed, 2);
    assert!(
        rig.app.snapshot().library.builds.iter().all(|build| {
            build.availability == InstalledBuildAvailability::SealedExactBytesReady
        })
    );

    rig.app.dispatch(PlatformCommand::SetLibraryFilter {
        query: Arc::from(second.aggregate_hash()),
    });
    assert_eq!(rig.app.snapshot().library.builds.len(), 1);
    assert_eq!(rig.app.snapshot().library.builds[0].build.principal, second);
    rig.app.dispatch(PlatformCommand::Close);

    let (restored, _) = open_app(
        Arc::clone(&rig.store),
        rig.host.clone(),
        rig.provider.clone(),
    );
    let snapshot = restored.snapshot();
    assert_eq!(snapshot.library.total_installed, 2);
    assert!(snapshot.library.builds.iter().all(|build| {
        build.availability == InstalledBuildAvailability::MetadataOnly
            && build.active_sessions.is_empty()
    }));
    restored.dispatch(PlatformCommand::Launch {
        principal: first,
        profile: ExecutionProfile::Legacy,
        required_domains: BTreeSet::new(),
    });
    assert_eq!(
        restored.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::OfflineBytesUnavailable
    );
}

#[test]
fn suspend_resume_is_typed_and_stale_session_handles_remain_inert() {
    let rig = Rig::new(false);
    let principal = principal('b');
    rig.install(principal.clone());
    rig.allow_runtime(principal.clone());
    let session = rig.launch(principal);

    rig.app.dispatch(PlatformCommand::Suspend { session });
    assert_eq!(
        rig.app.snapshot().sessions[0].state,
        SessionState::Suspended
    );
    rig.app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ready(),
    });
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::InvalidLifecycle
    );

    rig.app.dispatch(PlatformCommand::Resume { session });
    assert_eq!(rig.app.snapshot().sessions[0].state, SessionState::Running);
    rig.ready(session);
    assert!(rig.shell_provider.is_ready(session));

    rig.app.dispatch(PlatformCommand::Stop { session });
    rig.app.dispatch(PlatformCommand::Resume { session });
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::UnknownSession
    );
}

#[test]
fn uninstall_stops_only_exact_build_and_cleans_runtime_owned_state() {
    let rig = Rig::new(false);
    let removed = principal('b');
    let retained = principal('c');
    for principal in [removed.clone(), retained.clone()] {
        rig.install(principal.clone());
        rig.allow_runtime(principal);
    }
    rig.store
        .put_component_value(&removed, "storage", "draft", b"gm")
        .unwrap();
    let receipt_id = WriteReceiptId(Arc::from("nmp-owned-receipt"));
    rig.app.dispatch(PlatformCommand::SaveWorkspace {
        workspace: WorkspaceRecord {
            id: Arc::from("main"),
            definition: json(serde_json::json!({"layout": "two-up"})),
            retained_receipts: vec![receipt_id.clone()],
        },
    });
    rig.app.dispatch(PlatformCommand::AssignWorkspaceBuild {
        workspace_id: Arc::from("main"),
        principal: removed.clone(),
    });
    let removed_session = rig.launch(removed.clone());
    let retained_session = rig.launch(retained.clone());
    assert_eq!(rig.app.snapshot().sessions.len(), 2);

    rig.app.dispatch(PlatformCommand::Uninstall {
        principal: removed.clone(),
        cleanup: UninstallCleanupPolicy::RuntimeOwnedExactBuildState,
    });

    let snapshot = rig.app.snapshot();
    assert_eq!(snapshot.library.total_installed, 1);
    assert_eq!(snapshot.library.builds[0].build.principal, retained);
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].id, retained_session);
    assert_eq!(
        rig.store
            .component_value(&removed, "storage", "draft")
            .unwrap(),
        None
    );
    assert_eq!(
        rig.store.grant(&removed, &canary()).unwrap(),
        GrantDecision::Denied
    );
    assert!(rig.store.workspace_assignments("main").unwrap().is_empty());
    assert_eq!(
        rig.store.load_workspaces().unwrap()[0].retained_receipts,
        [receipt_id]
    );
    assert!(snapshot.workspaces[0].assigned_builds.is_empty());

    rig.app.dispatch(PlatformCommand::Resume {
        session: removed_session,
    });
    assert_eq!(
        rig.app.snapshot().recent_errors.last().unwrap().code,
        AppErrorCode::UnknownSession
    );
    rig.app.dispatch(PlatformCommand::Stop {
        session: retained_session,
    });
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
