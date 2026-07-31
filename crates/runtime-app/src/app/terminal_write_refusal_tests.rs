//! Regression coverage for terminal provider-write delivery ordering.

use std::{collections::BTreeSet, sync::Arc};

use nmp_native_nap_bridge::{
    BridgeLimits, ProviderCall, ProviderWriteCompletion, ProviderWriteRefusal,
};
use nmp_native_providers::{
    ShellEnvironment, ShellEnvironmentError, ShellEnvironmentLimits, ShellEnvironmentSource,
    ShellProvider, ShellProviderLimits,
};
use nmp_native_runtime_core::{
    AccountRef, ApprovedWrite, BoundedJson, Capability, GrantLimits, Principal, ReceiptEventSink,
    ReceiptSinkError, ResourceClass, ResourceLimits, SessionId,
};
use nmp_native_runtime_store::{PermissionDefaultPreference, RuntimeStore, StoreLimits};
use nmp_native_surface::BindingLimits;
use nmp_native_test_harness::FakeHostDataPlane;
use tempfile::TempDir;

use super::{ActiveOperation, RuntimeApp};
use crate::{
    AppLimits, KernelClock, PlatformCommand, PlatformEvent, ProviderOperationId, RuntimeAppConfig,
    SnapshotSection,
};

#[derive(Debug)]
struct TestClock;

impl KernelClock for TestClock {
    fn now_millis(&self) -> u64 {
        1
    }
}

#[derive(Debug)]
struct EmptyShellEnvironment;

impl ShellEnvironmentSource for EmptyShellEnvironment {
    fn environment(
        &self,
        _principal: &Principal,
        _session: SessionId,
        offered_domains: &BTreeSet<Capability>,
    ) -> Result<ShellEnvironment, ShellEnvironmentError> {
        ShellEnvironment::new(
            offered_domains.iter().cloned(),
            Vec::<Arc<str>>::new(),
            ShellEnvironmentLimits::default(),
        )
    }
}

#[derive(Debug)]
struct TerminalRefusal;

impl ProviderWriteCompletion for TerminalRefusal {
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
                "id": "terminal-request",
                "ok": false,
                "error": "provider-unavailable",
                "reason": reason,
            }),
            1_024,
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
fn revision_terminal_projects_pending_write_refusal_before_closing() {
    let directory = TempDir::new().unwrap();
    let store = Arc::new(
        RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default()).unwrap(),
    );
    let shell_provider = Arc::new(
        ShellProvider::new(
            Arc::new(EmptyShellEnvironment),
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
        data_plane: Arc::new(FakeHostDataPlane::new(8)),
        clock: Arc::new(TestClock),
        permission_default: PermissionDefaultPreference::AskEveryTime,
        shell_provider,
        providers: Vec::new(),
    })
    .unwrap();

    let principal = Principal::new("a".repeat(64), "terminal", "b".repeat(64)).unwrap();
    let session = SessionId(7);
    let work = app
        .resources
        .admit(session, None, ResourceClass::ProviderCall)
        .unwrap();
    let mut call = ProviderCall::proposed_write(
        None,
        ApprovedWrite {
            approval_id: Arc::from("terminal-write"),
            origin_principal: principal.clone(),
            origin_session: session,
            account: AccountRef(Arc::from("f".repeat(64))),
            draft: BoundedJson::from_raw("{}", 16).unwrap(),
        },
        Box::new(TerminalRefusal),
        work,
    );
    let operation = ActiveOperation {
        session,
        principal,
        domain: Capability::new("canary").unwrap(),
        handle: None,
        proposal: call.take_write_proposal(),
    };
    app.state
        .lock()
        .operations
        .insert(ProviderOperationId(1), operation);

    let before_terminal = app.events_after(0).newest_available;
    let mut exhausted = (*app.snapshot()).clone();
    exhausted.revisions.workspaces = u64::MAX;
    app.snapshots.send_replace(Arc::new(exhausted));

    app.dispatch(PlatformCommand::SetLibraryFilter {
        query: Arc::from("must-not-apply"),
    });

    let terminal = app.snapshot();
    assert!(terminal.closed);
    assert!(terminal.pending_writes.is_empty());
    assert!(app.events_after(before_terminal).events.iter().any(|item| {
        matches!(
            &item.event,
            PlatformEvent::EnvelopeHandled {
                session: response_session,
                operation: None,
                response: Some(response),
            } if *response_session == session
                && response.decode().unwrap()["id"] == "terminal-request"
                && response.decode().unwrap()["reason"]
                    == "runtime section revision exhausted"
        )
    }));
    assert_eq!(
        terminal.terminal_reason,
        Some(crate::AppTerminalReason::SectionRevisionExhausted {
            section: SnapshotSection::Workspaces,
        })
    );
}
