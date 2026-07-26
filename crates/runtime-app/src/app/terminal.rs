//! One-shot terminal closure for an exhausted snapshot identity.

use std::sync::Arc;

use nmp_native_nap_bridge::ProviderSessionEnd;

use super::{AppState, RuntimeApp};
use crate::views::{AppSnapshot, AppTerminalReason, SnapshotSection};

impl RuntimeApp {
    pub(super) fn preflight_revision_capacity(&self, state: &mut AppState) -> bool {
        if state.terminal_reason.is_some() {
            return false;
        }
        let snapshot = Arc::clone(&self.snapshots.borrow());
        let revisions = snapshot.revisions;
        let exhausted = [
            (state.revision, SnapshotSection::FusedSnapshot),
            (revisions.library, SnapshotSection::Library),
            (revisions.sessions, SnapshotSection::Sessions),
            (
                revisions.provider_push_lanes,
                SnapshotSection::ProviderPushLanes,
            ),
            (revisions.bindings, SnapshotSection::Bindings),
            (revisions.pending_writes, SnapshotSection::PendingWrites),
            (revisions.receipts, SnapshotSection::Receipts),
            (revisions.workspaces, SnapshotSection::Workspaces),
            (revisions.resources, SnapshotSection::Resources),
            (state.activity.appended(), SnapshotSection::Activity),
            (state.errors.appended(), SnapshotSection::Errors),
            (
                state.next_event_sequence,
                SnapshotSection::NewestEventSequence,
            ),
        ]
        .into_iter()
        .find_map(|(revision, section)| (revision == u64::MAX).then_some(section));
        if let Some(section) = exhausted {
            self.enter_revision_terminal_from(state, &snapshot, section);
            return false;
        }
        true
    }

    pub(super) fn enter_revision_terminal(&self, state: &mut AppState, section: SnapshotSection) {
        let previous = Arc::clone(&self.snapshots.borrow());
        self.enter_revision_terminal_from(state, &previous, section);
    }

    pub(super) fn enter_revision_terminal_from(
        &self,
        state: &mut AppState,
        previous: &AppSnapshot,
        section: SnapshotSection,
    ) {
        if state.terminal_reason.is_some() {
            return;
        }
        let reason = AppTerminalReason::SectionRevisionExhausted { section };
        state.closed = true;
        state.terminal_reason = Some(reason.clone());

        for (_, operation) in std::mem::take(&mut state.operations) {
            operation.cancel(Arc::from("runtime section revision exhausted"));
        }
        for (session_id, mut entry) in std::mem::take(&mut state.sessions) {
            self.shell_provider.close_session(session_id);
            self.bridge
                .close_session_with_reason(session_id, ProviderSessionEnd::RuntimeClosed);
            entry.session.stop();
            entry.push_observer.take();
            entry.push_delivery.take();
        }
        for (_, owner) in std::mem::take(&mut state.bindings) {
            owner.binding.close();
        }
        for (_, receipt) in std::mem::take(&mut state.receipts) {
            receipt.stop_delivery();
        }
        state.artifacts.clear();

        let mut terminal = previous.clone();
        if let Some(next) = state.revision.checked_add(1) {
            state.revision = next;
            terminal.revision = next;
        }
        terminal.closed = true;
        terminal.terminal_reason = Some(reason);
        self.snapshots.send_replace(Arc::new(terminal));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::Arc,
        sync::atomic::{AtomicU64, Ordering},
    };

    use nmp_native_nap_bridge::{BridgeLimits, ProviderPushError, SourceWindowId};
    use nmp_native_providers::{
        ShellEnvironment, ShellEnvironmentError, ShellEnvironmentLimits, ShellEnvironmentSource,
        ShellProvider, ShellProviderLimits,
    };
    use nmp_native_runtime_core::{Capability, GrantLimits, Principal, ResourceLimits, SessionId};
    use nmp_native_runtime_store::{PermissionDefaultPreference, RuntimeStore, StoreLimits};
    use nmp_native_surface::BindingLimits;
    use nmp_native_test_harness::FakeHostDataPlane;
    use tempfile::TempDir;

    use super::*;
    use crate::{AppLimits, KernelClock, PlatformCommand, RuntimeAppConfig};

