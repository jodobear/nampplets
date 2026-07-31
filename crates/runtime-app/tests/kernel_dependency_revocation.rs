//! A dependency revoke cancels writes already proposed by the dependent provider.

mod support;

use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    BridgeLimits, Provider, ProviderCall, ProviderDescriptor, ProviderError,
    ProviderPlatformAvailability, ProviderRequest, ProviderWriteCompletion, ProviderWriteRefusal,
};
use nmp_native_providers::{ShellProvider, ShellProviderLimits};
use nmp_native_runtime_app::{
    AppLimits, PlatformCommand, PlatformEvent, RuntimeApp, RuntimeAppConfig,
};
use nmp_native_runtime_core::{
    AccountRef, ApprovedWrite, BoundedJson, Capability, CapabilityRequirement, GrantDecision,
    GrantLimits, HostDataPlane, ReceiptEventSink, ReceiptSinkError, ResourceLimits, Sensitivity,
};
use nmp_native_runtime_store::{
    InstalledBuild, PermissionDefaultPreference, RuntimeStore, StoreLimits,
};
use nmp_native_surface::BindingLimits;
use nmp_native_test_harness::FakeHostDataPlane;
use support::*;
use tempfile::TempDir;

#[derive(Debug)]
struct StaticProvider {
    descriptor: ProviderDescriptor,
    proposes_write: bool,
}

impl StaticProvider {
    fn dependency() -> Arc<dyn Provider> {
        Arc::new(Self {
            descriptor: descriptor(identity(), ["get"], []),
            proposes_write: false,
        })
    }

    fn dependent() -> Arc<dyn Provider> {
        Arc::new(Self {
            descriptor: descriptor(canary(), ["ping"], [identity()]),
            proposes_write: true,
        })
    }
}

impl Provider for StaticProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        if !self.proposes_write {
            return Ok(ProviderCall::completed(None));
        }
        Ok(ProviderCall::proposed_write(
            None,
            ApprovedWrite {
                approval_id: Arc::from("dependent-write"),
                origin_principal: request.principal,
                origin_session: request.session,
                account: AccountRef(Arc::from("f".repeat(64))),
                draft: BoundedJson::from_raw("{}", 16).unwrap(),
            },
            Box::new(RefusalCompletion),
            request.work,
        ))
    }
}

fn descriptor<const A: usize, const D: usize>(
    domain: Capability,
    actions: [&str; A],
    dependencies: [Capability; D],
) -> ProviderDescriptor {
    ProviderDescriptor {
        domain,
        protocol_versions: BTreeSet::from([Arc::from("test/1")]),
        actions: actions.into_iter().map(Arc::from).collect(),
        sensitive: true,
        dependencies: dependencies.into_iter().collect(),
        platform_availability: ProviderPlatformAvailability::Available,
    }
}

#[derive(Debug)]
struct RefusalCompletion;

impl ProviderWriteCompletion for RefusalCompletion {
    fn into_receipt_sink(self: Box<Self>) -> Arc<dyn ReceiptEventSink> {
        Arc::new(NoopReceiptSink)
    }

    fn refused(self: Box<Self>, refusal: ProviderWriteRefusal) -> Option<BoundedJson> {
        let ProviderWriteRefusal::SystemUnavailable(reason) = refusal else {
            return None;
        };
        BoundedJson::from_value(
            &serde_json::json!({
                "type": "canary.ping.result",
                "id": "request-1",
                "ok": false,
                "reason": reason,
            }),
            1024,
        )
        .ok()
    }
}

#[derive(Debug)]
struct NoopReceiptSink;

impl ReceiptEventSink for NoopReceiptSink {
    fn push_latest(
        &self,
        _snapshot: nmp_native_runtime_core::ReceiptSnapshot,
    ) -> Result<(), ReceiptSinkError> {
        Ok(())
    }

    fn close(&self, _reason: Option<Arc<str>>) {}
}

