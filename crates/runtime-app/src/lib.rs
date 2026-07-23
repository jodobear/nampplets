//! Rust-owned application composition root for the native napplet runtime.
//!
//! The kernel is the single writer for product policy and lifecycle. Platform
//! shells submit commands and render bounded snapshots/events. NMP remains the
//! sole owner of canonical Nostr state and durable write obligations behind
//! [`HostDataPlane`].

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use nmp_native_artifact::VerifiedArtifactHandle;
use nmp_native_nap_bridge::{
    ActivitySink, BridgeError, BridgeLimits, DispatchOutcome, InjectionPlan, Provider,
    ProviderActivity, ProviderOperation, ProviderRegistry, SessionContext,
};
use nmp_native_providers::ShellProvider;
use nmp_native_runtime_core::{
    ApprovedWrite, BindingRequest, BoundedJson, Capability, ExecutionProfile, GrantDecision,
    GrantError, GrantLedger, GrantLimits, HostDataPlane, Principal, ReceiptEventSink,
    ReceiptObservation, ReceiptReattachment, ReceiptSinkError, ReceiptSnapshot, ResourceCensus,
    ResourceClass, ResourceLimits, ResourceRefusal, ResourceTracker, Sensitivity, Session,
    SessionError, SessionId, SessionSnapshot, SessionState, WorkLease, WriteReceiptId,
};
use nmp_native_runtime_store::{
    ActivityRecord, InstalledBuild, RuntimeStore, StoreError, WorkspaceRecord,
};
use nmp_native_surface::{Binding, BindingError, BindingLimits};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::watch;

/// Kernel-owned limits. Every collection crossing the platform boundary is
/// bounded by one of these values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppLimits {
    pub maximum_installed_artifacts: usize,
    pub maximum_sessions: usize,
    pub maximum_bindings: usize,
    pub maximum_receipts: usize,
    pub maximum_provider_operations: usize,
    pub maximum_activity_facts: usize,
    pub maximum_error_facts: usize,
    pub maximum_platform_events: usize,
    pub maximum_receipt_frame_bytes: usize,
    pub maximum_envelope_bytes: usize,
}

impl Default for AppLimits {
    fn default() -> Self {
        Self {
            maximum_installed_artifacts: 512,
            maximum_sessions: 16,
            maximum_bindings: 64,
            maximum_receipts: 256,
            maximum_provider_operations: 128,
            maximum_activity_facts: 1_024,
            maximum_error_facts: 256,
            maximum_platform_events: 1_024,
            maximum_receipt_frame_bytes: 256 * 1024,
            maximum_envelope_bytes: 256 * 1024,
        }
    }
}

