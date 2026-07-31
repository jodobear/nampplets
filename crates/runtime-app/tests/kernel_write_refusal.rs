//! Pending provider-write refusal delivery across permission revocation.

mod support;

use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    BridgeError, BridgeLimits, Provider, ProviderCall, ProviderDescriptor, ProviderError,
    ProviderPlatformAvailability, ProviderRequest, ProviderWriteCompletion, ProviderWriteRefusal,
};
use nmp_native_runtime_app::{AppErrorCode, PlatformCommand, PlatformEvent, RuntimeApp};
use nmp_native_runtime_core::{
    AccountRef, ApprovedWrite, BoundedJson, CapabilityRequirement, ExecutionProfile, GrantDecision,
    Principal, ReceiptEventSink, ReceiptSinkError, Sensitivity, SessionId,
};
use nmp_native_runtime_store::{InstalledBuild, RuntimeStore, StoreLimits};
use nmp_native_test_harness::FakeHostDataPlane;
use support::*;
use tempfile::TempDir;

#[derive(Debug)]
struct TerminalRefusalProvider {
    descriptor: ProviderDescriptor,
    response_padding: usize,
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
            response_padding: 0,
        }
    }

    fn with_response_padding(mut self, response_padding: usize) -> Self {
        self.response_padding = response_padding;
        self
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
            Box::new(TerminalRefusalCompletion {
                response_padding: self.response_padding,
            }),
            request.work,
        ))
    }
}

#[derive(Debug)]
struct TerminalRefusalCompletion {
    response_padding: usize,
}

impl ProviderWriteCompletion for TerminalRefusalCompletion {
    fn into_receipt_sink(self: Box<Self>) -> Arc<dyn ReceiptEventSink> {
        Arc::new(NoopReceiptSink)
    }

    fn refused(self: Box<Self>, refusal: ProviderWriteRefusal) -> Option<BoundedJson> {
        let ProviderWriteRefusal::SystemUnavailable(reason) = refusal else {
            return None;
        };
        let mut response = serde_json::json!({
            "type": "canary.ping.result",
            "id": "request-1",
            "ok": false,
            "error": "provider-unavailable",
            "reason": reason,
        });
        if self.response_padding > 0 {
            response["padding"] = serde_json::Value::String("x".repeat(self.response_padding));
        }
        BoundedJson::from_value(&response, 4 * 1024).ok()
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
    let (_directory, app, exact, session) = pending_write(0, BridgeLimits::default());
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

#[test]
fn host_owned_refusal_reuses_the_bridge_response_limit() {
    let limits = BridgeLimits {
        maximum_response_bytes: 1_024,
        ..BridgeLimits::default()
    };
    let (_directory, app, exact, session) = pending_write(2_048, limits);
    let before_revoke = app.events_after(0).newest_available;

    app.dispatch(PlatformCommand::Revoke {
        principal: exact.clone(),
        capability: canary(),
    });

    let events = app.events_after(before_revoke).events;
    assert!(!events.iter().any(|item| matches!(
        item.event,
        PlatformEvent::EnvelopeHandled {
            operation: None,
            response: Some(_),
            ..
        }
    )));
    assert!(events.iter().any(|item| matches!(
        &item.event,
        PlatformEvent::Refused(fact)
            if fact.code == AppErrorCode::Bridge
                && fact.principal.as_ref() == Some(&exact)
                && fact.session == Some(session)
                && fact.detail.as_ref() == BridgeError::ResponseTooLarge.to_string()
    )));
    assert!(app.snapshot().pending_writes.is_empty());
}

fn pending_write(
    response_padding: usize,
    bridge_limits: BridgeLimits,
) -> (TempDir, Arc<RuntimeApp>, Principal, SessionId) {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(
        RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default()).unwrap(),
    );
    let host = Arc::new(FakeHostDataPlane::new(16));
    let provider: Arc<dyn Provider> =
        Arc::new(TerminalRefusalProvider::new().with_response_padding(response_padding));
    let (app, _) =
        open_app_with_provider_and_bridge_limits(Arc::clone(&store), host, provider, bridge_limits);
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
    (directory, app, exact, session)
}
