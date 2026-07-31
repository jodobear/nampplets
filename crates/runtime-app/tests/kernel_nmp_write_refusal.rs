//! Actual NMP-provider refusal delivery across destructive lifecycle changes.

mod support;

use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{BridgeLimits, Provider};
use nmp_native_nmp_adapter::{NapNostrProviderLimits, NapNostrProviderSet, NmpDataPlane};
use nmp_native_providers::{ShellProvider, ShellProviderLimits};
use nmp_native_runtime_app::{
    AppLimits, PlatformCommand, PlatformEvent, RuntimeApp, RuntimeAppConfig,
};
use nmp_native_runtime_core::{
    Capability, CapabilityRequirement, ExecutionProfile, GrantDecision, GrantLimits, HostDataPlane,
    ResourceLimits, Sensitivity,
};
use nmp_native_runtime_store::{
    InstalledBuild, PermissionDefaultPreference, RuntimeStore, StoreLimits,
};
use nmp_native_surface::BindingLimits;
use support::{
    FixedShellEnvironment, TestArtifact, TestClock, json, mapped, principal, ready, request,
};
use tempfile::TempDir;

#[derive(Clone, Copy)]
enum Lifecycle {
    Revoke,
    Stop,
}

#[test]
fn nmp_publish_system_refusal_survives_revoke_and_session_close() {
    assert_nmp_publish_refusal(Lifecycle::Revoke, "permission revoked");
    assert_nmp_publish_refusal(Lifecycle::Stop, "provider operation cancelled");
}

fn assert_nmp_publish_refusal(lifecycle: Lifecycle, expected_error: &str) {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(
        RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default()).unwrap(),
    );
    let plane = Arc::new(NmpDataPlane::open(Default::default(), 8).unwrap());
    let providers =
        NapNostrProviderSet::new(Arc::clone(&plane), NapNostrProviderLimits::default()).unwrap();
    let relay_provider: Arc<dyn Provider> = providers.relay;
    let shell_provider = Arc::new(
        ShellProvider::new(
            Arc::new(FixedShellEnvironment {
                override_domains: None,
            }),
            ShellProviderLimits::default(),
        )
        .unwrap(),
    );
    let data_plane: Arc<dyn HostDataPlane> = plane.clone();
    let app = RuntimeApp::open(RuntimeAppConfig {
        limits: AppLimits::default(),
        resource_limits: ResourceLimits::default(),
        grant_limits: GrantLimits::default(),
        bridge_limits: BridgeLimits::default(),
        binding_limits: BindingLimits::default(),
        store,
        data_plane,
        clock: Arc::new(TestClock::new(1_000)),
        permission_default: PermissionDefaultPreference::AskEveryTime,
        shell_provider,
        providers: vec![relay_provider],
    })
    .unwrap();
    let exact = principal('d');
    let relay = Capability::new("relay").unwrap();

    app.dispatch(PlatformCommand::InstallVerified {
        build: InstalledBuild {
            principal: exact.clone(),
            title: Arc::from("NMP write napplet"),
            manifest_metadata: json(serde_json::json!({"kind": 34128})),
            capability_requests: vec![request(relay.clone(), CapabilityRequirement::Required)],
        },
        artifact: Arc::new(TestArtifact {
            kind: 35_129,
            author: exact.manifest_author().to_owned(),
            d_tag: exact.d_tag().to_owned(),
            aggregate: exact.aggregate_hash().to_owned(),
        }),
    });
    app.dispatch(PlatformCommand::SetGrant {
        principal: exact.clone(),
        capability: relay.clone(),
        sensitivity: Sensitivity::Sensitive,
        decision: GrantDecision::AllowExactBuild,
    });
    app.dispatch(PlatformCommand::Launch {
        principal: exact.clone(),
        profile: ExecutionProfile::Legacy,
        required_domains: BTreeSet::from([relay.clone()]),
    });
    let session = app.snapshot().sessions.last().unwrap().id;
    app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ready(),
    });
    app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: mapped(serde_json::json!({
            "type": "relay.publish",
            "id": "signed-publish-1",
            "event": signed_note(),
        })),
    });
    assert_eq!(app.snapshot().pending_writes.len(), 1);
    let before_lifecycle = app.events_after(0).newest_available;

    match lifecycle {
        Lifecycle::Revoke => app.dispatch(PlatformCommand::Revoke {
            principal: exact,
            capability: relay,
        }),
        Lifecycle::Stop => app.dispatch(PlatformCommand::Stop { session }),
    }

    let responses = app
        .events_after(before_lifecycle)
        .events
        .into_iter()
        .filter_map(|item| match item.event {
            PlatformEvent::EnvelopeHandled {
                session: handled_session,
                operation: None,
                response: Some(response),
            } if handled_session == session => Some(response.decode().unwrap()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        responses,
        vec![serde_json::json!({
            "type": "relay.publish.result",
            "id": "signed-publish-1",
            "ok": false,
            "error": expected_error,
        })]
    );
    assert!(app.snapshot().pending_writes.is_empty());
    plane.close();
}

fn signed_note() -> serde_json::Value {
    serde_json::json!({
        "kind": 1,
        "id": "134ce22e517d5c5cd574fe276e52cf713d7ca1228da7530cef10c58286c03025",
        "pubkey": "974ab003f85a1c8d6da5ed68f215a4a7b5d1c8b5382013a93fd301abf97a68d4",
        "created_at": 1_700_000_000_u64,
        "tags": [],
        "content": "deterministic public note",
        "sig": "ef6e98957a40ae0eba44499a34b30cec9dcf0c4e933de9a753e9b0b7e48eccfce56edc5b7704380dc4d2618876756a198f1acbf3e273a301881d1b702e19e15d",
    })
}