impl AppLimits {
    fn validate(self) -> Result<Self, OpenError> {
        if [
            self.maximum_installed_artifacts,
            self.maximum_sessions,
            self.maximum_bindings,
            self.maximum_receipts,
            self.maximum_provider_operations,
            self.maximum_activity_facts,
            self.maximum_error_facts,
            self.maximum_platform_events,
            self.maximum_receipt_frame_bytes,
            self.maximum_envelope_bytes,
        ]
        .contains(&0)
        {
            return Err(OpenError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Time is an explicit nondeterministic input owned by the Rust kernel.
pub trait KernelClock: Send + Sync + fmt::Debug {
    fn now_millis(&self) -> u64;
}

/// Adaptable immutable executable handle implemented by the trusted Rust
/// artifact-resolution boundary. Platform and untrusted component code never
/// construct implementations of this interface.
pub trait ExecutableArtifact: Send + Sync + fmt::Debug {
    fn manifest_kind(&self) -> u16;
    fn manifest_author(&self) -> &str;
    fn d_tag(&self) -> Option<&str>;
    fn aggregate_hash(&self) -> &str;
    fn contains_logical_path(&self, logical_path: &str) -> bool;
}

impl ExecutableArtifact for VerifiedArtifactHandle {
    fn manifest_kind(&self) -> u16 {
        self.index().kind()
    }

    fn manifest_author(&self) -> &str {
        self.index().author().as_str()
    }

    fn d_tag(&self) -> Option<&str> {
        self.index().d_tag()
    }

    fn aggregate_hash(&self) -> &str {
        self.index().aggregate().as_str()
    }

    fn contains_logical_path(&self, logical_path: &str) -> bool {
        self.index()
            .entries()
            .any(|entry| entry.path() == logical_path)
    }
}

#[derive(Debug)]
pub struct RuntimeAppConfig {
    pub limits: AppLimits,
    pub resource_limits: ResourceLimits,
    pub grant_limits: GrantLimits,
    pub bridge_limits: BridgeLimits,
    pub binding_limits: BindingLimits,
    pub store: Arc<RuntimeStore>,
    pub data_plane: Arc<dyn HostDataPlane>,
    pub clock: Arc<dyn KernelClock>,
    /// The mandatory NAP-SHELL provider. It is registered exactly once by the
    /// kernel and retained as the session-establishment authority.
    pub shell_provider: Arc<ShellProvider>,
    /// Fully conformant non-shell providers only.
    pub providers: Vec<Arc<dyn Provider>>,
}

/// Commands are semantic platform inputs. No mapped-message command accepts a
/// principal, profile, grant, or account chosen by untrusted content.
#[derive(Debug)]
pub enum PlatformCommand {
    InstallVerified {
        build: InstalledBuild,
        artifact: Arc<dyn ExecutableArtifact>,
    },
    SetGrant {
        principal: Principal,
        capability: Capability,
        sensitivity: Sensitivity,
        decision: GrantDecision,
    },
    Revoke {
        principal: Principal,
        capability: Capability,
    },
    Launch {
        principal: Principal,
        profile: ExecutionProfile,
        required_domains: BTreeSet<Capability>,
    },
    Stop {
        session: SessionId,
    },
    Crash {
        session: SessionId,
        reason: Arc<str>,
    },
    MappedEnvelope {
        session: SessionId,
        bytes: Arc<[u8]>,
    },
    CompleteProviderOperation {
        operation: ProviderOperationId,
    },
    OpenBinding {
        request: BindingRequest,
    },
    CloseBinding {
        binding_id: Arc<str>,
    },
    ApproveWrite {
        write: ApprovedWrite,
    },
    SaveWorkspace {
        workspace: WorkspaceRecord,
    },
    RestoreWorkspaces,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderOperationId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformEvent {
    Installed {
        principal: Principal,
    },
    GrantChanged {
        principal: Principal,
        capability: Capability,
        decision: GrantDecision,
    },
    SessionChanged(SessionSnapshot),
    EnvelopeHandled {
        session: SessionId,
        operation: Option<ProviderOperationId>,
        response: Option<BoundedJson>,
    },
    EnvelopeIgnored {
        session: SessionId,
    },
    ProviderOperationFinished {
        operation: ProviderOperationId,
    },
    BindingOpened {
        binding_id: Arc<str>,
        logical_source_id: Arc<str>,
    },
    BindingClosed {
        binding_id: Arc<str>,
    },
    WriteAccepted {
        receipt_id: WriteReceiptId,
        frozen_account: nmp_native_runtime_core::AccountRef,
    },
    WorkspaceSaved {
        workspace_id: Arc<str>,
    },
    WorkspaceRestored {
        workspace_id: Arc<str>,
    },
    ReceiptReattached {
        receipt_id: WriteReceiptId,
    },
    ReceiptNotFound {
        receipt_id: WriteReceiptId,
    },
    Refused(AppErrorFact),
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppErrorFact {
    pub code: AppErrorCode,
    pub principal: Option<Principal>,
    pub session: Option<SessionId>,
    pub detail: Arc<str>,
    pub occurred_at_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AppErrorCode {
    Capacity,
    NotInstalled,
    UnsupportedManifestIdentity,
    ArtifactIdentityMismatch,
    MissingIndex,
    UnknownSession,
    SessionIdentityMismatch,
    InvalidLifecycle,
    Grant,
    Bridge,
    Binding,
    HostData,
    Store,
    Receipt,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityFact {
    pub principal: Principal,
    pub category: Arc<str>,
    pub operation: Arc<str>,
    pub outcome: Arc<str>,
    pub occurred_at_millis: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingView {
    pub id: Arc<str>,
    pub schema: Arc<str>,
    pub logical_source_id: Option<Arc<str>>,
    pub revision: Option<u64>,
}

/// Fixed provider domains injected for one mapped session. This is the same
/// immutable negotiation plan used to build `shell.init`; native platforms
/// must not infer or widen it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionDomainView {
    pub session: SessionId,
    pub domains: Vec<Capability>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptDeliveryState {
    Observing,
    NotFound,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptView {
    pub receipt_id: WriteReceiptId,
    pub delivery: ReceiptDeliveryState,
    pub latest: Option<ReceiptSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceView {
    pub id: Arc<str>,
    pub definition: BoundedJson,
    pub retained_receipts: Vec<WriteReceiptId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppSnapshot {
    pub revision: u64,
    pub closed: bool,
    pub sessions: Vec<SessionSnapshot>,
    pub session_domains: Vec<SessionDomainView>,
    pub bindings: Vec<BindingView>,
    pub receipts: Vec<ReceiptView>,
    pub workspaces: Vec<WorkspaceView>,
    pub resources: ResourceCensus,
    pub recent_activity: Vec<ActivityFact>,
    pub recent_errors: Vec<AppErrorFact>,
}

#[derive(Debug)]
pub struct AppObserver {
    receiver: watch::Receiver<Arc<AppSnapshot>>,
}

impl AppObserver {
    pub fn latest(&self) -> Arc<AppSnapshot> {
        Arc::clone(&self.receiver.borrow())
    }

    pub async fn changed(&mut self) -> Result<Arc<AppSnapshot>, ObservationClosed> {
        self.receiver
            .changed()
            .await
            .map_err(|_| ObservationClosed)?;
        Ok(self.latest())
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("application observation is closed")]
pub struct ObservationClosed;

#[derive(Debug)]
pub struct RuntimeApp {
    limits: AppLimits,
    binding_limits: BindingLimits,
    resources: Arc<ResourceTracker>,
    grants: Arc<GrantLedger>,
    bridge: ProviderRegistry,
    shell_provider: Arc<ShellProvider>,
    mapped_routes: BTreeSet<(Capability, Arc<str>)>,
    store: Arc<RuntimeStore>,
    data_plane: Arc<dyn HostDataPlane>,
    clock: Arc<dyn KernelClock>,
    state: Mutex<AppState>,
    snapshots: watch::Sender<Arc<AppSnapshot>>,
}

#[derive(Debug)]
struct AppState {
    next_session_id: u64,
    next_operation_id: u64,
    revision: u64,
    closed: bool,
    artifacts: BTreeMap<Principal, Arc<dyn ExecutableArtifact>>,
    sessions: BTreeMap<SessionId, SessionEntry>,
    operations: BTreeMap<ProviderOperationId, ActiveOperation>,
    bindings: BTreeMap<Arc<str>, BindingOwner>,
    receipts: BTreeMap<WriteReceiptId, Arc<AppReceipt>>,
    workspaces: BTreeMap<Arc<str>, WorkspaceRecord>,
    activity: VecDeque<ActivityFact>,
    errors: VecDeque<AppErrorFact>,
    events: VecDeque<SequencedPlatformEvent>,
}

#[derive(Debug)]
struct SessionEntry {
    session: Arc<Session>,
    context: SessionContext,
    plan: InjectionPlan,
    _artifact: Arc<dyn ExecutableArtifact>,
    _webview: WorkLease,
}

#[derive(Debug)]
struct ActiveOperation {
    session: SessionId,
    principal: Principal,
    domain: Capability,
    handle: ProviderOperation,
}

#[derive(Debug)]
struct BindingOwner {
    request: BindingRequest,
    binding: Arc<Binding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequencedPlatformEvent {
    pub sequence: u64,
    pub event: PlatformEvent,
}

#[derive(Debug)]
pub struct EventBatch {
    pub oldest_available: u64,
    pub newest_available: u64,
    pub events: Vec<SequencedPlatformEvent>,
    pub cursor_was_stale: bool,
}

#[derive(Debug, Error)]
pub enum OpenError {
    #[error("application limits must all be finite and non-zero")]
    InvalidLimits,
    #[error(transparent)]
    Resource(#[from] ResourceRefusal),
    #[error(transparent)]
    Grant(#[from] GrantError),
    #[error(transparent)]
    Bridge(#[from] BridgeError),
}

impl RuntimeApp {
    pub fn open(config: RuntimeAppConfig) -> Result<Arc<Self>, OpenError> {
        let limits = config.limits.validate()?;
        let resources = Arc::new(ResourceTracker::new(config.resource_limits)?);
        let grants = Arc::new(GrantLedger::new(
            config.grant_limits,
            Arc::clone(&resources),
        )?);
        let activity_sink: Arc<dyn ActivitySink> = Arc::new(NoopBridgeActivity);
        let mut bridge = ProviderRegistry::new(
            config.bridge_limits,
            Arc::clone(&resources),
            Arc::clone(&grants),
            activity_sink,
        )?;
        let shell_provider = config.shell_provider;
        let mut mapped_routes = shell_provider
            .descriptor()
            .actions
            .iter()
            .cloned()
            .map(|action| (shell_provider.descriptor().domain.clone(), action))
            .collect::<BTreeSet<_>>();
        let registered_shell: Arc<dyn Provider> = shell_provider.clone();
        bridge.register(registered_shell)?;
        for provider in config.providers {
            mapped_routes.extend(
                provider
                    .descriptor()
                    .actions
                    .iter()
                    .cloned()
                    .map(|action| (provider.descriptor().domain.clone(), action)),
            );
            bridge.register(provider)?;
        }
        let initial = Arc::new(AppSnapshot {
            revision: 0,
            closed: false,
            sessions: Vec::new(),
            session_domains: Vec::new(),
            bindings: Vec::new(),
            receipts: Vec::new(),
            workspaces: Vec::new(),
            resources: resources.census(),
            recent_activity: Vec::new(),
            recent_errors: Vec::new(),
        });
        let (snapshots, _) = watch::channel(initial);
        Ok(Arc::new(Self {
            limits,
            binding_limits: config.binding_limits,
            resources,
            grants,
            bridge,
            shell_provider,
            mapped_routes,
            store: config.store,
            data_plane: config.data_plane,
            clock: config.clock,
            state: Mutex::new(AppState {
                next_session_id: 0,
                next_operation_id: 0,
                revision: 0,
                closed: false,
                artifacts: BTreeMap::new(),
                sessions: BTreeMap::new(),
                operations: BTreeMap::new(),
                bindings: BTreeMap::new(),
                receipts: BTreeMap::new(),
                workspaces: BTreeMap::new(),
                activity: VecDeque::with_capacity(limits.maximum_activity_facts),
                errors: VecDeque::with_capacity(limits.maximum_error_facts),
                events: VecDeque::with_capacity(limits.maximum_platform_events),
            }),
            snapshots,
        }))
    }

    /// Fire-and-observe command boundary. Operation success and failure are
    /// projected through [`PlatformEvent`] and [`AppSnapshot`], never returned
    /// to the native renderer as product control flow.
    pub fn dispatch(&self, command: PlatformCommand) {
        let now = self.clock.now_millis();
        let mut state = self.state.lock();
        if state.closed && !matches!(command, PlatformCommand::Close) {
            self.refuse(
                &mut state,
                AppErrorCode::Closed,
                None,
                None,
                "runtime is closed",
                now,
            );
            self.publish(&mut state);
            return;
        }

        match command {
            PlatformCommand::InstallVerified { build, artifact } => {
                self.install_verified(&mut state, build, artifact, now);
            }
            PlatformCommand::SetGrant {
                principal,
                capability,
                sensitivity,
                decision,
            } => self.set_grant(
                &mut state,
                principal,
                capability,
                sensitivity,
                decision,
                now,
            ),
            PlatformCommand::Revoke {
                principal,
                capability,
            } => self.revoke(&mut state, principal, capability, now),
            PlatformCommand::Launch {
                principal,
                profile,
                required_domains,
            } => self.launch(&mut state, principal, profile, required_domains, now),
            PlatformCommand::Stop { session } => {
                self.end_session(&mut state, session, SessionState::Stopped, None, now);
            }
            PlatformCommand::Crash { session, reason } => {
                self.end_session(
                    &mut state,
                    session,
                    SessionState::Crashed,
                    Some(reason),
                    now,
                );
            }
            PlatformCommand::MappedEnvelope { session, bytes } => {
                self.dispatch_envelope(&mut state, session, &bytes, now);
            }
            PlatformCommand::CompleteProviderOperation { operation } => {
                self.complete_operation(&mut state, operation, now);
            }
            PlatformCommand::OpenBinding { request } => {
                self.open_binding(&mut state, request, now);
            }
            PlatformCommand::CloseBinding { binding_id } => {
                self.close_binding(&mut state, &binding_id, now);
            }
            PlatformCommand::ApproveWrite { write } => {
                self.approve_write(&mut state, write, now);
            }
            PlatformCommand::SaveWorkspace { workspace } => {
                self.save_workspace(&mut state, workspace, now);
            }
            PlatformCommand::RestoreWorkspaces => self.restore_workspaces(&mut state, now),
            PlatformCommand::Close => self.close(&mut state, now),
        }
        self.publish(&mut state);
    }

    pub fn observe(&self) -> AppObserver {
        AppObserver {
            receiver: self.snapshots.subscribe(),
        }
    }

    pub fn snapshot(&self) -> Arc<AppSnapshot> {
        Arc::clone(&self.snapshots.borrow())
    }

    pub fn binding(&self, binding_id: &str) -> Option<Arc<Binding>> {
        self.state
            .lock()
            .bindings
            .get(binding_id)
            .map(|owner| Arc::clone(&owner.binding))
    }

    pub fn receipt(&self, receipt_id: &WriteReceiptId) -> Option<Arc<AppReceipt>> {
        self.state.lock().receipts.get(receipt_id).cloned()
    }

    /// Finite activity/event replay. A stale cursor is observable and the
    /// caller must resynchronize from the current bounded snapshot.
    pub fn events_after(&self, sequence: u64) -> EventBatch {
        let state = self.state.lock();
        let oldest_available = state
            .events
            .front()
            .map_or(state.revision, |item| item.sequence);
        let newest_available = state
            .events
            .back()
            .map_or(state.revision, |item| item.sequence);
        let cursor_was_stale = sequence.saturating_add(1) < oldest_available;
        let events = if cursor_was_stale {
            Vec::new()
        } else {
            state
                .events
                .iter()
                .filter(|item| item.sequence > sequence)
                .cloned()
                .collect()
        };
        EventBatch {
            oldest_available,
            newest_available,
            events,
            cursor_was_stale,
        }
    }

    fn install_verified(
        &self,
        state: &mut AppState,
        build: InstalledBuild,
        artifact: Arc<dyn ExecutableArtifact>,
        now: u64,
    ) {
        if artifact.manifest_kind() != 35_129 || artifact.d_tag().is_none() {
            let detail = match artifact.manifest_kind() {
                5_129 => {
                    "verified kind 5129 snapshot has no d tag; the pinned baseline does not yet define its exact-build runtime principal mapping"
                }
                15_129 => {
                    "verified kind 15129 root has no d tag; the pinned baseline does not yet define its exact-build runtime principal mapping"
                }
                _ => {
                    "verified artifact kind has no supported exact-build runtime principal mapping"
                }
            };
            self.refuse(
                state,
                AppErrorCode::UnsupportedManifestIdentity,
                Some(build.principal),
                None,
                detail,
                now,
            );
            return;
        }
        if artifact.manifest_author() != build.principal.manifest_author()
            || artifact.d_tag() != Some(build.principal.d_tag())
            || artifact.aggregate_hash() != build.principal.aggregate_hash()
        {
            self.refuse(
                state,
                AppErrorCode::ArtifactIdentityMismatch,
                Some(build.principal),
                None,
                "verified artifact aggregate does not match the exact principal",
                now,
            );
            return;
        }
        if !artifact.contains_logical_path(nmp_native_artifact::INDEX_PATH) {
            self.refuse(
                state,
                AppErrorCode::MissingIndex,
                Some(build.principal),
                None,
                "verified artifact has no /index.html",
                now,
            );
            return;
        }
        if !state.artifacts.contains_key(&build.principal)
            && state.artifacts.len() >= self.limits.maximum_installed_artifacts
        {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                Some(build.principal),
                None,
                "installed artifact handle capacity is full",
                now,
            );
            return;
        }
        if let Err(error) = self.store.install(&build) {
            self.refuse_store(state, Some(build.principal), None, error, now);
            return;
        }
        let principal = build.principal;
        state.artifacts.insert(principal.clone(), artifact);
        self.record_activity(state, &principal, "install", "verified", "completed", now);
        self.push_event(state, PlatformEvent::Installed { principal });
    }

    fn set_grant(
        &self,
        state: &mut AppState,
        principal: Principal,
        capability: Capability,
        sensitivity: Sensitivity,
        decision: GrantDecision,
        now: u64,
    ) {
        if !state.artifacts.contains_key(&principal) {
            self.refuse(
                state,
                AppErrorCode::NotInstalled,
                Some(principal),
                None,
                "grant target is not an installed exact build",
                now,
            );
            return;
        }
        if capability.as_str() == "shell" {
            self.refuse(
                state,
                AppErrorCode::Grant,
                Some(principal),
                None,
                "foundational shell is mandatory and is not grant-controlled",
                now,
            );
            return;
        }
        let previous = self.grants.decision(&principal, &capability);
        if let Err(error) =
            self.grants
                .set(principal.clone(), capability.clone(), sensitivity, decision)
        {
            self.refuse(
                state,
                AppErrorCode::Grant,
                Some(principal),
                None,
                error.to_string(),
                now,
            );
            return;
        }
        let persistent = decision != GrantDecision::AllowSession;
        if persistent && let Err(error) = self.store.set_grant(&principal, &capability, decision) {
            let _ = self
                .grants
                .set(principal.clone(), capability.clone(), sensitivity, previous);
            self.refuse_store(state, Some(principal), None, error, now);
            return;
        }
        self.record_activity(
            state,
            &principal,
            "grant",
            capability.as_str(),
            grant_outcome(decision),
            now,
        );
        self.push_event(
            state,
            PlatformEvent::GrantChanged {
                principal,
                capability,
                decision,
            },
        );
    }

    fn revoke(&self, state: &mut AppState, principal: Principal, capability: Capability, now: u64) {
        if capability.as_str() == "shell" {
            self.refuse(
                state,
                AppErrorCode::Grant,
                Some(principal),
                None,
                "foundational shell is mandatory and is not grant-controlled",
                now,
            );
            return;
        }
        if let Err(error) = self
            .store
            .set_grant(&principal, &capability, GrantDecision::Denied)
        {
            self.refuse_store(state, Some(principal), None, error, now);
            return;
        }
        self.bridge.revoke(&principal, &capability);
        let operations = state
            .operations
            .iter()
            .filter_map(|(id, operation)| {
                (operation.principal == principal && operation.domain == capability)
                    .then_some((*id, operation.session))
            })
            .collect::<Vec<_>>();
        for (id, _) in operations {
            if let Some(operation) = state.operations.remove(&id) {
                operation.handle.cancel();
            }
        }
        self.record_activity(
            state,
            &principal,
            "grant",
            capability.as_str(),
            "revoked",
            now,
        );
        self.push_event(
            state,
            PlatformEvent::GrantChanged {
                principal,
                capability,
                decision: GrantDecision::Denied,
            },
        );
    }

    fn launch(
        &self,
        state: &mut AppState,
        principal: Principal,
        profile: ExecutionProfile,
        required_domains: BTreeSet<Capability>,
        now: u64,
    ) {
        let Some(artifact) = state.artifacts.get(&principal).cloned() else {
            self.refuse(
                state,
                AppErrorCode::NotInstalled,
                Some(principal),
                None,
                "launch target is not an installed exact build",
                now,
            );
            return;
        };
        if state.sessions.len() >= self.limits.maximum_sessions {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                Some(principal),
                None,
                "session capacity is full",
                now,
            );
            return;
        }
        if let Err(error) = self.restore_persistent_grants(&principal) {
            self.refuse_store(state, Some(principal), None, error, now);
            return;
        }
        let plan = match self
            .bridge
            .negotiate(&principal, profile, &required_domains)
        {
            Ok(plan) => plan,
            Err(error) => {
                self.refuse_bridge(state, Some(principal), None, error, now);
                return;
            }
        };
        let Some(next) = state.next_session_id.checked_add(1) else {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                Some(principal),
                None,
                "session identifier space is exhausted",
                now,
            );
            return;
        };
        let session_id = SessionId(next);
        let webview = match self
            .resources
            .admit(session_id, None, ResourceClass::WebView)
        {
            Ok(lease) => lease,
            Err(error) => {
                self.refuse(
                    state,
                    AppErrorCode::Capacity,
                    Some(principal),
                    Some(session_id),
                    error.to_string(),
                    now,
                );
                return;
            }
        };
        let session = Arc::new(Session::new(
            session_id,
            principal.clone(),
            profile,
            Arc::clone(&self.resources),
        ));
        let context = SessionContext {
            id: session_id,
            principal: principal.clone(),
            profile,
        };
        if let Err(error) =
            self.shell_provider
                .prepare_session(&principal, session_id, plan.domains())
        {
            drop(webview);
            self.refuse(
                state,
                AppErrorCode::Bridge,
                Some(principal),
                Some(session_id),
                error.to_string(),
                now,
            );
            return;
        }
        if let Err(error) = self.bridge.open_session(&context, now) {
            self.shell_provider.close_session(session_id);
            drop(webview);
            self.refuse_bridge(state, Some(principal), Some(session_id), error, now);
            return;
        }
        if let Err(error) = session.transition(SessionState::Running) {
            self.shell_provider.close_session(session_id);
            self.bridge.close_session(session_id);
            drop(webview);
            self.refuse_session(state, Some(principal), Some(session_id), error, now);
            return;
        }
        state.next_session_id = next;
        state.sessions.insert(
            session_id,
            SessionEntry {
                session: Arc::clone(&session),
                context,
                plan,
                _artifact: artifact,
                _webview: webview,
            },
        );
        self.record_activity(state, &principal, "session", "launch", "running", now);
        self.push_event(state, PlatformEvent::SessionChanged(session.snapshot()));
    }

    fn end_session(
        &self,
        state: &mut AppState,
        session_id: SessionId,
        terminal: SessionState,
        reason: Option<Arc<str>>,
        now: u64,
    ) {
        let Some(entry) = state.sessions.remove(&session_id) else {
            self.refuse(
                state,
                AppErrorCode::UnknownSession,
                None,
                Some(session_id),
                "stale or unknown session",
                now,
            );
            return;
        };
        let operation_ids = state
            .operations
            .iter()
            .filter_map(|(id, operation)| (operation.session == session_id).then_some(*id))
            .collect::<Vec<_>>();
        for operation_id in operation_ids {
            if let Some(operation) = state.operations.remove(&operation_id) {
                operation.handle.cancel();
            }
        }
        self.shell_provider.close_session(session_id);
        self.bridge.close_session(session_id);
        let transition = if terminal == SessionState::Stopped {
            entry.session.stop();
            Ok(())
        } else {
            entry.session.transition(terminal)
        };
        if let Err(error) = transition {
            self.refuse_session(
                state,
                Some(entry.context.principal.clone()),
                Some(session_id),
                error,
                now,
            );
        }
        let snapshot = entry.session.snapshot();
        let outcome = match terminal {
            SessionState::Crashed => reason
                .as_deref()
                .map_or("crashed".to_owned(), |reason| format!("crashed:{reason}")),
            _ => "stopped".to_owned(),
        };
        self.record_activity(
            state,
            &entry.context.principal,
            "session",
            "teardown",
            &outcome,
            now,
        );
        drop(entry);
        self.push_event(state, PlatformEvent::SessionChanged(snapshot));
    }

    fn dispatch_envelope(
        &self,
        state: &mut AppState,
        session_id: SessionId,
        bytes: &[u8],
        now: u64,
    ) {
        if bytes.len() > self.limits.maximum_envelope_bytes {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                None,
                Some(session_id),
                "mapped envelope exceeds the application bound",
                now,
            );
            return;
        }
        let Some(entry) = state.sessions.get(&session_id) else {
            self.refuse(
                state,
                AppErrorCode::UnknownSession,
                None,
                Some(session_id),
                "stale or unknown session",
                now,
            );
            return;
        };
        let principal = entry.context.principal.clone();
        let context = entry.context.clone();
        let plan = entry.plan.clone();
        let route = envelope_route(bytes);
        let domain = route.as_ref().map(|(domain, _)| domain.clone());
        if route.as_ref().is_some_and(|(domain, action)| {
            domain.as_str() == "shell" && action == "ready" && !exact_shell_ready(bytes)
        }) {
            self.refuse(
                state,
                AppErrorCode::Bridge,
                Some(principal),
                Some(session_id),
                "shell.ready must be exactly the uncorrelated liveness envelope",
                now,
            );
            return;
        }
        if route.as_ref().is_some_and(|(domain, action)| {
            domain.as_str() != "shell"
                && self
                    .mapped_routes
                    .contains(&(domain.clone(), Arc::from(action.as_str())))
        }) && !self.shell_provider.is_ready(session_id)
        {
            self.refuse(
                state,
                AppErrorCode::Bridge,
                Some(principal),
                Some(session_id),
                "NAP-SHELL handshake has not established this mapped session",
                now,
            );
            return;
        }
        match self.bridge.dispatch(&context, &plan, bytes, now) {
            Ok(DispatchOutcome::IgnoredUnknown) => {
                self.push_event(
                    state,
                    PlatformEvent::EnvelopeIgnored {
                        session: session_id,
                    },
                );
            }
            Ok(DispatchOutcome::Handled(mut call)) => {
                if domain
                    .as_ref()
                    .is_some_and(|domain| domain.as_str() == "shell")
                    && call.response.is_some()
                    && !shell_init_matches_plan(call.response.as_ref(), &plan)
                {
                    self.shell_provider.close_session(session_id);
                    self.refuse(
                        state,
                        AppErrorCode::Bridge,
                        Some(principal),
                        Some(session_id),
                        "shell.init capability set does not match the fixed session plan",
                        now,
                    );
                    return;
                }
                let operation = if let Some(handle) = call.take_operation() {
                    if state.operations.len() >= self.limits.maximum_provider_operations {
                        handle.cancel();
                        self.refuse(
                            state,
                            AppErrorCode::Capacity,
                            Some(principal),
                            Some(session_id),
                            "provider operation ownership capacity is full",
                            now,
                        );
                        return;
                    }
                    let Some(next) = state.next_operation_id.checked_add(1) else {
                        handle.cancel();
                        self.refuse(
                            state,
                            AppErrorCode::Capacity,
                            Some(principal),
                            Some(session_id),
                            "provider operation identifier space is exhausted",
                            now,
                        );
                        return;
                    };
                    let domain = domain.clone().unwrap_or_else(|| {
                        Capability::new("unknown").expect("static capability is valid")
                    });
                    let id = ProviderOperationId(next);
                    state.next_operation_id = next;
                    state.operations.insert(
                        id,
                        ActiveOperation {
                            session: session_id,
                            principal: principal.clone(),
                            domain,
                            handle,
                        },
                    );
                    Some(id)
                } else {
                    None
                };
                self.push_event(
                    state,
                    PlatformEvent::EnvelopeHandled {
                        session: session_id,
                        operation,
                        response: call.response,
                    },
                );
            }
            Err(BridgeError::SessionIdentityMismatch { .. }) => {
                self.refuse(
                    state,
                    AppErrorCode::SessionIdentityMismatch,
                    Some(principal),
                    Some(session_id),
                    "mapped source no longer matches the fixed session identity",
                    now,
                );
            }
            Err(error) => {
                self.refuse_bridge(state, Some(principal), Some(session_id), error, now);
            }
        }
    }

    fn complete_operation(
        &self,
        state: &mut AppState,
        operation_id: ProviderOperationId,
        now: u64,
    ) {
        let Some(operation) = state.operations.remove(&operation_id) else {
            self.refuse(
                state,
                AppErrorCode::Bridge,
                None,
                None,
                "unknown provider operation",
                now,
            );
            return;
        };
        operation.handle.complete();
        self.push_event(
            state,
            PlatformEvent::ProviderOperationFinished {
                operation: operation_id,
            },
        );
    }

    fn open_binding(&self, state: &mut AppState, request: BindingRequest, now: u64) {
        if state.bindings.contains_key(&request.workspace_binding_id) {
            self.refuse(
                state,
                AppErrorCode::Binding,
                None,
                None,
                "workspace binding id is already open",
                now,
            );
            return;
        }
        if state.bindings.len() >= self.limits.maximum_bindings {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                None,
                None,
                "binding capacity is full",
                now,
            );
            return;
        }
        let binding = match Binding::new(
            Arc::clone(&request.workspace_binding_id),
            Arc::clone(&request.schema),
            self.binding_limits,
        ) {
            Ok(binding) => binding,
            Err(error) => {
                self.refuse_binding(state, error, now);
                return;
            }
        };
        let source = match self
            .data_plane
            .open_binding(request.clone(), binding.clone())
        {
            Ok(source) => source,
            Err(error) => {
                self.refuse(
                    state,
                    AppErrorCode::HostData,
                    None,
                    None,
                    error.to_string(),
                    now,
                );
                return;
            }
        };
        let logical_source_id: Arc<str> = Arc::from(source.logical_id());
        if let Err(error) = binding.attach_source(source) {
            self.refuse_binding(state, error, now);
            return;
        }
        let binding_id = Arc::clone(&request.workspace_binding_id);
        state
            .bindings
            .insert(Arc::clone(&binding_id), BindingOwner { request, binding });
        self.push_event(
            state,
            PlatformEvent::BindingOpened {
                binding_id,
                logical_source_id,
            },
        );
    }

    fn close_binding(&self, state: &mut AppState, binding_id: &Arc<str>, now: u64) {
        let Some(owner) = state.bindings.remove(binding_id) else {
            self.refuse(
                state,
                AppErrorCode::Binding,
                None,
                None,
                "unknown binding",
                now,
            );
            return;
        };
        owner.binding.close();
        self.push_event(
            state,
            PlatformEvent::BindingClosed {
                binding_id: Arc::clone(binding_id),
            },
        );
    }

    fn approve_write(&self, state: &mut AppState, write: ApprovedWrite, now: u64) {
        let Some(session) = state.sessions.get(&write.origin_session) else {
            self.refuse(
                state,
                AppErrorCode::UnknownSession,
                Some(write.origin_principal),
                Some(write.origin_session),
                "write approval names a stale or stopped origin session",
                now,
            );
            return;
        };
        if session.context.principal != write.origin_principal {
            self.refuse(
                state,
                AppErrorCode::SessionIdentityMismatch,
                Some(write.origin_principal),
                Some(write.origin_session),
                "write approval principal does not match the fixed origin session",
                now,
            );
            return;
        }
        if state.receipts.len() >= self.limits.maximum_receipts {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                Some(write.origin_principal),
                Some(write.origin_session),
                "receipt ownership capacity is full before write acceptance",
                now,
            );
            return;
        }
        let principal = write.origin_principal.clone();
        let origin_session = write.origin_session;
        let expected_account = write.account.clone();
        let receipt = Arc::new(AppReceipt::unassigned(
            self.limits.maximum_receipt_frame_bytes,
        ));
        let accepted = match self.data_plane.accept_write(write, receipt.clone()) {
            Ok(accepted) => accepted,
            Err(error) => {
                self.refuse(
                    state,
                    AppErrorCode::HostData,
                    Some(principal),
                    Some(origin_session),
                    error.to_string(),
                    now,
                );
                return;
            }
        };
        if let Err(detail) = receipt.assign(accepted.receipt_id.clone()) {
            self.refuse(
                state,
                AppErrorCode::Receipt,
                Some(principal.clone()),
                Some(origin_session),
                detail,
                now,
            );
        }
        if accepted.frozen_account != expected_account {
            self.refuse(
                state,
                AppErrorCode::Receipt,
                Some(principal.clone()),
                Some(origin_session),
                "host data plane returned a different frozen account",
                now,
            );
        }
        state.receipts.insert(accepted.receipt_id.clone(), receipt);
        self.record_activity(
            state,
            &principal,
            "write",
            "accept",
            "durable-obligation",
            now,
        );
        self.push_event(
            state,
            PlatformEvent::WriteAccepted {
                receipt_id: accepted.receipt_id,
                frozen_account: accepted.frozen_account,
            },
        );
    }

    fn save_workspace(&self, state: &mut AppState, workspace: WorkspaceRecord, now: u64) {
        if let Err(error) = self.store.save_workspace(&workspace) {
            self.refuse_store(state, None, None, error, now);
            return;
        }
        let workspace_id = Arc::clone(&workspace.id);
        state
            .workspaces
            .insert(Arc::clone(&workspace_id), workspace);
        self.push_event(state, PlatformEvent::WorkspaceSaved { workspace_id });
    }

    fn restore_workspaces(&self, state: &mut AppState, now: u64) {
        let workspaces = match self.store.load_workspaces() {
            Ok(workspaces) => workspaces,
            Err(error) => {
                self.refuse_store(state, None, None, error, now);
                return;
            }
        };
        for workspace in workspaces {
            let workspace_id = Arc::clone(&workspace.id);
            for receipt_id in workspace.retained_receipts.iter().cloned() {
                if state.receipts.contains_key(&receipt_id) {
                    continue;
                }
                if state.receipts.len() >= self.limits.maximum_receipts {
                    self.refuse(
                        state,
                        AppErrorCode::Capacity,
                        None,
                        None,
                        "receipt restoration capacity is full",
                        now,
                    );
                    break;
                }
                self.reattach_receipt(state, receipt_id, now);
            }
            state.workspaces.insert(workspace_id.clone(), workspace);
            self.push_event(state, PlatformEvent::WorkspaceRestored { workspace_id });
        }
    }

    fn reattach_receipt(&self, state: &mut AppState, receipt_id: WriteReceiptId, now: u64) {
        let receipt = Arc::new(AppReceipt::assigned(
            receipt_id.clone(),
            self.limits.maximum_receipt_frame_bytes,
        ));
        match self
            .data_plane
            .reattach_receipt(receipt_id.clone(), receipt.clone())
        {
            Ok(ReceiptReattachment::Attached(observation)) => {
                if observation.receipt_id() != &receipt_id {
                    observation.stop_delivery();
                    receipt.set_closed();
                    self.refuse(
                        state,
                        AppErrorCode::Receipt,
                        None,
                        None,
                        "receipt observation identity mismatch",
                        now,
                    );
                } else {
                    receipt.attach_observation(observation);
                    state.receipts.insert(receipt_id.clone(), receipt);
                    self.push_event(state, PlatformEvent::ReceiptReattached { receipt_id });
                }
            }
            Ok(ReceiptReattachment::NotFound) => {
                receipt.set_not_found();
                state.receipts.insert(receipt_id.clone(), receipt);
                self.push_event(state, PlatformEvent::ReceiptNotFound { receipt_id });
            }
            Err(error) => {
                self.refuse(
                    state,
                    AppErrorCode::HostData,
                    None,
                    None,
                    error.to_string(),
                    now,
                );
            }
        }
    }

    fn close(&self, state: &mut AppState, now: u64) {
        if state.closed {
            return;
        }
        let sessions = state.sessions.keys().copied().collect::<Vec<_>>();
        for session in sessions {
            self.end_session(state, session, SessionState::Stopped, None, now);
        }
        let bindings = state.bindings.keys().cloned().collect::<Vec<_>>();
        for binding in bindings {
            self.close_binding(state, &binding, now);
        }
        for (_, receipt) in std::mem::take(&mut state.receipts) {
            receipt.stop_delivery();
        }
        state.operations.clear();
        state.artifacts.clear();
        state.closed = true;
        self.push_event(state, PlatformEvent::Closed);
    }

    fn restore_persistent_grants(&self, principal: &Principal) -> Result<(), StoreError> {
        for capability in self.bridge.advertised_domains() {
            let decision = self.store.grant(principal, &capability)?;
            if decision != GrantDecision::Denied {
                self.grants
                    .set(
                        principal.clone(),
                        capability,
                        Sensitivity::Sensitive,
                        decision,
                    )
                    .map_err(|error| StoreError::Corrupt(error.to_string()))?;
            }
        }
        Ok(())
    }

    fn publish(&self, state: &mut AppState) {
        state.revision = state.revision.saturating_add(1);
        let snapshot = Arc::new(self.build_snapshot(state));
        self.snapshots.send_replace(snapshot);
    }

    fn build_snapshot(&self, state: &AppState) -> AppSnapshot {
        AppSnapshot {
            revision: state.revision,
            closed: state.closed,
            sessions: state
                .sessions
                .values()
                .map(|entry| entry.session.snapshot())
                .collect(),
            session_domains: state
                .sessions
                .iter()
                .map(|(session, entry)| SessionDomainView {
                    session: *session,
                    domains: entry.plan.domains().iter().cloned().collect(),
                })
                .collect(),
            bindings: state
                .bindings
                .iter()
                .map(|(id, owner)| BindingView {
                    id: Arc::clone(id),
                    schema: Arc::clone(&owner.request.schema),
                    logical_source_id: owner.binding.logical_source_id().map(Arc::from),
                    revision: owner.binding.latest().map(|snapshot| snapshot.revision),
                })
                .collect(),
            receipts: state
                .receipts
                .values()
                .filter_map(|receipt| receipt.view())
                .collect(),
            workspaces: state
                .workspaces
                .values()
                .map(|workspace| WorkspaceView {
                    id: Arc::clone(&workspace.id),
                    definition: workspace.definition.clone(),
                    retained_receipts: workspace.retained_receipts.clone(),
                })
                .collect(),
            resources: self.resources.census(),
            recent_activity: state.activity.iter().cloned().collect(),
            recent_errors: state.errors.iter().cloned().collect(),
        }
    }

    fn push_event(&self, state: &mut AppState, event: PlatformEvent) {
        let sequence = state.revision.saturating_add(1);
        push_bounded(
            &mut state.events,
            self.limits.maximum_platform_events,
            SequencedPlatformEvent { sequence, event },
        );
    }

    fn record_activity(
        &self,
        state: &mut AppState,
        principal: &Principal,
        category: &str,
        operation: &str,
        outcome: &str,
        now: u64,
    ) {
        let fact = ActivityFact {
            principal: principal.clone(),
            category: Arc::from(category),
            operation: Arc::from(operation),
            outcome: Arc::from(outcome),
            occurred_at_millis: now,
        };
        let persisted = ActivityRecord {
            principal: fact.principal.clone(),
            category: Arc::clone(&fact.category),
            operation: Arc::clone(&fact.operation),
            outcome: Arc::clone(&fact.outcome),
            occurred_at_millis: now,
        };
        push_bounded(
            &mut state.activity,
            self.limits.maximum_activity_facts,
            fact,
        );
        if let Err(error) = self.store.append_activity(&persisted) {
            self.record_error(
                state,
                AppErrorFact {
                    code: AppErrorCode::Store,
                    principal: Some(principal.clone()),
                    session: None,
                    detail: Arc::from(error.to_string()),
                    occurred_at_millis: now,
                },
            );
        }
    }

    fn refuse(
        &self,
        state: &mut AppState,
        code: AppErrorCode,
        principal: Option<Principal>,
        session: Option<SessionId>,
        detail: impl Into<Arc<str>>,
        now: u64,
    ) {
        let fact = AppErrorFact {
            code,
            principal,
            session,
            detail: detail.into(),
            occurred_at_millis: now,
        };
        self.record_error(state, fact.clone());
        self.push_event(state, PlatformEvent::Refused(fact));
    }

    fn record_error(&self, state: &mut AppState, fact: AppErrorFact) {
        push_bounded(&mut state.errors, self.limits.maximum_error_facts, fact);
    }

    fn refuse_store(
        &self,
        state: &mut AppState,
        principal: Option<Principal>,
        session: Option<SessionId>,
        error: StoreError,
        now: u64,
    ) {
        self.refuse(
            state,
            AppErrorCode::Store,
            principal,
            session,
            error.to_string(),
            now,
        );
    }

    fn refuse_bridge(
        &self,
        state: &mut AppState,
        principal: Option<Principal>,
        session: Option<SessionId>,
        error: BridgeError,
        now: u64,
    ) {
        self.refuse(
            state,
            AppErrorCode::Bridge,
            principal,
            session,
            error.to_string(),
            now,
        );
    }

    fn refuse_session(
        &self,
        state: &mut AppState,
        principal: Option<Principal>,
        session: Option<SessionId>,
        error: SessionError,
        now: u64,
    ) {
        self.refuse(
            state,
            AppErrorCode::InvalidLifecycle,
            principal,
            session,
            error.to_string(),
            now,
        );
    }

    fn refuse_binding(&self, state: &mut AppState, error: BindingError, now: u64) {
        self.refuse(
            state,
            AppErrorCode::Binding,
            None,
            None,
            error.to_string(),
            now,
        );
    }
}

impl Drop for RuntimeApp {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        state.operations.clear();
        for (session_id, entry) in std::mem::take(&mut state.sessions) {
            self.shell_provider.close_session(session_id);
            self.bridge.close_session(session_id);
            entry.session.stop();
        }
        for (_, owner) in std::mem::take(&mut state.bindings) {
            owner.binding.close();
        }
        for (_, receipt) in std::mem::take(&mut state.receipts) {
            receipt.stop_delivery();
        }
        state.artifacts.clear();
        state.closed = true;
    }
}

#[derive(Debug)]
pub struct AppReceipt {
    maximum_frame_bytes: usize,
    inner: Mutex<AppReceiptState>,
    closed: AtomicBool,
    snapshots: watch::Sender<Option<ReceiptSnapshot>>,
}

#[derive(Debug)]
struct AppReceiptState {
    receipt_id: Option<WriteReceiptId>,
    delivery: ReceiptDeliveryState,
    latest: Option<ReceiptSnapshot>,
    observation: Option<Arc<dyn ReceiptObservation>>,
}

#[derive(Debug)]
pub struct ReceiptObserver {
    receiver: watch::Receiver<Option<ReceiptSnapshot>>,
}

impl ReceiptObserver {
    pub fn latest(&self) -> Option<ReceiptSnapshot> {
        self.receiver.borrow().clone()
    }

    pub async fn changed(&mut self) -> Result<ReceiptSnapshot, ObservationClosed> {
        loop {
            self.receiver
                .changed()
                .await
                .map_err(|_| ObservationClosed)?;
            if let Some(snapshot) = self.receiver.borrow_and_update().clone() {
                return Ok(snapshot);
            }
        }
    }
}

impl AppReceipt {
    fn unassigned(maximum_frame_bytes: usize) -> Self {
        Self::new(None, maximum_frame_bytes)
    }

    fn assigned(receipt_id: WriteReceiptId, maximum_frame_bytes: usize) -> Self {
        Self::new(Some(receipt_id), maximum_frame_bytes)
    }

    fn new(receipt_id: Option<WriteReceiptId>, maximum_frame_bytes: usize) -> Self {
        let (snapshots, _) = watch::channel(None);
        Self {
            maximum_frame_bytes,
            inner: Mutex::new(AppReceiptState {
                receipt_id,
                delivery: ReceiptDeliveryState::Observing,
                latest: None,
                observation: None,
            }),
            closed: AtomicBool::new(false),
            snapshots,
        }
    }

    fn assign(&self, receipt_id: WriteReceiptId) -> Result<(), Arc<str>> {
        let mut inner = self.inner.lock();
        if let Some(existing) = &inner.receipt_id
            && existing != &receipt_id
        {
            return Err(Arc::from(
                "receipt sink observed a different id before acceptance returned",
            ));
        }
        if inner
            .latest
            .as_ref()
            .is_some_and(|snapshot| snapshot.receipt_id != receipt_id)
        {
            return Err(Arc::from(
                "receipt snapshot identity differs from accepted receipt",
            ));
        }
        inner.receipt_id = Some(receipt_id);
        Ok(())
    }

    fn attach_observation(&self, observation: Arc<dyn ReceiptObservation>) {
        self.inner.lock().observation = Some(observation);
    }

    fn set_not_found(&self) {
        self.inner.lock().delivery = ReceiptDeliveryState::NotFound;
    }

    fn set_closed(&self) {
        self.closed.store(true, Ordering::Release);
        self.inner.lock().delivery = ReceiptDeliveryState::Closed;
    }

    pub fn observe(&self) -> ReceiptObserver {
        ReceiptObserver {
            receiver: self.snapshots.subscribe(),
        }
    }

    pub fn view(&self) -> Option<ReceiptView> {
        let inner = self.inner.lock();
        Some(ReceiptView {
            receipt_id: inner.receipt_id.clone()?,
            delivery: inner.delivery,
            latest: inner.latest.clone(),
        })
    }

    /// Ends this app consumer's delivery. It does not cancel or weaken the NMP
    /// durable write obligation.
    pub fn stop_delivery(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut inner = self.inner.lock();
        if let Some(observation) = inner.observation.take() {
            observation.stop_delivery();
        }
        inner.delivery = ReceiptDeliveryState::Closed;
    }
}

impl ReceiptEventSink for AppReceipt {
    fn push_latest(&self, snapshot: ReceiptSnapshot) -> Result<(), ReceiptSinkError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ReceiptSinkError::Closed);
        }
        if snapshot.state.byte_len() > self.maximum_frame_bytes {
            return Err(ReceiptSinkError::FrameTooLarge);
        }
        let mut inner = self.inner.lock();
        if let Some(receipt_id) = &inner.receipt_id
            && receipt_id != &snapshot.receipt_id
        {
            return Err(ReceiptSinkError::Closed);
        }
        if inner.receipt_id.is_none() {
            inner.receipt_id = Some(snapshot.receipt_id.clone());
        }
        inner.latest = Some(snapshot.clone());
        self.snapshots.send_replace(Some(snapshot));
        Ok(())
    }

    fn close(&self, _reason: Option<Arc<str>>) {
        self.set_closed();
    }
}

impl Drop for AppReceipt {
    fn drop(&mut self) {
        self.stop_delivery();
    }
}

#[derive(Debug)]
struct NoopBridgeActivity;

impl ActivitySink for NoopBridgeActivity {
    fn record(&self, _fact: ProviderActivity) {}
}

fn push_bounded<T>(queue: &mut VecDeque<T>, maximum: usize, value: T) {
    if queue.len() == maximum {
        queue.pop_front();
    }
    queue.push_back(value);
}

fn grant_outcome(decision: GrantDecision) -> &'static str {
    match decision {
        GrantDecision::Denied => "denied",
        GrantDecision::AskEveryTime => "ask-every-time",
        GrantDecision::AllowSession => "allowed-session",
        GrantDecision::AllowExactBuild => "allowed-exact-build",
        GrantDecision::Managed => "managed",
    }
}

fn envelope_route(bytes: &[u8]) -> Option<(Capability, String)> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let message_type = value.get("type")?.as_str()?;
    let (domain, action) = message_type.split_once('.')?;
    Some((Capability::new(domain).ok()?, action.to_owned()))
}

fn exact_shell_ready(bytes: &[u8]) -> bool {
    let Ok(serde_json::Value::Object(fields)) = serde_json::from_slice::<serde_json::Value>(bytes)
    else {
        return false;
    };
    fields.len() == 1
        && fields
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message_type| message_type == "shell.ready")
}

fn shell_init_matches_plan(response: Option<&BoundedJson>, plan: &InjectionPlan) -> bool {
    let Some(response) = response.and_then(|response| response.decode().ok()) else {
        return false;
    };
    let Some(domains) = response
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("domains"))
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    let Some(domains) = domains
        .iter()
        .map(serde_json::Value::as_str)
        .collect::<Option<BTreeSet<_>>>()
    else {
        return false;
    };
    let planned = plan
        .domains()
        .iter()
        .map(Capability::as_str)
        .collect::<BTreeSet<_>>();
    domains == planned
}
