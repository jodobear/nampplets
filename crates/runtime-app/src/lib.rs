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
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use nmp_native_artifact::VerifiedArtifactHandle;
use nmp_native_nap_bridge::{
    ActivitySink, BridgeError, BridgeLimits, DispatchOutcome, InjectionPlan, Provider,
    ProviderActivity, ProviderDescriptor, ProviderOperation, ProviderPlatformAvailability,
    ProviderPush, ProviderPushBatch, ProviderPushError, ProviderPushObserver,
    ProviderPushTermination, ProviderRegistry, ProviderSessionEnd, ProviderWriteProposal,
    SessionContext, SourceWindowId,
};
use nmp_native_providers::ShellProvider;
use nmp_native_runtime_core::{
    ApprovedWrite, BindingRequest, BoundedJson, Capability, CapabilityRequirement,
    ExecutionProfile, GrantBatchError, GrantDecision, GrantError, GrantLedger, GrantLimits,
    HostDataPlane, Principal, ReceiptEventSink, ReceiptObservation, ReceiptReattachment,
    ReceiptSinkError, ReceiptSnapshot, ResourceCensus, ResourceClass, ResourceLimits,
    ResourceRefusal, ResourceTracker, Sensitivity, Session, SessionError, SessionId,
    SessionSnapshot, SessionState, WorkLease, WriteReceiptId,
};
use nmp_native_runtime_store::{
    ActivityRecord, InstalledBuild, RuntimeStore, StoreError, UninstallCleanupPolicy,
    UninstallReport, WorkspaceRecord,
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
    pub maximum_library_query_bytes: usize,
    pub maximum_sessions: usize,
    pub maximum_bindings: usize,
    pub maximum_receipts: usize,
    pub maximum_provider_operations: usize,
    pub maximum_activity_facts: usize,
    pub maximum_error_facts: usize,
    pub maximum_platform_events: usize,
    pub maximum_provider_push_batch: usize,
    pub maximum_receipt_frame_bytes: usize,
    pub maximum_envelope_bytes: usize,
}

impl Default for AppLimits {
    fn default() -> Self {
        Self {
            maximum_installed_artifacts: 512,
            maximum_library_query_bytes: 256,
            maximum_sessions: 16,
            maximum_bindings: 64,
            maximum_receipts: 256,
            maximum_provider_operations: 128,
            maximum_activity_facts: 1_024,
            maximum_error_facts: 256,
            maximum_platform_events: 1_024,
            maximum_provider_push_batch: 64,
            maximum_receipt_frame_bytes: 256 * 1024,
            maximum_envelope_bytes: 256 * 1024,
        }
    }
}

