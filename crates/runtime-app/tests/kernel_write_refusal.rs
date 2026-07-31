//! Pending provider-write refusal delivery across permission revocation.

mod support;

use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderRequest, ProviderWriteCompletion, ProviderWriteRefusal,
};
use nmp_native_runtime_app::{PlatformCommand, PlatformEvent};
use nmp_native_runtime_core::{
    AccountRef, ApprovedWrite, BoundedJson, CapabilityRequirement, ExecutionProfile, GrantDecision,
    ReceiptEventSink, ReceiptSinkError, Sensitivity,
};
use nmp_native_runtime_store::{InstalledBuild, RuntimeStore, StoreLimits};
use nmp_native_test_harness::FakeHostDataPlane;
use support::*;
use tempfile::TempDir;

#[derive(Debug)]
struct TerminalRefusalProvider {
    descriptor: ProviderDescriptor,
}

impl TerminalRefusalProvider {
    fn new() -> Self {
        Self {
            descriptor: ProviderDescriptor {
                domain: canary(),
                protocol_versions: BTreeSet::from([Arc::from("internal-canary/1")]),
                actions: BTreeSet::from([Arc::from("ping")]),
                sensitive: false,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
        }
    }
}

impl Provider for TerminalRefusalProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let write = ApprovedWrite {
            approval_id: Arc::from("canary-write-1"),
            origin_principal: request.principal,
            origin_session: request.session,
            account: AccountRef(Arc::from("f".repeat(64))),
            draft: BoundedJson::from_raw("{}", 16).unwrap(),
        };
        Ok(ProviderCall::proposed_write(
            None,
            write,
            Box::new(TerminalRefusalCompletion),
            request.work,
        ))
    }
}

#[derive(Debug)]
struct TerminalRefusalCompletion;

impl ProviderWriteCompletion for TerminalRefusalCompletion {
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
                "error": "provider-unavailable",
                "reason": reason,
            }),
            4 * 1024,
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
fn revocation_projects_a_pending_write_refusal_before_the_provider_lane_closes() {
    assert_pending_write_refusal_from_revocation(false);
    assert_pending_write_refusal_from_revocation(true);
}

fn assert_pending_write_refusal_from_revocation(batch: bool) {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(
        RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default()).unwrap(),
    );
    let host = Arc::new(FakeHostDataPlane::new(16));
    let provider: Arc<dyn Provider> = Arc::new(TerminalRefusalProvider::new());
    let (app, _) = open_app_with_provider(Arc::clone(&store), host, provider);
    let exact = principal('b');

    app.dispatch(PlatformCommand::InstallVerified {
        build: InstalledBuild {
            principal: exact.clone(),
            title: Arc::from("Write napplet"),
            manifest_metadata: json(serde_json::json!({"kind": 34128})),
            capability_requests: vec![request(canary(), CapabilityRequirement::Required)],
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
        capability: canary(),
        sensitivity: Sensitivity::Ordinary,
        decision: GrantDecision::AllowExactBuild,
    });
    app.dispatch(PlatformCommand::Launch {
        principal: exact.clone(),
        profile: ExecutionProfile::Legacy,
        required_domains: BTreeSet::from([canary()]),
    });
    let session = app.snapshot().sessions.last().unwrap().id;
    app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ready(),
    });
    app.dispatch(PlatformCommand::MappedEnvelope {
        session,
        bytes: ping(serde_json::json!({})),
    });
    assert_eq!(app.snapshot().pending_writes.len(), 1);
    let before_revoke = app.events_after(0).newest_available;

    if batch {
        app.dispatch(PlatformCommand::ApplyPermissionChanges(permission_changes(
            &app,
            exact,
            vec![permission(canary(), GrantDecision::AskEveryTime)],
        )));
    } else {
        app.dispatch(PlatformCommand::Revoke {
            principal: exact,
            capability: canary(),
        });
    }

    let events = app.events_after(before_revoke).events;
    let response_position = events
        .iter()
        .position(|item| {
            matches!(
                &item.event,
                PlatformEvent::EnvelopeHandled {
                    session: handled_session,
                    operation: None,
                    response: Some(response),
                } if *handled_session == session
                    && response.decode().unwrap()["error"] == "provider-unavailable"
            )
        })
        .expect("system refusal must be projected as host-owned terminal output");
    let revocation_position = events
        .iter()
        .position(|item| {
            if batch {
                matches!(item.event, PlatformEvent::PermissionChangesApplied { .. })
            } else {
                matches!(item.event, PlatformEvent::GrantChanged { .. })
            }
        })
        .expect("revocation must remain observable");
    assert!(
        response_position < revocation_position,
        "terminal output must be observable before the revocation event"
    );
    let PlatformEvent::EnvelopeHandled {
        response: Some(response),
        ..
    } = &events[response_position].event
    else {
        unreachable!();
    };
    assert_eq!(
        response.decode().unwrap(),
        serde_json::json!({
            "type": "canary.ping.result",
            "id": "request-1",
            "ok": false,
            "error": "provider-unavailable",
            "reason": "permission revoked",
        })
    );
    assert!(app.snapshot().pending_writes.is_empty());
}