#[test]
fn dependency_revoke_cancels_pending_write_before_approval() {
    for batch in [false, true] {
        assert_dependency_revoke_cancels_write(batch);
    }
}

fn assert_dependency_revoke_cancels_write(batch: bool) {
    let (_directory, app, host, exact) = pending_write();
    let pending = app.snapshot().pending_writes[0].operation;
    let before = app.events_after(0).newest_available;

    if batch {
        app.dispatch(PlatformCommand::ApplyPermissionChanges(permission_changes(
            &app,
            exact.clone(),
            vec![
                permission(identity(), GrantDecision::AskEveryTime),
                permission(canary(), GrantDecision::AskEveryTime),
            ],
        )));
    } else {
        app.dispatch(PlatformCommand::Revoke {
            principal: exact.clone(),
            capability: identity(),
        });
    }

    assert!(
        app.snapshot().pending_writes.is_empty(),
        "pending dependent write survived batch={batch}"
    );
    assert_eq!(host.receipt_count(), 0);
    let events = app.events_after(before).events;
    let refusal = events
        .iter()
        .position(|event| {
            matches!(
                &event.event,
                PlatformEvent::EnvelopeHandled { response: Some(response), .. }
                    if response.decode().unwrap()["reason"] == "permission revoked"
            )
        })
        .expect("dependent pending write receives a terminal refusal");
    let revoked = events
        .iter()
        .position(|event| {
            if batch {
                matches!(event.event, PlatformEvent::PermissionChangesApplied { .. })
            } else {
                matches!(event.event, PlatformEvent::GrantChanged { .. })
            }
        })
        .expect("revocation remains observable");
    assert!(refusal < revoked);

    app.dispatch(PlatformCommand::DecideProviderWrite {
        operation: pending,
        approve: true,
    });
    assert_eq!(host.receipt_count(), 0);
}

fn pending_write() -> (
    TempDir,
    Arc<RuntimeApp>,
    Arc<FakeHostDataPlane>,
    nmp_native_runtime_core::Principal,
) {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(
        RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default()).unwrap(),
    );
    let host = Arc::new(FakeHostDataPlane::new(8));
    let data_plane: Arc<dyn HostDataPlane> = host.clone();
    let shell = Arc::new(
        ShellProvider::new(
            Arc::new(FixedShellEnvironment {
                override_domains: None,
            }),
            ShellProviderLimits::default(),
        )
        .unwrap(),
    );
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
        shell_provider: shell,
        providers: vec![StaticProvider::dependency(), StaticProvider::dependent()],
    })
    .unwrap();
    let exact = principal('d');
    app.dispatch(PlatformCommand::InstallVerified {
        build: InstalledBuild {
            principal: exact.clone(),
            title: Arc::from("Dependent writer"),
            manifest_metadata: json(serde_json::json!({"kind": 35129})),
            capability_requests: vec![
                request(canary(), CapabilityRequirement::Required),
                request(identity(), CapabilityRequirement::Required),
            ],
        },
        artifact: Arc::new(TestArtifact {
            kind: 35_129,
            author: exact.manifest_author().to_owned(),
            d_tag: exact.d_tag().to_owned(),
            aggregate: exact.aggregate_hash().to_owned(),
        }),
    });
    for capability in [identity(), canary()] {
        app.dispatch(PlatformCommand::SetGrant {
            principal: exact.clone(),
            capability,
            sensitivity: Sensitivity::Sensitive,
            decision: GrantDecision::AllowExactBuild,
        });
    }
    app.dispatch(PlatformCommand::Launch {
        principal: exact.clone(),
        profile: nmp_native_runtime_core::ExecutionProfile::Legacy,
        required_domains: BTreeSet::from([canary()]),
    });
    let session = app.snapshot().sessions[0].id;
    app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ready(),
    });
    app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ping(serde_json::json!({})),
    });
    assert_eq!(app.snapshot().pending_writes.len(), 1);
    (directory, app, host, exact)
}

fn identity() -> Capability {
    Capability::new("identity").unwrap()
}