impl AppLimits {
    fn validate(self) -> Result<Self, OpenError> {
        if [
            self.maximum_installed_artifacts,
            self.maximum_library_query_bytes,
            self.maximum_sessions,
            self.maximum_bindings,
            self.maximum_receipts,
            self.maximum_provider_operations,
            self.maximum_activity_facts,
            self.maximum_error_facts,
            self.maximum_platform_events,
            self.maximum_provider_push_batch,
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
    SetLibraryFilter {
        query: Arc<str>,
    },
    Uninstall {
        principal: Principal,
        cleanup: UninstallCleanupPolicy,
    },
    SetGrant {
        principal: Principal,
        capability: Capability,
        sensitivity: Sensitivity,
        decision: GrantDecision,
    },
    ApplyPermissionBatch {
        principal: Principal,
        decisions: Vec<PermissionDecision>,
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
    Suspend {
        session: SessionId,
    },
    Resume {
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
    DecideProviderWrite {
        operation: ProviderOperationId,
        approve: bool,
    },
    SaveWorkspace {
        workspace: WorkspaceRecord,
    },
    AssignWorkspaceBuild {
        workspace_id: Arc<str>,
        principal: Principal,
    },
    RemoveWorkspaceBuild {
        workspace_id: Arc<str>,
        principal: Principal,
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
    LibraryFilterChanged {
        query: Arc<str>,
    },
    Uninstalled {
        principal: Principal,
        cleanup: UninstallReport,
    },
    GrantChanged {
        principal: Principal,
        capability: Capability,
        decision: GrantDecision,
    },
    PermissionBatchApplied {
        principal: Principal,
        decisions: Vec<PermissionDecision>,
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
    ProviderPush {
        session: SessionId,
        source_window: SourceWindowId,
        provider_sequence: u64,
        domain: Capability,
        envelope: BoundedJson,
    },
    ProviderPushLaneClosed {
        session: SessionId,
        source_window: SourceWindowId,
        termination: Option<ProviderPushTermination>,
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
    WorkspaceAssignmentChanged {
        workspace_id: Arc<str>,
        principal: Principal,
        assigned: bool,
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
    OfflineBytesUnavailable,
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

/// Bounded provider-to-component delivery state for one exact mapped source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderPushLaneView {
    pub session: SessionId,
    pub source_window: SourceWindowId,
    pub ready: bool,
    pub last_provider_sequence: Option<u64>,
    pub delivered_count: u64,
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
pub struct ProviderWriteProposalView {
    pub operation: ProviderOperationId,
    pub approval_id: Arc<str>,
    pub principal: Principal,
    pub session: SessionId,
    pub account: nmp_native_runtime_core::AccountRef,
    pub draft: BoundedJson,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceView {
    pub id: Arc<str>,
    pub definition: BoundedJson,
    pub retained_receipts: Vec<WriteReceiptId>,
    pub assigned_builds: Vec<Principal>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstalledBuildAvailability {
    /// Verified metadata survived restart, but no live immutable artifact
    /// handle currently proves that the sealed bytes are available offline.
    MetadataOnly,
    /// The runtime holds a verifier-produced immutable handle for this exact
    /// aggregate and can launch without resolving mutable network state.
    SealedExactBytesReady,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledBuildView {
    pub build: InstalledBuild,
    pub availability: InstalledBuildAvailability,
    pub active_sessions: Vec<SessionId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledLibraryView {
    pub query: Arc<str>,
    pub total_installed: usize,
    pub builds: Vec<InstalledBuildView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionPlatformAvailability {
    Available,
    Unknown { reason: Arc<str> },
    Unavailable { reason: Arc<str> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionCapabilityView {
    pub capability: Capability,
    pub requirement: CapabilityRequirement,
    pub sensitivity: Option<Sensitivity>,
    pub dependencies: Vec<Capability>,
    pub platform_availability: PermissionPlatformAvailability,
    pub current_decision: GrantDecision,
    pub requested_decision: Option<GrantDecision>,
    pub decision_options: Vec<PermissionDecisionOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionDecisionOption {
    pub decision: GrantDecision,
    pub valid: bool,
    pub invalid_reason: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionReviewView {
    pub principal: Principal,
    pub title: Arc<str>,
    pub capabilities: Vec<PermissionCapabilityView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionDecision {
    pub capability: Capability,
    pub decision: GrantDecision,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PermissionReviewError {
    #[error("permission target is not an installed exact build")]
    NotInstalled,
    #[error("persistent grant state could not be read: {detail}")]
    Store { detail: Arc<str> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppSnapshot {
    pub revision: u64,
    pub closed: bool,
    pub library: InstalledLibraryView,
    pub sessions: Vec<SessionSnapshot>,
    pub session_domains: Vec<SessionDomainView>,
    pub provider_push_lanes: Vec<ProviderPushLaneView>,
    pub bindings: Vec<BindingView>,
    pub pending_writes: Vec<ProviderWriteProposalView>,
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
    next_source_window_id: u64,
    next_operation_id: u64,
    next_event_sequence: u64,
    revision: u64,
    closed: bool,
    library_query: Arc<str>,
    installed: BTreeMap<Principal, InstalledBuild>,
    artifacts: BTreeMap<Principal, Arc<dyn ExecutableArtifact>>,
    sessions: BTreeMap<SessionId, SessionEntry>,
    operations: BTreeMap<ProviderOperationId, ActiveOperation>,
    bindings: BTreeMap<Arc<str>, BindingOwner>,
    receipts: BTreeMap<WriteReceiptId, Arc<AppReceipt>>,
    workspaces: BTreeMap<Arc<str>, WorkspaceRecord>,
    workspace_assignments: BTreeMap<Arc<str>, BTreeSet<Principal>>,
    activity: VecDeque<ActivityFact>,
    errors: VecDeque<AppErrorFact>,
    events: VecDeque<SequencedPlatformEvent>,
}

#[derive(Debug)]
struct SessionEntry {
    session: Arc<Session>,
    context: SessionContext,
    plan: InjectionPlan,
    source_window: SourceWindowId,
    push_observer: Option<ProviderPushObserver>,
    push_delivery: Option<ProviderPushDelivery>,
    ready: bool,
    last_provider_sequence: Option<u64>,
    delivered_push_count: u64,
    _artifact: Arc<dyn ExecutableArtifact>,
    _webview: WorkLease,
}

#[derive(Debug)]
struct ProviderPushDelivery {
    join: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct ActiveOperation {
    session: SessionId,
    principal: Principal,
    domain: Capability,
    handle: Option<ProviderOperation>,
    proposal: Option<ProviderWriteProposal>,
}

impl ActiveOperation {
    fn cancel(self, reason: Arc<str>) {
        if let Some(proposal) = self.proposal {
            proposal.refuse(reason);
        }
        if let Some(handle) = self.handle {
            handle.cancel();
        }
    }

    fn complete(self) {
        drop(self.proposal);
        if let Some(handle) = self.handle {
            handle.complete();
        }
    }
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
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("persistent library has {actual} builds; the application maximum is {maximum}")]
    InstalledLibraryCapacity { actual: usize, maximum: usize },
}

impl RuntimeApp {
    pub fn open(config: RuntimeAppConfig) -> Result<Arc<Self>, OpenError> {
        let limits = config.limits.validate()?;
        let installed = config
            .store
            .installed_builds()?
            .into_iter()
            .map(|build| (build.principal.clone(), build))
            .collect::<BTreeMap<_, _>>();
        if installed.len() > limits.maximum_installed_artifacts {
            return Err(OpenError::InstalledLibraryCapacity {
                actual: installed.len(),
                maximum: limits.maximum_installed_artifacts,
            });
        }
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
            library: installed_library_view(&installed, &BTreeMap::new(), &BTreeMap::new(), ""),
            sessions: Vec::new(),
            session_domains: Vec::new(),
            provider_push_lanes: Vec::new(),
            bindings: Vec::new(),
            pending_writes: Vec::new(),
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
                next_source_window_id: 0,
                next_operation_id: 0,
                next_event_sequence: 0,
                revision: 0,
                closed: false,
                library_query: Arc::from(""),
                installed,
                artifacts: BTreeMap::new(),
                sessions: BTreeMap::new(),
                operations: BTreeMap::new(),
                bindings: BTreeMap::new(),
                receipts: BTreeMap::new(),
                workspaces: BTreeMap::new(),
                workspace_assignments: BTreeMap::new(),
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
    pub fn dispatch(self: &Arc<Self>, command: PlatformCommand) {
        let now = self.clock.now_millis();
        let mut state = self.state.lock();
        let mut delivery_joins = Vec::new();
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
            PlatformCommand::SetLibraryFilter { query } => {
                self.set_library_filter(&mut state, query, now);
            }
            PlatformCommand::Uninstall { principal, cleanup } => {
                delivery_joins.extend(self.uninstall(&mut state, principal, cleanup, now));
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
            PlatformCommand::ApplyPermissionBatch {
                principal,
                decisions,
            } => self.apply_permission_batch(&mut state, principal, decisions, now),
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
                if let Some(join) =
                    self.end_session(&mut state, session, SessionState::Stopped, None, now)
                {
                    delivery_joins.push(join);
                }
            }
            PlatformCommand::Suspend { session } => {
                self.transition_session(&mut state, session, SessionState::Suspended, now);
            }
            PlatformCommand::Resume { session } => {
                self.transition_session(&mut state, session, SessionState::Running, now);
            }
            PlatformCommand::Crash { session, reason } => {
                if let Some(join) = self.end_session(
                    &mut state,
                    session,
                    SessionState::Crashed,
                    Some(reason),
                    now,
                ) {
                    delivery_joins.push(join);
                }
            }
            PlatformCommand::MappedEnvelope { session, bytes } => {
                if let Some(join) = self.dispatch_envelope(&mut state, session, &bytes, now) {
                    delivery_joins.push(join);
                }
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
            PlatformCommand::DecideProviderWrite { operation, approve } => {
                self.decide_provider_write(&mut state, operation, approve, now);
            }
            PlatformCommand::SaveWorkspace { workspace } => {
                self.save_workspace(&mut state, workspace, now);
            }
            PlatformCommand::AssignWorkspaceBuild {
                workspace_id,
                principal,
            } => self.assign_workspace_build(&mut state, workspace_id, principal, true, now),
            PlatformCommand::RemoveWorkspaceBuild {
                workspace_id,
                principal,
            } => self.assign_workspace_build(&mut state, workspace_id, principal, false, now),
            PlatformCommand::RestoreWorkspaces => self.restore_workspaces(&mut state, now),
            PlatformCommand::Close => delivery_joins.extend(self.close(&mut state, now)),
        }
        drop(state);
        for join in delivery_joins {
            let _ = join.join();
        }
        let mut state = self.state.lock();
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

    /// Builds one bounded exact-build permission review from Rust-owned
    /// installation requests, provider metadata, live session grants, and
    /// durable grant rows. Missing provider metadata stays explicitly unknown.
    pub fn permission_review(
        &self,
        principal: &Principal,
    ) -> Result<PermissionReviewView, PermissionReviewError> {
        let build = self
            .state
            .lock()
            .installed
            .get(principal)
            .cloned()
            .ok_or(PermissionReviewError::NotInstalled)?;
        let mut capabilities = Vec::with_capacity(build.capability_requests.len());
        for request in &build.capability_requests {
            let persistent = self
                .store
                .grant(principal, &request.capability)
                .map_err(|error| PermissionReviewError::Store {
                    detail: Arc::from(error.to_string()),
                })?;
            let current_decision = self
                .grants
                .decision_entry(principal, &request.capability)
                .unwrap_or(persistent);
            let descriptor = self.bridge.permission_descriptor(&request.capability);
            let (sensitivity, dependencies, platform_availability) =
                permission_provider_projection(descriptor);
            let (requested_decision, decision_options) =
                permission_decision_policy(current_decision, &platform_availability);
            capabilities.push(PermissionCapabilityView {
                capability: request.capability.clone(),
                requirement: request.requirement,
                sensitivity,
                dependencies,
                platform_availability,
                current_decision,
                requested_decision,
                decision_options,
            });
        }
        Ok(PermissionReviewView {
            principal: principal.clone(),
            title: build.title,
            capabilities,
        })
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
            .map_or(state.next_event_sequence, |item| item.sequence);
        let newest_available = state
            .events
            .back()
            .map_or(state.next_event_sequence, |item| item.sequence);
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
        if !state.installed.contains_key(&build.principal)
            && state.installed.len() >= self.limits.maximum_installed_artifacts
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
        let principal = build.principal.clone();
        state.installed.insert(principal.clone(), build);
        state.artifacts.insert(principal.clone(), artifact);
        self.record_activity(state, &principal, "install", "verified", "completed", now);
        self.push_event(state, PlatformEvent::Installed { principal });
    }

    fn set_library_filter(&self, state: &mut AppState, query: Arc<str>, now: u64) {
        if query.len() > self.limits.maximum_library_query_bytes {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                None,
                None,
                format!(
                    "library query is {} bytes; the maximum is {}",
                    query.len(),
                    self.limits.maximum_library_query_bytes
                ),
                now,
            );
            return;
        }
        if let Err(error) = self
            .store
            .search_installed_builds(&query, self.limits.maximum_installed_artifacts)
        {
            self.refuse_store(state, None, None, error, now);
            return;
        }
        state.library_query = Arc::clone(&query);
        self.push_event(state, PlatformEvent::LibraryFilterChanged { query });
    }

    fn uninstall(
        &self,
        state: &mut AppState,
        principal: Principal,
        cleanup: UninstallCleanupPolicy,
        now: u64,
    ) -> Vec<JoinHandle<()>> {
        if !state.installed.contains_key(&principal) {
            self.refuse(
                state,
                AppErrorCode::NotInstalled,
                Some(principal),
                None,
                "uninstall target is not an installed exact build",
                now,
            );
            return Vec::new();
        }

        let sessions = state
            .sessions
            .iter()
            .filter_map(|(id, entry)| (entry.context.principal == principal).then_some(*id))
            .collect::<Vec<_>>();
        let mut joins = Vec::with_capacity(sessions.len());
        for session in sessions {
            if let Some(join) = self.end_session(state, session, SessionState::Stopped, None, now) {
                joins.push(join);
            }
        }

        let report = match self.store.uninstall_exact_build(&principal, cleanup) {
            Ok(report) => report,
            Err(error) => {
                self.refuse_store(state, Some(principal), None, error, now);
                return joins;
            }
        };

        for domain in self.bridge.advertised_domains() {
            self.bridge.revoke(&principal, &domain);
        }
        state.artifacts.remove(&principal);
        state.installed.remove(&principal);
        for assignments in state.workspace_assignments.values_mut() {
            assignments.remove(&principal);
        }
        self.record_activity(
            state,
            &principal,
            "install",
            "uninstall",
            "runtime-state-removed",
            now,
        );
        self.push_event(
            state,
            PlatformEvent::Uninstalled {
                principal,
                cleanup: report,
            },
        );
        joins
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
        if !state.installed.contains_key(&principal) {
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

    fn apply_permission_batch(
        &self,
        state: &mut AppState,
        principal: Principal,
        decisions: Vec<PermissionDecision>,
        now: u64,
    ) {
        let Some(build) = state.installed.get(&principal) else {
            self.refuse(
                state,
                AppErrorCode::NotInstalled,
                Some(principal),
                None,
                "permission target is not an installed exact build",
                now,
            );
            return;
        };
        let requested = build
            .capability_requests
            .iter()
            .map(|request| (request.capability.clone(), request.requirement))
            .collect::<BTreeMap<_, _>>();
        if decisions.is_empty() || decisions.len() != requested.len() {
            self.refuse(
                state,
                AppErrorCode::Grant,
                Some(principal),
                None,
                "permission batch must contain exactly one decision for every requested capability",
                now,
            );
            return;
        }
        let mut selected = BTreeMap::new();
        for decision in &decisions {
            if decision.capability.as_str() == "shell" {
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
            if !requested.contains_key(&decision.capability) {
                self.refuse(
                    state,
                    AppErrorCode::Grant,
                    Some(principal),
                    None,
                    format!(
                        "permission batch contains unrequested capability {}",
                        decision.capability
                    ),
                    now,
                );
                return;
            }
            if selected
                .insert(decision.capability.clone(), decision.decision)
                .is_some()
            {
                self.refuse(
                    state,
                    AppErrorCode::Grant,
                    Some(principal),
                    None,
                    format!(
                        "permission batch repeats capability {}",
                        decision.capability
                    ),
                    now,
                );
                return;
            }
            if decision.decision == GrantDecision::Managed {
                self.refuse(
                    state,
                    AppErrorCode::Grant,
                    Some(principal),
                    None,
                    "managed decisions may be set only by host policy",
                    now,
                );
                return;
            }
        }
        if selected.keys().ne(requested.keys()) {
            self.refuse(
                state,
                AppErrorCode::Grant,
                Some(principal),
                None,
                "permission batch capability set does not match the installed exact build",
                now,
            );
            return;
        }

        let mut metadata = BTreeMap::new();
        for capability in requested.keys() {
            let descriptor = self.bridge.permission_descriptor(capability);
            if descriptor.is_none()
                && selected
                    .get(capability)
                    .is_some_and(|decision| *decision != GrantDecision::Denied)
            {
                self.refuse(
                    state,
                    AppErrorCode::Grant,
                    Some(principal),
                    None,
                    format!(
                        "capability {capability} has no registered provider metadata; only denial is valid"
                    ),
                    now,
                );
                return;
            }
            if descriptor.as_ref().is_some_and(|descriptor| {
                matches!(
                    descriptor.platform_availability,
                    ProviderPlatformAvailability::Unavailable { .. }
                )
            }) && selected
                .get(capability)
                .is_some_and(|decision| *decision != GrantDecision::Denied)
            {
                self.refuse(
                    state,
                    AppErrorCode::Grant,
                    Some(principal),
                    None,
                    format!(
                        "capability {capability} is unavailable on this platform; only denial is valid"
                    ),
                    now,
                );
                return;
            }
            metadata.insert(capability.clone(), descriptor);
        }

        for (capability, decision) in &selected {
            if !decision.allows_without_prompt() {
                continue;
            }
            let Some(descriptor) = metadata.get(capability).and_then(Option::as_ref) else {
                continue;
            };
            for dependency in &descriptor.dependencies {
                let dependency_decision = match selected.get(dependency).copied() {
                    Some(decision) => decision,
                    None => match self.current_grant_decision(&principal, dependency) {
                        Ok(decision) => decision,
                        Err(error) => {
                            self.refuse_store(state, Some(principal), None, error, now);
                            return;
                        }
                    },
                };
                if !dependency_decision.allows_without_prompt() {
                    self.refuse(
                        state,
                        AppErrorCode::Grant,
                        Some(principal),
                        None,
                        format!("capability {capability} requires allowed dependency {dependency}"),
                        now,
                    );
                    return;
                }
            }
        }

        let mut previous = BTreeMap::new();
        let mut ledger_changes = Vec::with_capacity(decisions.len());
        let mut persistent = Vec::with_capacity(decisions.len());
        for decision in &decisions {
            let current = match self.current_grant_decision(&principal, &decision.capability) {
                Ok(current) => current,
                Err(error) => {
                    self.refuse_store(state, Some(principal), None, error, now);
                    return;
                }
            };
            if current == GrantDecision::Managed {
                self.refuse(
                    state,
                    AppErrorCode::Grant,
                    Some(principal),
                    None,
                    format!(
                        "capability {} is managed by host policy",
                        decision.capability
                    ),
                    now,
                );
                return;
            }
            previous.insert(decision.capability.clone(), current);
            let sensitivity = metadata
                .get(&decision.capability)
                .and_then(Option::as_ref)
                .map_or(Sensitivity::Sensitive, |descriptor| {
                    if descriptor.sensitive {
                        Sensitivity::Sensitive
                    } else {
                        Sensitivity::Ordinary
                    }
                });
            ledger_changes.push((decision.capability.clone(), sensitivity, decision.decision));
            persistent.push((decision.capability.clone(), decision.decision));
        }
        match self
            .grants
            .commit_batch(principal.clone(), &ledger_changes, || {
                self.store.set_grants_atomic(&principal, &persistent)
            }) {
            Ok(()) => {}
            Err(GrantBatchError::Grant(error)) => {
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
            Err(GrantBatchError::Commit(error)) => {
                self.refuse_store(state, Some(principal), None, error, now);
                return;
            }
        }

        for decision in &decisions {
            let prior = previous
                .get(&decision.capability)
                .copied()
                .unwrap_or(GrantDecision::Denied);
            if prior.allows_without_prompt() && !decision.decision.allows_without_prompt() {
                self.bridge
                    .cancel_capability_work(&principal, &decision.capability);
                let operations = state
                    .operations
                    .iter()
                    .filter_map(|(id, operation)| {
                        (operation.principal == principal
                            && operation.domain == decision.capability)
                            .then_some(*id)
                    })
                    .collect::<Vec<_>>();
                for id in operations {
                    if let Some(operation) = state.operations.remove(&id) {
                        operation.cancel(Arc::from("permission revoked"));
                    }
                }
            }
            self.record_activity(
                state,
                &principal,
                "grant",
                decision.capability.as_str(),
                grant_outcome(decision.decision),
                now,
            );
        }
        self.push_event(
            state,
            PlatformEvent::PermissionBatchApplied {
                principal,
                decisions,
            },
        );
    }

    fn current_grant_decision(
        &self,
        principal: &Principal,
        capability: &Capability,
    ) -> Result<GrantDecision, StoreError> {
        match self.grants.decision_entry(principal, capability) {
            Some(decision) => Ok(decision),
            None => self.store.grant(principal, capability),
        }
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
                operation.cancel(Arc::from("session ended"));
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
        if !state.installed.contains_key(&principal) {
            self.refuse(
                state,
                AppErrorCode::NotInstalled,
                Some(principal),
                None,
                "launch target is not an installed exact build",
                now,
            );
            return;
        }
        let Some(artifact) = state.artifacts.get(&principal).cloned() else {
            self.refuse(
                state,
                AppErrorCode::OfflineBytesUnavailable,
                Some(principal),
                None,
                "installed exact-build metadata is restored but sealed artifact bytes are not attached",
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
        let advertised_domains = self.bridge.advertised_domains();
        let (grantable_domains, unavailable_domains): (BTreeSet<Capability>, BTreeSet<Capability>) =
            required_domains
                .into_iter()
                .partition(|domain| advertised_domains.contains(domain));
        if !unavailable_domains.is_empty() {
            let dropped = unavailable_domains
                .iter()
                .map(Capability::as_str)
                .collect::<Vec<_>>()
                .join(",");
            self.record_activity(
                state,
                &principal,
                "capability",
                "required-domain-unavailable",
                &dropped,
                now,
            );
        }
        let plan = match self
            .bridge
            .negotiate(&principal, profile, &grantable_domains)
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
        let Some(next_source_window) = state.next_source_window_id.checked_add(1) else {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                Some(principal),
                None,
                "source-window identifier space is exhausted",
                now,
            );
            return;
        };
        let session_id = SessionId(next);
        let source_window = SourceWindowId(next_source_window);
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
        let push_observer =
            match self
                .bridge
                .open_session_bound(&context, &plan, source_window, now)
            {
                Ok(observer) => observer,
                Err(error) => {
                    self.shell_provider.close_session(session_id);
                    drop(webview);
                    self.refuse_bridge(state, Some(principal), Some(session_id), error, now);
                    return;
                }
            };
        if let Err(error) = session.transition(SessionState::Running) {
            self.shell_provider.close_session(session_id);
            self.bridge
                .close_session_with_reason(session_id, ProviderSessionEnd::OpenFailed);
            drop(webview);
            self.refuse_session(state, Some(principal), Some(session_id), error, now);
            return;
        }
        state.next_session_id = next;
        state.next_source_window_id = next_source_window;
        state.sessions.insert(
            session_id,
            SessionEntry {
                session: Arc::clone(&session),
                context,
                plan,
                source_window,
                push_observer: Some(push_observer),
                push_delivery: None,
                ready: false,
                last_provider_sequence: None,
                delivered_push_count: 0,
                _artifact: artifact,
                _webview: webview,
            },
        );
        self.record_activity(state, &principal, "session", "launch", "running", now);
        self.push_event(state, PlatformEvent::SessionChanged(session.snapshot()));
    }

    fn transition_session(
        &self,
        state: &mut AppState,
        session_id: SessionId,
        next: SessionState,
        now: u64,
    ) {
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
        let session = Arc::clone(&entry.session);
        if let Err(error) = session.transition(next) {
            self.refuse_session(state, Some(principal), Some(session_id), error, now);
            return;
        }
        let (operation, outcome) = match next {
            SessionState::Suspended => ("suspend", "suspended"),
            SessionState::Running => ("resume", "running"),
            _ => ("transition", "completed"),
        };
        self.record_activity(
            state,
            session.principal(),
            "session",
            operation,
            outcome,
            now,
        );
        self.push_event(state, PlatformEvent::SessionChanged(session.snapshot()));
    }

    fn end_session(
        &self,
        state: &mut AppState,
        session_id: SessionId,
        terminal: SessionState,
        reason: Option<Arc<str>>,
        now: u64,
    ) -> Option<JoinHandle<()>> {
        let Some(mut entry) = state.sessions.remove(&session_id) else {
            self.refuse(
                state,
                AppErrorCode::UnknownSession,
                None,
                Some(session_id),
                "stale or unknown session",
                now,
            );
            return None;
        };
        let operation_ids = state
            .operations
            .iter()
            .filter_map(|(id, operation)| (operation.session == session_id).then_some(*id))
            .collect::<Vec<_>>();
        for operation_id in operation_ids {
            if let Some(operation) = state.operations.remove(&operation_id) {
                operation.cancel(Arc::from("provider operation cancelled"));
            }
        }
        self.shell_provider.close_session(session_id);
        let provider_reason = match terminal {
            SessionState::Crashed => ProviderSessionEnd::Crashed,
            _ => ProviderSessionEnd::Stopped,
        };
        self.bridge
            .close_session_with_reason(session_id, provider_reason);
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
        let delivery_join = entry
            .push_delivery
            .take()
            .and_then(|mut delivery| delivery.join.take());
        drop(entry);
        self.push_event(state, PlatformEvent::SessionChanged(snapshot));
        delivery_join
    }

    fn dispatch_envelope(
        self: &Arc<Self>,
        state: &mut AppState,
        session_id: SessionId,
        bytes: &[u8],
        now: u64,
    ) -> Option<JoinHandle<()>> {
        if bytes.len() > self.limits.maximum_envelope_bytes {
            self.refuse(
                state,
                AppErrorCode::Capacity,
                None,
                Some(session_id),
                "mapped envelope exceeds the application bound",
                now,
            );
            return None;
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
            return None;
        };
        if entry.session.state() != SessionState::Running {
            let principal = entry.context.principal.clone();
            self.refuse(
                state,
                AppErrorCode::InvalidLifecycle,
                Some(principal),
                Some(session_id),
                "mapped envelopes are refused while the session is suspended",
                now,
            );
            return None;
        }
        let principal = entry.context.principal.clone();
        let context = entry.context.clone();
        let plan = entry.plan.clone();
        let route = envelope_route(bytes);
        let domain = route.as_ref().map(|(domain, _)| domain.clone());
        let is_shell_ready = route
            .as_ref()
            .is_some_and(|(domain, action)| domain.as_str() == "shell" && action == "ready");
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
            return None;
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
            return None;
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
                    self.refuse(
                        state,
                        AppErrorCode::Bridge,
                        Some(principal),
                        Some(session_id),
                        "shell.init capability set does not match the fixed session plan",
                        now,
                    );
                    return self.end_session(
                        state,
                        session_id,
                        SessionState::Crashed,
                        Some(Arc::from("invalid shell.init")),
                        now,
                    );
                }
                if is_shell_ready
                    && let Err(detail) = self.activate_push_delivery(state, session_id)
                {
                    self.refuse(
                        state,
                        AppErrorCode::Bridge,
                        Some(principal),
                        Some(session_id),
                        detail,
                        now,
                    );
                    return self.end_session(
                        state,
                        session_id,
                        SessionState::Crashed,
                        Some(Arc::from("provider delivery activation failed")),
                        now,
                    );
                }
                let mut handle = call.take_operation();
                let mut proposal = call.take_write_proposal();
                if handle.is_some() && proposal.is_some() {
                    if let Some(proposal) = proposal.take() {
                        proposal.refuse(Arc::from(
                            "provider returned both a streaming operation and a write proposal",
                        ));
                    }
                    if let Some(handle) = handle.take() {
                        handle.cancel();
                    }
                    self.refuse(
                        state,
                        AppErrorCode::Bridge,
                        Some(principal.clone()),
                        Some(session_id),
                        "provider returned conflicting operation ownership",
                        now,
                    );
                    return None;
                }
                let operation = if handle.is_some() || proposal.is_some() {
                    if state.operations.len() >= self.limits.maximum_provider_operations {
                        if let Some(proposal) = proposal.take() {
                            proposal.refuse(Arc::from("provider operation capacity is full"));
                        }
                        if let Some(handle) = handle.take() {
                            handle.cancel();
                        }
                        self.refuse(
                            state,
                            AppErrorCode::Capacity,
                            Some(principal),
                            Some(session_id),
                            "provider operation ownership capacity is full",
                            now,
                        );
                        return None;
                    }
                    let Some(next) = state.next_operation_id.checked_add(1) else {
                        if let Some(proposal) = proposal.take() {
                            proposal.refuse(Arc::from(
                                "provider operation identifier space is exhausted",
                            ));
                        }
                        if let Some(handle) = handle.take() {
                            handle.cancel();
                        }
                        self.refuse(
                            state,
                            AppErrorCode::Capacity,
                            Some(principal),
                            Some(session_id),
                            "provider operation identifier space is exhausted",
                            now,
                        );
                        return None;
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
                            proposal,
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
        None
    }

    fn activate_push_delivery(
        self: &Arc<Self>,
        state: &mut AppState,
        session_id: SessionId,
    ) -> Result<(), Arc<str>> {
        let Some(entry) = state.sessions.get(&session_id) else {
            return Err(Arc::from("provider delivery session is no longer active"));
        };
        if entry.ready {
            return Ok(());
        }
        self.bridge
            .mark_session_ready(session_id)
            .map_err(|error| Arc::from(error.to_string()))?;
        let delivery_lease = self
            .resources
            .admit(session_id, None, ResourceClass::StateDelivery)
            .map_err(|error| Arc::from(error.to_string()))?;
        let entry = state
            .sessions
            .get_mut(&session_id)
            .expect("session was validated while holding the app lock");
        let observer = entry
            .push_observer
            .take()
            .ok_or_else(|| Arc::from("provider delivery observer is unavailable"))?;
        let source_window = entry.source_window;
        entry.ready = true;
        let app = Arc::downgrade(self);
        let maximum_batch = self.limits.maximum_provider_push_batch;
        let join = thread::Builder::new()
            .name(format!("nap-push-{}", session_id.0))
            .spawn(move || {
                run_provider_push_delivery(
                    app,
                    observer,
                    delivery_lease,
                    session_id,
                    source_window,
                    maximum_batch,
                );
            })
            .map_err(|error| Arc::from(error.to_string()))?;
        entry.push_delivery = Some(ProviderPushDelivery { join: Some(join) });
        Ok(())
    }

    fn ingest_provider_push_batch(
        &self,
        session_id: SessionId,
        source_window: SourceWindowId,
        batch: ProviderPushBatch,
    ) -> bool {
        let now = self.clock.now_millis();
        let mut state = self.state.lock();
        let Some(entry) = state.sessions.get_mut(&session_id) else {
            return false;
        };
        if entry.source_window != source_window || !entry.ready {
            let principal = entry.context.principal.clone();
            self.refuse(
                &mut state,
                AppErrorCode::SessionIdentityMismatch,
                Some(principal),
                Some(session_id),
                "provider push source no longer matches the ready mapped session",
                now,
            );
            let _ = self.end_session(
                &mut state,
                session_id,
                SessionState::Crashed,
                Some(Arc::from("provider push source mismatch")),
                now,
            );
            self.publish(&mut state);
            return false;
        }

        let principal = entry.context.principal.clone();
        let domains = entry.plan.domains().clone();
        let mut accepted = Vec::with_capacity(batch.pushes.len());
        let mut invalid = None;
        for push in batch.pushes {
            if push.session != session_id
                || push.source_window != source_window
                || !domains.contains(&push.domain)
                || entry
                    .last_provider_sequence
                    .is_some_and(|sequence| push.sequence <= sequence)
            {
                invalid =
                    Some("provider push violated its fixed session, source, domain, or sequence");
                break;
            }
            if push.domain.as_str() != "shell"
                && !self
                    .grants
                    .decision(&principal, &push.domain)
                    .allows_without_prompt()
            {
                continue;
            }
            entry.last_provider_sequence = Some(push.sequence);
            entry.delivered_push_count = entry.delivered_push_count.saturating_add(1);
            accepted.push(push);
        }
        let closed = batch.closed;
        let termination = batch.termination;
        for push in accepted {
            self.project_provider_push(&mut state, push);
        }
        if let Some(detail) = invalid {
            self.refuse(
                &mut state,
                AppErrorCode::SessionIdentityMismatch,
                Some(principal),
                Some(session_id),
                detail,
                now,
            );
            self.push_event(
                &mut state,
                PlatformEvent::ProviderPushLaneClosed {
                    session: session_id,
                    source_window,
                    termination: Some(ProviderPushTermination::ProviderFailure),
                },
            );
            let _ = self.end_session(
                &mut state,
                session_id,
                SessionState::Crashed,
                Some(Arc::from("invalid provider push routing")),
                now,
            );
            self.publish(&mut state);
            return false;
        }
        if closed {
            self.push_event(
                &mut state,
                PlatformEvent::ProviderPushLaneClosed {
                    session: session_id,
                    source_window,
                    termination,
                },
            );
            let reason = match termination {
                Some(ProviderPushTermination::Backpressure) => {
                    "provider push lane terminated by backpressure"
                }
                Some(ProviderPushTermination::ProviderFailure) => {
                    "provider push lane terminated by provider failure"
                }
                None => "provider push lane closed unexpectedly",
            };
            let _ = self.end_session(
                &mut state,
                session_id,
                SessionState::Crashed,
                Some(Arc::from(reason)),
                now,
            );
            self.publish(&mut state);
            return false;
        }
        self.publish(&mut state);
        true
    }

    fn provider_push_observation_failed(
        &self,
        session_id: SessionId,
        source_window: SourceWindowId,
        error: ProviderPushError,
    ) {
        let now = self.clock.now_millis();
        let mut state = self.state.lock();
        let Some(entry) = state.sessions.get(&session_id) else {
            return;
        };
        if entry.source_window != source_window {
            return;
        }
        let principal = entry.context.principal.clone();
        self.refuse(
            &mut state,
            AppErrorCode::Bridge,
            Some(principal),
            Some(session_id),
            error.to_string(),
            now,
        );
        self.push_event(
            &mut state,
            PlatformEvent::ProviderPushLaneClosed {
                session: session_id,
                source_window,
                termination: Some(ProviderPushTermination::ProviderFailure),
            },
        );
        let _ = self.end_session(
            &mut state,
            session_id,
            SessionState::Crashed,
            Some(Arc::from("provider push observation failed")),
            now,
        );
        self.publish(&mut state);
    }

    fn project_provider_push(&self, state: &mut AppState, push: ProviderPush) {
        self.push_event(
            state,
            PlatformEvent::ProviderPush {
                session: push.session,
                source_window: push.source_window,
                provider_sequence: push.sequence,
                domain: push.domain,
                envelope: push.envelope,
            },
        );
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
        if operation.proposal.is_some() {
            operation.cancel(Arc::from("pending write requires an approval decision"));
            self.refuse(
                state,
                AppErrorCode::Bridge,
                None,
                None,
                "a pending provider write cannot be completed without approval",
                now,
            );
            return;
        }
        operation.complete();
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
        self.accept_approved_write(state, write, None, now);
    }

    fn decide_provider_write(
        &self,
        state: &mut AppState,
        operation_id: ProviderOperationId,
        approve: bool,
        now: u64,
    ) {
        let Some(mut operation) = state.operations.remove(&operation_id) else {
            self.refuse(
                state,
                AppErrorCode::Bridge,
                None,
                None,
                "unknown provider write proposal",
                now,
            );
            return;
        };
        let Some(proposal) = operation.proposal.take() else {
            let principal = operation.principal.clone();
            let session = operation.session;
            operation.complete();
            self.refuse(
                state,
                AppErrorCode::Bridge,
                Some(principal),
                Some(session),
                "provider operation is not awaiting a write decision",
                now,
            );
            return;
        };
        if !approve {
            proposal.refuse(Arc::from("native approval was denied"));
            if let Some(handle) = operation.handle {
                handle.cancel();
            }
            self.push_event(
                state,
                PlatformEvent::ProviderOperationFinished {
                    operation: operation_id,
                },
            );
            return;
        }
        let (write, completion, work) = proposal.into_parts();
        let provider_sink = completion.into_receipt_sink();
        self.accept_approved_write(state, write, Some(provider_sink), now);
        drop(work);
        self.push_event(
            state,
            PlatformEvent::ProviderOperationFinished {
                operation: operation_id,
            },
        );
    }

    fn accept_approved_write(
        &self,
        state: &mut AppState,
        write: ApprovedWrite,
        provider_sink: Option<Arc<dyn ReceiptEventSink>>,
        now: u64,
    ) {
        let Some(session) = state.sessions.get(&write.origin_session) else {
            if let Some(sink) = provider_sink.as_ref() {
                sink.close(Some(Arc::from("origin session is no longer active")));
            }
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
            if let Some(sink) = provider_sink.as_ref() {
                sink.close(Some(Arc::from("origin session identity changed")));
            }
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
            if let Some(sink) = provider_sink.as_ref() {
                sink.close(Some(Arc::from("receipt ownership capacity is full")));
            }
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
        let receipt_sink: Arc<dyn ReceiptEventSink> = match provider_sink {
            Some(provider) => Arc::new(ReceiptFanout {
                app: Arc::clone(&receipt),
                provider,
            }),
            None => receipt.clone(),
        };
        let accepted = match self.data_plane.accept_write(write, receipt_sink.clone()) {
            Ok(accepted) => accepted,
            Err(error) => {
                receipt_sink.close(Some(Arc::from(error.to_string())));
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
            receipt_sink.close(Some(Arc::clone(&detail)));
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
            receipt_sink.close(Some(Arc::from(
                "host data plane returned a different frozen account",
            )));
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

    fn assign_workspace_build(
        &self,
        state: &mut AppState,
        workspace_id: Arc<str>,
        principal: Principal,
        assigned: bool,
        now: u64,
    ) {
        let result = if assigned {
            self.store
                .assign_build_to_workspace(&workspace_id, &principal)
                .map(|()| true)
        } else {
            self.store
                .remove_build_from_workspace(&workspace_id, &principal)
        };
        let changed = match result {
            Ok(changed) => changed,
            Err(error) => {
                self.refuse_store(state, Some(principal), None, error, now);
                return;
            }
        };
        let assignments = state
            .workspace_assignments
            .entry(Arc::clone(&workspace_id))
            .or_default();
        if assigned {
            assignments.insert(principal.clone());
        } else {
            assignments.remove(&principal);
        }
        if changed || assigned {
            self.push_event(
                state,
                PlatformEvent::WorkspaceAssignmentChanged {
                    workspace_id,
                    principal,
                    assigned,
                },
            );
        }
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
            let assignments = match self.store.workspace_assignments(&workspace_id) {
                Ok(assignments) => assignments,
                Err(error) => {
                    self.refuse_store(state, None, None, error, now);
                    continue;
                }
            };
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
            state
                .workspace_assignments
                .insert(workspace_id.clone(), assignments.into_iter().collect());
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

    fn close(&self, state: &mut AppState, now: u64) -> Vec<JoinHandle<()>> {
        if state.closed {
            return Vec::new();
        }
        let mut delivery_joins = Vec::new();
        let sessions = state.sessions.keys().copied().collect::<Vec<_>>();
        for session in sessions {
            self.bridge
                .close_session_with_reason(session, ProviderSessionEnd::RuntimeClosed);
            if let Some(join) = self.end_session(state, session, SessionState::Stopped, None, now) {
                delivery_joins.push(join);
            }
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
        delivery_joins
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
            library: installed_library_view(
                &state.installed,
                &state.artifacts,
                &state.sessions,
                &state.library_query,
            ),
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
            provider_push_lanes: state
                .sessions
                .iter()
                .map(|(session, entry)| ProviderPushLaneView {
                    session: *session,
                    source_window: entry.source_window,
                    ready: entry.ready,
                    last_provider_sequence: entry.last_provider_sequence,
                    delivered_count: entry.delivered_push_count,
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
            pending_writes: state
                .operations
                .iter()
                .filter_map(|(operation, active)| {
                    let proposal = active.proposal.as_ref()?;
                    let write = proposal.write.as_ref()?;
                    Some(ProviderWriteProposalView {
                        operation: *operation,
                        approval_id: Arc::clone(&write.approval_id),
                        principal: write.origin_principal.clone(),
                        session: write.origin_session,
                        account: write.account.clone(),
                        draft: write.draft.clone(),
                    })
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
                    assigned_builds: state
                        .workspace_assignments
                        .get(&workspace.id)
                        .map(|assignments| assignments.iter().cloned().collect())
                        .unwrap_or_default(),
                })
                .collect(),
            resources: self.resources.census(),
            recent_activity: state.activity.iter().cloned().collect(),
            recent_errors: state.errors.iter().cloned().collect(),
        }
    }

    fn push_event(&self, state: &mut AppState, event: PlatformEvent) {
        state.next_event_sequence = state.next_event_sequence.saturating_add(1);
        let sequence = state.next_event_sequence;
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
        let mut delivery_joins = Vec::new();
        for (session_id, mut entry) in std::mem::take(&mut state.sessions) {
            self.shell_provider.close_session(session_id);
            self.bridge
                .close_session_with_reason(session_id, ProviderSessionEnd::RuntimeClosed);
            entry.session.stop();
            if let Some(join) = entry
                .push_delivery
                .take()
                .and_then(|mut delivery| delivery.join.take())
            {
                delivery_joins.push(join);
            }
        }
        for join in delivery_joins {
            let _ = join.join();
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

fn run_provider_push_delivery(
    app: Weak<RuntimeApp>,
    mut observer: ProviderPushObserver,
    delivery_lease: WorkLease,
    session_id: SessionId,
    source_window: SourceWindowId,
    maximum_batch: usize,
) {
    let mut delivery_lease = Some(delivery_lease);
    let runtime = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            drop(delivery_lease.take());
            if let Some(app) = app.upgrade() {
                app.provider_push_observation_failed(
                    session_id,
                    source_window,
                    ProviderPushError::Malformed(Arc::from(error.to_string())),
                );
            }
            return;
        }
    };
    runtime.block_on(async {
        loop {
            match observer.changed(maximum_batch).await {
                Ok(batch) => {
                    if batch.closed {
                        drop(delivery_lease.take());
                    }
                    let Some(app) = app.upgrade() else {
                        break;
                    };
                    let closed = batch.closed;
                    if !app.ingest_provider_push_batch(session_id, source_window, batch) || closed {
                        break;
                    }
                }
                Err(ProviderPushError::Closed) => break,
                Err(error) => {
                    drop(delivery_lease.take());
                    if let Some(app) = app.upgrade() {
                        app.provider_push_observation_failed(session_id, source_window, error);
                    }
                    break;
                }
            }
        }
    });
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

/// Keeps the runtime-owned receipt projection authoritative while forwarding
/// the same NMP receipt frame to the provider's bounded protocol response.
/// Provider-lane closure never weakens the app-owned durable receipt view.
#[derive(Debug)]
struct ReceiptFanout {
    app: Arc<AppReceipt>,
    provider: Arc<dyn ReceiptEventSink>,
}

impl ReceiptEventSink for ReceiptFanout {
    fn push_latest(&self, snapshot: ReceiptSnapshot) -> Result<(), ReceiptSinkError> {
        self.app.push_latest(snapshot.clone())?;
        let _ = self.provider.push_latest(snapshot);
        Ok(())
    }

    fn close(&self, reason: Option<Arc<str>>) {
        self.app.close(reason.clone());
        self.provider.close(reason);
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

fn installed_library_view(
    installed: &BTreeMap<Principal, InstalledBuild>,
    artifacts: &BTreeMap<Principal, Arc<dyn ExecutableArtifact>>,
    sessions: &BTreeMap<SessionId, SessionEntry>,
    query: &str,
) -> InstalledLibraryView {
    let builds = installed
        .values()
        .filter(|build| {
            query.is_empty()
                || [
                    build.title.as_ref(),
                    build.principal.manifest_author(),
                    build.principal.d_tag(),
                    build.principal.aggregate_hash(),
                ]
                .iter()
                .any(|value| contains_library_search(value, query))
        })
        .map(|build| InstalledBuildView {
            build: build.clone(),
            availability: if artifacts.contains_key(&build.principal) {
                InstalledBuildAvailability::SealedExactBytesReady
            } else {
                InstalledBuildAvailability::MetadataOnly
            },
            active_sessions: sessions
                .iter()
                .filter_map(|(id, entry)| {
                    (entry.context.principal == build.principal).then_some(*id)
                })
                .collect(),
        })
        .collect();
    InstalledLibraryView {
        query: Arc::from(query),
        total_installed: installed.len(),
        builds,
    }
}

fn contains_library_search(value: &str, query: &str) -> bool {
    query.is_empty()
        || value
            .as_bytes()
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query.as_bytes()))
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

fn permission_provider_projection(
    descriptor: Option<ProviderDescriptor>,
) -> (
    Option<Sensitivity>,
    Vec<Capability>,
    PermissionPlatformAvailability,
) {
    match descriptor {
        Some(descriptor) => {
            let sensitivity = Some(if descriptor.sensitive {
                Sensitivity::Sensitive
            } else {
                Sensitivity::Ordinary
            });
            let dependencies = descriptor.dependencies.into_iter().collect();
            let availability = match descriptor.platform_availability {
                ProviderPlatformAvailability::Available => {
                    PermissionPlatformAvailability::Available
                }
                ProviderPlatformAvailability::Unavailable { reason } => {
                    PermissionPlatformAvailability::Unavailable { reason }
                }
            };
            (sensitivity, dependencies, availability)
        }
        None => (
            None,
            Vec::new(),
            PermissionPlatformAvailability::Unknown {
                reason: Arc::from(
                    "no provider metadata is registered for this capability on this runtime",
                ),
            },
        ),
    }
}

fn permission_decision_policy(
    current: GrantDecision,
    availability: &PermissionPlatformAvailability,
) -> (Option<GrantDecision>, Vec<PermissionDecisionOption>) {
    let user_decisions = [
        GrantDecision::Denied,
        GrantDecision::AskEveryTime,
        GrantDecision::AllowSession,
        GrantDecision::AllowExactBuild,
    ];
    if current == GrantDecision::Managed {
        let reason: Arc<str> = Arc::from("this capability is managed by host policy");
        return (
            None,
            user_decisions
                .into_iter()
                .map(|decision| PermissionDecisionOption {
                    decision,
                    valid: false,
                    invalid_reason: Some(Arc::clone(&reason)),
                })
                .collect(),
        );
    }
    let unavailable_reason = match availability {
        PermissionPlatformAvailability::Available => None,
        PermissionPlatformAvailability::Unknown { reason }
        | PermissionPlatformAvailability::Unavailable { reason } => Some(Arc::clone(reason)),
    };
    let requested = if unavailable_reason.is_some() {
        GrantDecision::Denied
    } else if current == GrantDecision::Denied {
        GrantDecision::AskEveryTime
    } else {
        current
    };
    (
        Some(requested),
        user_decisions
            .into_iter()
            .map(|decision| {
                let invalid_reason = (decision != GrantDecision::Denied)
                    .then(|| unavailable_reason.clone())
                    .flatten();
                PermissionDecisionOption {
                    decision,
                    valid: invalid_reason.is_none(),
                    invalid_reason,
                }
            })
            .collect(),
    )
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