    #[derive(Debug)]
    struct TestClock(AtomicU64);

    impl KernelClock for TestClock {
        fn now_millis(&self) -> u64 {
            self.0.fetch_add(1, Ordering::AcqRel)
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

    fn app() -> (TempDir, Arc<RuntimeApp>) {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(
            RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default())
                .unwrap(),
        );
        let shell_provider = Arc::new(
            ShellProvider::new(
                Arc::new(EmptyShellEnvironment),
                ShellProviderLimits::default(),
            )
            .unwrap(),
        );
        let data_plane: Arc<dyn nmp_native_runtime_core::HostDataPlane> =
            Arc::new(FakeHostDataPlane::new(8));
        let app = RuntimeApp::open(RuntimeAppConfig {
            limits: AppLimits::default(),
            resource_limits: ResourceLimits::default(),
            grant_limits: GrantLimits::default(),
            bridge_limits: BridgeLimits::default(),
            binding_limits: BindingLimits::default(),
            store,
            data_plane,
            clock: Arc::new(TestClock(AtomicU64::new(1))),
            permission_default: PermissionDefaultPreference::AskEveryTime,
            shell_provider,
            providers: Vec::new(),
        })
        .unwrap();
        (directory, app)
    }

    fn seed_exhausted(app: &RuntimeApp, section: SnapshotSection) {
        let mut snapshot = (*app.snapshot()).clone();
        let mut state = app.state.lock();
        match section {
            SnapshotSection::Workspaces => snapshot.revisions.workspaces = u64::MAX,
            SnapshotSection::Activity => {
                snapshot.revisions.activity = u64::MAX;
                state.activity.seed_appended(u64::MAX);
            }
            SnapshotSection::Errors => {
                snapshot.revisions.errors = u64::MAX;
                state.errors.seed_appended(u64::MAX);
            }
            SnapshotSection::NewestEventSequence => {
                snapshot.revisions.newest_event_sequence = u64::MAX;
                state.next_event_sequence = u64::MAX;
            }
            _ => unreachable!(),
        }
        drop(state);
        app.snapshots.send_replace(Arc::new(snapshot));
    }

    #[test]
    fn every_identity_family_closes_before_mutation_and_later_inputs_are_noops() {
        for section in [
            SnapshotSection::Workspaces,
            SnapshotSection::Activity,
            SnapshotSection::Errors,
            SnapshotSection::NewestEventSequence,
        ] {
            let (_directory, app) = app();
            seed_exhausted(&app, section);
            let last_truthful = app.snapshot();

            app.dispatch(PlatformCommand::SetLibraryFilter {
                query: Arc::from("must-not-apply"),
            });
            let terminal = app.snapshot();
            assert!(terminal.closed);
            assert_eq!(
                terminal.terminal_reason,
                Some(AppTerminalReason::SectionRevisionExhausted { section })
            );
            assert_eq!(terminal.revisions, last_truthful.revisions);
            assert_eq!(terminal.library, last_truthful.library);

            let terminal_revision = terminal.revision;
            app.dispatch(PlatformCommand::SetLibraryFilter {
                query: Arc::from("still-must-not-apply"),
            });
            app.provider_push_observation_failed(
                SessionId(999),
                SourceWindowId(999),
                ProviderPushError::Closed,
            );
            let after_callbacks = app.snapshot();
            assert_eq!(after_callbacks.revision, terminal_revision);
            assert_eq!(after_callbacks, terminal);
        }
    }

    #[test]
    fn structural_advance_at_max_returns_the_exact_section() {
        let (_directory, app) = app();
        let mut previous = (*app.snapshot()).clone();
        previous.revisions.workspaces = u64::MAX;
        let mut current = previous.clone();
        current.workspaces.push(crate::views::WorkspaceView {
            id: Arc::from("changed"),
            definition: nmp_native_runtime_core::BoundedJson::from_value(
                &serde_json::json!({}),
                32,
            )
            .unwrap(),
            retained_receipts: Vec::new(),
            assigned_builds: Vec::new(),
        });
        let state = app.state.lock();
        assert_eq!(
            super::super::revisions::advance_revisions(&previous, &current, &state),
            Err(SnapshotSection::Workspaces)
        );
    }
}
