//! UniFFI projection for the native napplet runtime.
//!
//! This crate is the only unsafe/native ABI boundary in the runtime
//! workspace.  It exports sealed verified-artifact handles and one
//! Rust-owned controller; native callers cannot construct principals or
//! smuggle session authority through napplet envelopes.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    io::Cursor,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::Engine;
use nmp::EngineConfig;
use nmp_native_artifact::{
    ArtifactLimits, ArtifactMode, ArtifactSourcePolicy, BlobFetchRequest, BlobFetchResponse,
    BlobSourceError, FileArtifactCache, ManifestBlobSource, ManifestCoordinate, ManifestError,
    ManifestEventLimits, ManifestEventVerifier, SignedArtifactResolver, VerifiedArtifactHandle,
    reopen_verified_artifact,
};
use nmp_native_nap_bridge::{BridgeLimits, Provider};
use nmp_native_nmp_adapter::{
    AccountLifecycleError, LocalAccountHandle, LocalAccountKind, LocalAccountSnapshot,
    NapNostrProviderLimits, NapNostrProviderSet, NmpDataPlane,
};
use nmp_native_provider_identity::{
    IdentityDataPlane, IdentityProvider, IdentityProviderLimits, NoopIdentityDiagnostics,
};
use nmp_native_provider_inc::{
    AllowAllIncAcl, IncNativeAction, IncNativeActionKind, IncNativeActionOrigin,
    IncNativeActionSessionEnd, IncNativeActionSink, IncNativeActionSinkError, IncProvider,
    IncProviderLimits, NoopIncActivity,
};
use nmp_native_provider_link::{
    CancelIntentChoice, IntentHandlerDeclaration, IntentProvider, IntentProviderLimits,
    NativeIntentDispatcher, NoopIntentActivity,
};
use nmp_native_providers::{
    ConfigProvider, ConfigProviderLimits, ProviderPushReport,
    SettingsExecutor as ProviderSettingsExecutor, SettingsExecutorError,
    SettingsRequest as ProviderSettingsRequest, ShellEnvironment, ShellEnvironmentError,
    ShellEnvironmentLimits, ShellEnvironmentSource, ShellProvider, ShellProviderLimits,
    StorageProvider, StorageProviderLimits, ThemeProvider, ThemeProviderLimits, ThemeSnapshot,
    ThemeSource,
};
use nmp_native_runtime_app::{
    AppLimits, AppSnapshot, ExecutableArtifact, InstalledBuildAvailability, KernelClock,
    PermissionDecision, PermissionPlatformAvailability, PermissionReviewView, PlatformCommand,
    PlatformEvent, ProviderOperationId, ReceiptDeliveryState, RuntimeApp, RuntimeAppConfig,
    WorkspaceView,
};
use nmp_native_runtime_core::{
    BoundedJson, Capability, CapabilityRequest, CapabilityRequirement, ExecutionProfile,
    GrantDecision, GrantLimits, Principal, ResourceLimits, Sensitivity, SessionId, SessionState,
    WriteReceiptId,
};
use nmp_native_runtime_store::{
    InstalledBuild, RuntimeStore, StoreLimits, UninstallCleanupPolicy, WorkspaceRecord,
};
use nmp_native_surface::BindingLimits;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

mod catalog;
mod diagnostics;
mod ffi_account;
mod ffi_catalog;
mod ffi_install;
mod ffi_permission;
mod ffi_provider;
mod ffi_session;
mod ffi_snapshot;
mod ffi_workspace;
mod intent_dispatch;
mod projection;
mod workspace_storage;

use projection::*;
use workspace_storage::*;

use intent_dispatch::{DefaultOnlyIntentPolicy, IntentActivationSink, RuntimeIntentDispatcher};

pub use catalog::{
    RuntimeCatalogCancellationResult, RuntimeCatalogCapability, RuntimeCatalogConfirmation,
    RuntimeCatalogConfirmationResult, RuntimeCatalogEntry, RuntimeCatalogFailure,
    RuntimeCatalogFeedSnapshot, RuntimeCatalogLookupState, RuntimeCatalogPage,
    RuntimeCatalogPageResult, RuntimeCatalogProvenance, RuntimeCatalogReview,
    RuntimeCatalogReviewResult, RuntimeCatalogShortfall, RuntimeCatalogSource,
    RuntimeCatalogSourceAccess, RuntimeCatalogSourceState, RuntimeCatalogWindowState,
};
use catalog::{RuntimeCatalogService, project_catalog_error};
use diagnostics::RuntimeDiagnosticsService;
pub use diagnostics::{
    RuntimeRelayAccess, RuntimeRelayCoverage, RuntimeRelayDiagnostics,
    RuntimeRelayDiagnosticsObservation, RuntimeRelayDiagnosticsObservationStart,
    RuntimeRelayDiagnosticsObserver, RuntimeRelayDiagnosticsSnapshot, RuntimeRelayKindCount,
    RuntimeRelayLane, RuntimeRelayLaneCount, RuntimeRelaySubscription,
};

const DEFAULT_MAXIMUM_CONFIG_STRING_BYTES: u64 = 16 * 1_024;
const DEFAULT_MAXIMUM_CONFIG_ITEMS: u64 = 64;
const DEFAULT_MAXIMUM_MANIFEST_BYTES: u64 = 256 * 1_024;
// Must fit the base64 of a `DEFAULT_MAXIMUM_MANIFEST_BYTES` signed event
// (~1.34x inflation) plus the small surrounding metadata fields, and stay
// under `runtime-store`'s own `StoreLimits::maximum_value_bytes` (512 KiB
// default) so a large-but-valid manifest never fails at the store instead
// of here with a clear reason.
const MAXIMUM_INSTALLED_MANIFEST_METADATA_BYTES: usize = 512 * 1_024;
const DEFAULT_MAXIMUM_ARTIFACT_READ_BYTES: u64 = 8 * 1_024 * 1_024;
const DEFAULT_MAXIMUM_OBSERVERS: u64 = 8;
const DEFAULT_MAXIMUM_BOUNDARY_EVENTS: u64 = 256;
const WORKSPACE_SCHEMA_VERSION: u16 = 1;
const MAXIMUM_WORKSPACE_SLOTS: usize = 16;
const MAXIMUM_WORKSPACE_JSON_BYTES: usize = 512 * 1_024;
const MAXIMUM_WORKSPACE_FIELD_BYTES: usize = 64 * 1_024;
const MAXIMUM_WORKSPACE_RECEIPTS: usize = 256;
const MAXIMUM_WORKSPACE_POINT_SIZE: u16 = 4_096;
const MAXIMUM_PERMISSION_DECISIONS: usize = 64;

uniffi::setup_scaffolding!();

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeConfig {
    pub runtime_store_path: String,
    pub nmp_store_path: Option<String>,
    pub artifact_cache_path: String,
    pub indexer_relays: Vec<String>,
    pub app_relays: Vec<String>,
    pub fallback_relays: Vec<String>,
    pub allowed_local_relay_hosts: Vec<String>,
    pub maximum_nmp_relays: u64,
    pub maximum_bridge_workers: u64,
    pub maximum_observers: u64,
    pub maximum_boundary_events: u64,
    pub maximum_config_items: u64,
    pub maximum_config_string_bytes: u64,
    pub maximum_manifest_bytes: u64,
    pub maximum_artifact_files: u64,
    pub maximum_artifact_file_bytes: u64,
    pub maximum_artifact_total_bytes: u64,
    pub maximum_verified_read_bytes: u64,
    pub maximum_blob_sources: u64,
}

impl RuntimeConfig {
    fn validated(self) -> Result<ValidatedConfig, RuntimeOpenError> {
        let maximum_config_items =
            nonzero_usize(self.maximum_config_items, "maximum_config_items")?;
        let maximum_config_string_bytes = nonzero_usize(
            self.maximum_config_string_bytes,
            "maximum_config_string_bytes",
        )?;
        let maximum_observers = nonzero_usize(self.maximum_observers, "maximum_observers")?;
        let maximum_boundary_events =
            nonzero_usize(self.maximum_boundary_events, "maximum_boundary_events")?;
        let maximum_manifest_bytes =
            nonzero_usize(self.maximum_manifest_bytes, "maximum_manifest_bytes")?;
        let maximum_verified_read_bytes = nonzero_usize(
            self.maximum_verified_read_bytes,
            "maximum_verified_read_bytes",
        )?;
        let maximum_bridge_workers =
            nonzero_usize(self.maximum_bridge_workers, "maximum_bridge_workers")?;
        let maximum_blob_sources =
            nonzero_usize(self.maximum_blob_sources, "maximum_blob_sources")?;
        let artifact_limits = ArtifactLimits {
            maximum_files: nonzero_usize(self.maximum_artifact_files, "maximum_artifact_files")?,
            maximum_file_bytes: nonzero_usize(
                self.maximum_artifact_file_bytes,
                "maximum_artifact_file_bytes",
            )?,
            maximum_total_bytes: nonzero_usize(
                self.maximum_artifact_total_bytes,
                "maximum_artifact_total_bytes",
            )?,
        };
        let maximum_nmp_relays = nonzero_usize(self.maximum_nmp_relays, "maximum_nmp_relays")?;
        validate_string(
            "runtime_store_path",
            &self.runtime_store_path,
            maximum_config_string_bytes,
        )?;
        validate_string(
            "artifact_cache_path",
            &self.artifact_cache_path,
            maximum_config_string_bytes,
        )?;
        if let Some(path) = &self.nmp_store_path {
            validate_string("nmp_store_path", path, maximum_config_string_bytes)?;
        }
        for (name, values) in [
            ("indexer_relays", &self.indexer_relays),
            ("app_relays", &self.app_relays),
            ("fallback_relays", &self.fallback_relays),
            ("allowed_local_relay_hosts", &self.allowed_local_relay_hosts),
        ] {
            if values.len() > maximum_config_items {
                return Err(RuntimeOpenError::InvalidConfig {
                    detail: format!(
                        "{name} has {} items; the configured maximum is {maximum_config_items}",
                        values.len()
                    ),
                });
            }
            for value in values {
                validate_string(name, value, maximum_config_string_bytes)?;
            }
        }

        Ok(ValidatedConfig {
            runtime_store_path: self.runtime_store_path,
            nmp_store_path: self.nmp_store_path,
            artifact_cache_path: self.artifact_cache_path,
            indexer_relays: self.indexer_relays,
            app_relays: self.app_relays,
            fallback_relays: self.fallback_relays,
            allowed_local_relay_hosts: self.allowed_local_relay_hosts,
            maximum_nmp_relays,
            maximum_bridge_workers,
            maximum_observers,
            maximum_boundary_events,
            maximum_manifest_bytes,
            artifact_limits,
            maximum_verified_read_bytes,
            maximum_blob_sources,
            maximum_command_items: maximum_config_items,
            maximum_command_string_bytes: maximum_config_string_bytes,
        })
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            runtime_store_path: "runtime.sqlite3".to_owned(),
            nmp_store_path: Some("nmp.redb".to_owned()),
            artifact_cache_path: "artifacts".to_owned(),
            indexer_relays: Vec::new(),
            app_relays: Vec::new(),
            fallback_relays: Vec::new(),
            allowed_local_relay_hosts: Vec::new(),
            maximum_nmp_relays: 64,
            maximum_bridge_workers: 12,
            maximum_observers: DEFAULT_MAXIMUM_OBSERVERS,
            maximum_boundary_events: DEFAULT_MAXIMUM_BOUNDARY_EVENTS,
            maximum_config_items: DEFAULT_MAXIMUM_CONFIG_ITEMS,
            maximum_config_string_bytes: DEFAULT_MAXIMUM_CONFIG_STRING_BYTES,
            maximum_manifest_bytes: DEFAULT_MAXIMUM_MANIFEST_BYTES,
            maximum_artifact_files: 256,
            maximum_artifact_file_bytes: DEFAULT_MAXIMUM_ARTIFACT_READ_BYTES,
            maximum_artifact_total_bytes: 32 * 1_024 * 1_024,
            maximum_verified_read_bytes: DEFAULT_MAXIMUM_ARTIFACT_READ_BYTES,
            maximum_blob_sources: 8,
        }
    }
}

#[derive(Debug)]
struct ValidatedConfig {
    runtime_store_path: String,
    nmp_store_path: Option<String>,
    artifact_cache_path: String,
    indexer_relays: Vec<String>,
    app_relays: Vec<String>,
    fallback_relays: Vec<String>,
    allowed_local_relay_hosts: Vec<String>,
    maximum_nmp_relays: usize,
    maximum_bridge_workers: usize,
    maximum_observers: usize,
    maximum_boundary_events: usize,
    maximum_manifest_bytes: usize,
    artifact_limits: ArtifactLimits,
    maximum_verified_read_bytes: usize,
    maximum_blob_sources: usize,
    maximum_command_items: usize,
    maximum_command_string_bytes: usize,
}

#[derive(Clone, Debug, thiserror::Error, uniffi::Error)]
pub enum RuntimeOpenError {
    #[error("invalid runtime configuration: {detail}")]
    InvalidConfig { detail: String },
    #[error("runtime storage could not be opened: {detail}")]
    RuntimeStore { detail: String },
    #[error("artifact cache could not be opened: {detail}")]
    ArtifactCache { detail: String },
    #[error("NMP data plane could not be opened: {detail}")]
    Nmp { detail: String },
    #[error("runtime kernel could not be opened: {detail}")]
    Runtime { detail: String },
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ArtifactFetchRequest {
    pub logical_path: String,
    pub expected_sha256: String,
    pub candidate_urls: Vec<String>,
    pub maximum_bytes: u64,
    pub redirects_allowed: bool,
}

#[derive(Clone, Debug, uniffi::Enum)]
pub enum ArtifactFetchResponse {
    Body {
        source_url: String,
        http_status: u16,
        bytes: Vec<u8>,
    },
    Redirect {
        source_url: String,
        http_status: u16,
        location: String,
    },
    Refused {
        reason: String,
    },
}

#[uniffi::export(callback_interface)]
pub trait ArtifactSource: Send + Sync {
    fn fetch(&self, request: ArtifactFetchRequest) -> ArtifactFetchResponse;
}

/// Raw host appearance facts. Native reports OS state; Rust owns the mapping
/// to the pinned NAP-THEME payload.
#[derive(Clone, Debug, uniffi::Record)]
pub struct NativeAppearanceSnapshot {
    pub dark: bool,
    pub increased_contrast: bool,
    pub reduced_transparency: bool,
    pub accent_red: u8,
    pub accent_green: u8,
    pub accent_blue: u8,
}

#[uniffi::export(callback_interface)]
pub trait NativeAppearanceSource: Send + Sync {
    fn current(&self) -> Option<NativeAppearanceSnapshot>;
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct NativeSettingsRequest {
    pub manifest_author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
    pub session_id: u64,
    pub section: Option<String>,
    pub schema_json: String,
    pub values_json: String,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum NativeSettingsOpenResult {
    Accepted,
    Saturated,
    Unavailable,
    Closed,
}

#[uniffi::export(callback_interface)]
pub trait NativeSettingsExecutor: Send + Sync {
    fn try_open(&self, request: NativeSettingsRequest) -> NativeSettingsOpenResult;
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct NativeIncActionRequest {
    pub manifest_author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
    pub session_id: u64,
    pub source_window_id: u64,
    pub kind: String,
    pub payload_json: String,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct NativeIncActionEnd {
    pub manifest_author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
    pub session_id: u64,
    pub source_window_id: u64,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum NativeIncActionEnqueueResult {
    Accepted,
    Backpressure,
    Closed,
}

#[uniffi::export(callback_interface)]
pub trait NativeIncActionExecutor: Send + Sync {
    fn try_enqueue(&self, request: NativeIncActionRequest) -> NativeIncActionEnqueueResult;
    fn session_ended(&self, end: NativeIncActionEnd);
}

/// Identifies the NAP-INTENT handler a launched/focused window should target.
/// `Principal` (manifest author + d tag + aggregate hash) already *is* an
/// exact-build identity, so this maps 1:1 onto a native workspace window
/// identity with no further resolution.
#[derive(Clone, Debug, uniffi::Record)]
pub struct NativeIntentActivationRequest {
    pub manifest_author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
}

/// Native signal fired before any webview session may exist yet: "create (if
/// needed) and bring to front the window for this handler." Distinct from
/// `NativeIncActionExecutor`, which is scoped to an already-live session and
/// refuses otherwise.
#[uniffi::export(callback_interface)]
pub trait NativeIntentActivationExecutor: Send + Sync {
    fn focus_or_launch(&self, handler: NativeIntentActivationRequest);
}

struct CallbackIntentActivation {
    callback: Arc<dyn NativeIntentActivationExecutor>,
}

impl fmt::Debug for CallbackIntentActivation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallbackIntentActivation")
            .finish_non_exhaustive()
    }
}

impl IntentActivationSink for CallbackIntentActivation {
    fn focus_or_launch(&self, handler: Principal) {
        self.callback
            .focus_or_launch(NativeIntentActivationRequest {
                manifest_author: handler.manifest_author().to_owned(),
                d_tag: handler.d_tag().to_owned(),
                aggregate_hash: handler.aggregate_hash().to_owned(),
            });
    }
}

struct CallbackArtifactSource {
    callback: Arc<dyn ArtifactSource>,
}

impl fmt::Debug for CallbackArtifactSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallbackArtifactSource")
            .finish_non_exhaustive()
    }
}

impl ManifestBlobSource for CallbackArtifactSource {
    fn fetch(&self, request: &BlobFetchRequest) -> Result<BlobFetchResponse, BlobSourceError> {
        let maximum_bytes = request.maximum_bytes();
        let response = self.callback.fetch(ArtifactFetchRequest {
            logical_path: request.logical_path().to_owned(),
            expected_sha256: request.digest().as_str().to_owned(),
            candidate_urls: request.candidate_urls().map(str::to_owned).collect(),
            maximum_bytes: maximum_bytes as u64,
            redirects_allowed: false,
        });
        match response {
            ArtifactFetchResponse::Body {
                source_url,
                http_status,
                bytes,
            } => {
                if bytes.len() > maximum_bytes {
                    return Err(BlobSourceError {
                        reason: format!(
                            "artifact source returned {} bytes; the maximum is {maximum_bytes}",
                            bytes.len()
                        ),
                    });
                }
                Ok(BlobFetchResponse::status(
                    source_url,
                    http_status,
                    Box::new(Cursor::new(bytes)),
                ))
            }
            ArtifactFetchResponse::Redirect {
                source_url,
                http_status,
                location,
            } => Ok(BlobFetchResponse::redirect(
                source_url,
                http_status,
                location,
            )),
            ArtifactFetchResponse::Refused { reason } => Err(BlobSourceError { reason }),
        }
    }
}

#[derive(Debug)]
struct RuntimeThemeSource {
    current: Mutex<Option<ThemeSnapshot>>,
}

impl RuntimeThemeSource {
    fn new(current: ThemeSnapshot) -> Self {
        Self {
            current: Mutex::new(Some(current)),
        }
    }

    fn replace(&self, current: ThemeSnapshot) {
        *self.current.lock() = Some(current);
    }
}

impl ThemeSource for RuntimeThemeSource {
    fn current(&self) -> Option<ThemeSnapshot> {
        self.current.lock().clone()
    }
}

struct CallbackSettingsExecutor {
    callback: Arc<dyn NativeSettingsExecutor>,
}

impl fmt::Debug for CallbackSettingsExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallbackSettingsExecutor")
            .finish_non_exhaustive()
    }
}

impl ProviderSettingsExecutor for CallbackSettingsExecutor {
    fn try_open(&self, request: ProviderSettingsRequest) -> Result<(), SettingsExecutorError> {
        let result = self.callback.try_open(NativeSettingsRequest {
            manifest_author: request.principal.manifest_author().to_owned(),
            d_tag: request.principal.d_tag().to_owned(),
            aggregate_hash: request.principal.aggregate_hash().to_owned(),
            session_id: request.session.0,
            section: request.section.as_deref().map(str::to_owned),
            schema_json: request.schema.as_str().to_owned(),
            values_json: request.values.as_str().to_owned(),
        });
        match result {
            NativeSettingsOpenResult::Accepted => Ok(()),
            NativeSettingsOpenResult::Saturated => Err(SettingsExecutorError::Saturated),
            NativeSettingsOpenResult::Unavailable => Err(SettingsExecutorError::Unavailable),
            NativeSettingsOpenResult::Closed => Err(SettingsExecutorError::Closed),
        }
    }
}

struct CallbackIncNativeActions {
    callback: Arc<dyn NativeIncActionExecutor>,
}

impl fmt::Debug for CallbackIncNativeActions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CallbackIncNativeActions")
            .finish_non_exhaustive()
    }
}

impl IncNativeActionSink for CallbackIncNativeActions {
    fn try_enqueue(&self, action: IncNativeAction) -> Result<(), IncNativeActionSinkError> {
        let request = NativeIncActionRequest {
            manifest_author: action.origin.principal.manifest_author().to_owned(),
            d_tag: action.origin.principal.d_tag().to_owned(),
            aggregate_hash: action.origin.principal.aggregate_hash().to_owned(),
            session_id: action.origin.session.0,
            source_window_id: action.origin.source_window.0,
            kind: match action.kind {
                IncNativeActionKind::NoteOpen => "note-open",
                IncNativeActionKind::ProfileOpen => "profile-open",
                IncNativeActionKind::ComposeOpen => "compose-open",
            }
            .to_owned(),
            payload_json: action.payload.as_str().to_owned(),
        };
        match self.callback.try_enqueue(request) {
            NativeIncActionEnqueueResult::Accepted => Ok(()),
            NativeIncActionEnqueueResult::Backpressure => {
                Err(IncNativeActionSinkError::Backpressure)
            }
            NativeIncActionEnqueueResult::Closed => Err(IncNativeActionSinkError::Closed),
        }
    }

    fn session_ended(&self, origin: &IncNativeActionOrigin, reason: IncNativeActionSessionEnd) {
        self.callback.session_ended(NativeIncActionEnd {
            manifest_author: origin.principal.manifest_author().to_owned(),
            d_tag: origin.principal.d_tag().to_owned(),
            aggregate_hash: origin.principal.aggregate_hash().to_owned(),
            session_id: origin.session.0,
            source_window_id: origin.source_window.0,
            reason: match reason {
                IncNativeActionSessionEnd::Closed(reason) => {
                    format!("closed-{}", format!("{reason:?}").to_ascii_lowercase())
                }
                IncNativeActionSessionEnd::Revoked => "revoked".to_owned(),
            },
        });
    }
}

#[derive(Clone, Debug, uniffi::Enum)]
pub enum ArtifactCoordinate {
    Snapshot { event_id: String, author: String },
    Root { author: String },
    Named { author: String, d_tag: String },
}

#[derive(Debug, uniffi::Object)]
pub struct VerifiedArtifact {
    handle: Arc<VerifiedArtifactHandle>,
    principal: Option<Principal>,
}

#[uniffi::export]
impl VerifiedArtifact {
    pub fn author(&self) -> String {
        self.handle.index().author().as_str().to_owned()
    }

    pub fn d_tag(&self) -> Option<String> {
        self.handle.index().d_tag().map(str::to_owned)
    }

    pub fn aggregate_hash(&self) -> String {
        self.handle.index().aggregate().as_str().to_owned()
    }

    pub fn manifest_kind(&self) -> u16 {
        self.handle.index().kind()
    }

    pub fn mode(&self) -> ArtifactExecutionMode {
        match self.handle.index().mode() {
            ArtifactMode::SingleFile => ArtifactExecutionMode::SingleFile,
            ArtifactMode::ExternalAssets => ArtifactExecutionMode::ExternalAssets,
        }
    }

    pub fn logical_paths(&self) -> Vec<String> {
        self.handle
            .index()
            .entries()
            .map(|entry| entry.path().to_owned())
            .collect()
    }

    /// Verified manifest requirements. Native presentation may render these,
    /// but launch authority always derives them again from the sealed handle.
    pub fn requires(&self) -> Vec<String> {
        self.handle
            .manifest()
            .requirements()
            .map(str::to_owned)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum ArtifactExecutionMode {
    SingleFile,
    ExternalAssets,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ArtifactVerification {
    pub artifact: Option<Arc<VerifiedArtifact>>,
    pub refusal: Option<RuntimeRefusal>,
}

#[derive(Clone, Debug, uniffi::Enum)]
pub enum VerifiedRead {
    Bytes {
        bytes: Vec<u8>,
        media_type: String,
        sha256: String,
    },
    Refused {
        refusal: RuntimeRefusal,
    },
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeRefusal {
    pub code: String,
    pub detail: String,
    pub occurred_at_millis: u64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeProviderUpdate {
    pub accepted: bool,
    pub attempted: u64,
    pub delivered: u64,
    pub refused: u64,
    pub refusal: Option<RuntimeRefusal>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct NativeConfigCommit {
    pub manifest_author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
    pub session_id: u64,
    pub values_json: String,
}

impl RuntimeProviderUpdate {
    fn accepted(report: ProviderPushReport) -> Self {
        Self {
            accepted: true,
            attempted: report.attempted as u64,
            delivered: report.delivered as u64,
            refused: report.refused as u64,
            refusal: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeAccountHandle {
    pub installation_id: u64,
    pub public_key: String,
    pub kind: RuntimeAccountKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeAccountKind {
    LocalSigner,
    ReadOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeAccountSnapshot {
    pub generation: u64,
    pub active_public_key: Option<String>,
    pub local_accounts: Vec<RuntimeAccountHandle>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeAccountFailure {
    Closed,
    InvalidSecretKey,
    InvalidPublicKey,
    Nip05ResolutionUnavailable,
    Capacity { limit: u64 },
    InstanceExhausted,
    StaleInstallation,
    Failed { reason: String },
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeAccountUpdate {
    pub accepted: bool,
    pub handle: Option<RuntimeAccountHandle>,
    pub snapshot: Option<RuntimeAccountSnapshot>,
    pub failure: Option<RuntimeAccountFailure>,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeExecutionProfile {
    Legacy,
    Renderer,
    Hybrid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeGrantDecision {
    Denied,
    AskEveryTime,
    AllowSession,
    AllowExactBuild,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeSensitivity {
    Ordinary,
    Sensitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimePermissionRequirement {
    Required,
    Optional,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimePermissionSensitivity {
    Ordinary,
    Sensitive,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimePermissionPlatformAvailability {
    Available,
    Unknown { reason: String },
    Unavailable { reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimePermissionExistingDecision {
    Denied,
    AskEveryTime,
    AllowSession,
    AllowExactBuild,
    Managed,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimePermissionDecisionOption {
    pub decision: RuntimeGrantDecision,
    pub valid: bool,
    pub invalid_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimePermissionCapabilitySnapshot {
    pub domain: String,
    pub requirement: RuntimePermissionRequirement,
    pub sensitivity: RuntimePermissionSensitivity,
    pub dependencies: Vec<String>,
    pub platform_availability: RuntimePermissionPlatformAvailability,
    pub existing_decision: RuntimePermissionExistingDecision,
    pub requested_decision: Option<RuntimeGrantDecision>,
    pub decision_options: Vec<RuntimePermissionDecisionOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimePermissionReviewSnapshot {
    pub coordinate: RuntimeExactBuildCoordinate,
    pub title: String,
    pub capabilities: Vec<RuntimePermissionCapabilitySnapshot>,
    pub launch_permitted: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimePermissionReviewResult {
    pub review: Option<RuntimePermissionReviewSnapshot>,
    pub refusal: Option<RuntimeRefusal>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimePermissionDecisionSelection {
    pub domain: String,
    pub decision: RuntimeGrantDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimePermissionDecisionBatch {
    pub coordinate: RuntimeExactBuildCoordinate,
    pub decisions: Vec<RuntimePermissionDecisionSelection>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimePermissionBatchUpdate {
    pub applied: bool,
    pub review: Option<RuntimePermissionReviewSnapshot>,
    pub refusal: Option<RuntimeRefusal>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeSessionSnapshot {
    pub id: u64,
    pub author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
    pub profile: RuntimeExecutionProfile,
    pub state: String,
    /// Exact kernel-negotiated domain set used by both native injection and
    /// the NAP-SHELL `shell.init` response.
    pub domains: Vec<String>,
}

/// Exact installed-build identity. Every library action remains bound to all
/// three coordinate fields; native callers cannot target a publisher/dTag
/// pair without naming the verified aggregate.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeExactBuildCoordinate {
    pub manifest_author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeInstalledBuildAvailability {
    /// Verified installation metadata survived, but this process does not
    /// currently retain a verifier-produced immutable artifact handle.
    MetadataOnly,
    /// This process retains the immutable exact-build handle required for an
    /// offline launch.
    SealedExactBytesReady,
}

/// Bounded, screen-shaped installed-build projection. Manifest metadata is
/// opaque verified JSON; native presentation must not reinterpret it as
/// runtime authority.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeInstalledBuildSnapshot {
    pub coordinate: RuntimeExactBuildCoordinate,
    pub title: String,
    pub manifest_metadata_json: String,
    pub availability: RuntimeInstalledBuildAvailability,
    pub active_session_ids: Vec<u64>,
    pub assigned_workspace_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeInstalledLibrarySnapshot {
    pub query: String,
    pub total_installed: u64,
    pub builds: Vec<RuntimeInstalledBuildSnapshot>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeBindingSnapshot {
    pub id: String,
    pub schema: String,
    pub logical_source_id: Option<String>,
    pub revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeReceiptStatus {
    Pending,
    Delivered,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeReceiptSnapshot {
    pub receipt_id: String,
    pub status: RuntimeReceiptStatus,
    pub delivery: String,
    pub latest_state_json: Option<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimePendingWriteSnapshot {
    pub operation_id: u64,
    pub approval_id: String,
    pub author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
    pub session_id: u64,
    pub account: String,
    pub draft_json: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeWorkspaceAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeWorkspaceRole {
    Feed,
    Detail,
    Profile,
    Thread,
    Composer,
    MediaPlayer,
    ToolWindow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeWorkspaceRenderer {
    Native,
    LegacyNapplet,
    Surface,
    Unavailable,
}

/// One coarse native workspace slot. Dynamic binding and navigation values
/// remain bounded JSON objects because their schemas belong to the selected
/// handler, while identity, role, renderer, visibility, and layout constraints
/// are typed at this boundary.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeWorkspaceSlot {
    pub slot_id: String,
    pub role: RuntimeWorkspaceRole,
    pub renderer: RuntimeWorkspaceRenderer,
    pub handler_id: String,
    pub manifest_author: Option<String>,
    pub d_tag: Option<String>,
    pub aggregate_hash: Option<String>,
    pub binding_parameters_json: String,
    pub navigation_json: String,
    pub visible: bool,
    pub order: u16,
    pub size_points: u16,
    pub minimum_points: u16,
    pub maximum_points: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RuntimeWorkspaceDefinition {
    pub schema_version: u16,
    pub workspace_id: String,
    pub axis: RuntimeWorkspaceAxis,
    pub slots: Vec<RuntimeWorkspaceSlot>,
    pub focused_slot_id: Option<String>,
    pub activity_drawer_visible: bool,
    pub preferences_json: String,
    pub retained_receipt_ids: Vec<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeWorkspaceUpdate {
    pub accepted: bool,
    pub workspace: Option<RuntimeWorkspaceDefinition>,
    pub refusal: Option<RuntimeRefusal>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeWorkspaceRestore {
    pub accepted: bool,
    pub workspaces: Vec<RuntimeWorkspaceDefinition>,
    pub refusal: Option<RuntimeRefusal>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeActivitySnapshot {
    pub author: String,
    pub d_tag: String,
    pub aggregate_hash: String,
    pub category: String,
    pub operation: String,
    pub outcome: String,
    pub occurred_at_millis: u64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeErrorSnapshot {
    pub code: String,
    pub author: Option<String>,
    pub d_tag: Option<String>,
    pub aggregate_hash: Option<String>,
    pub session_id: Option<u64>,
    pub detail: String,
    pub occurred_at_millis: u64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeSnapshot {
    pub revision: u64,
    pub closed: bool,
    pub installed_library: RuntimeInstalledLibrarySnapshot,
    pub sessions: Vec<RuntimeSessionSnapshot>,
    pub bindings: Vec<RuntimeBindingSnapshot>,
    pub pending_writes: Vec<RuntimePendingWriteSnapshot>,
    pub receipts: Vec<RuntimeReceiptSnapshot>,
    pub workspaces: Vec<RuntimeWorkspaceDefinition>,
    pub recent_activity: Vec<RuntimeActivitySnapshot>,
    pub recent_errors: Vec<RuntimeErrorSnapshot>,
    pub boundary_refusals: Vec<RuntimeRefusal>,
    pub active_resources: u64,
    pub resource_high_watermark: u64,
    pub resource_refusal_count: u64,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeEvent {
    pub sequence: u64,
    pub kind: String,
    pub detail: String,
    pub session_id: Option<u64>,
    pub response_json: Option<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeObservationFrame {
    pub snapshot: RuntimeSnapshot,
    pub catalog: RuntimeCatalogFeedSnapshot,
    pub events: Vec<RuntimeEvent>,
    pub oldest_available_event: u64,
    pub newest_available_event: u64,
    pub event_cursor_was_stale: bool,
}

#[uniffi::export(callback_interface)]
pub trait RuntimeObserver: Send + Sync {
    fn update(&self, frame: RuntimeObservationFrame);
}

#[derive(Debug, uniffi::Object)]
pub struct RuntimeObservation {
    stopped: Arc<AtomicBool>,
    signal: watch::Sender<u64>,
}

#[uniffi::export]
impl RuntimeObservation {
    pub fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            bump_signal(&self.signal);
        }
    }
}

impl Drop for RuntimeObservation {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ObservationStart {
    pub observation: Option<Arc<RuntimeObservation>>,
    pub refusal: Option<RuntimeRefusal>,
}

#[derive(uniffi::Object)]
pub struct RuntimeController {
    app: Arc<RuntimeApp>,
    data_plane: Arc<NmpDataPlane>,
    runtime_store: Arc<RuntimeStore>,
    artifact_cache: Arc<FileArtifactCache>,
    catalog: Arc<RuntimeCatalogService>,
    diagnostics: Arc<RuntimeDiagnosticsService>,
    artifact_source: CallbackArtifactSource,
    artifact_limits: ArtifactLimits,
    maximum_manifest_bytes: usize,
    maximum_verified_read_bytes: usize,
    maximum_blob_sources: usize,
    maximum_command_items: usize,
    maximum_command_string_bytes: usize,
    maximum_envelope_bytes: usize,
    theme_source: Option<Arc<RuntimeThemeSource>>,
    theme_provider: Option<Arc<ThemeProvider>>,
    config_provider: Option<Arc<ConfigProvider>>,
    intent_provider: Arc<IntentProvider>,
    artifacts: Arc<Mutex<BTreeMap<Principal, Arc<VerifiedArtifactHandle>>>>,
    boundary_refusals: Mutex<VecDeque<RuntimeRefusal>>,
    maximum_boundary_events: usize,
    signal: watch::Sender<u64>,
    observers: Arc<AtomicUsize>,
    maximum_observers: usize,
    closed: AtomicBool,
}

impl fmt::Debug for RuntimeController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeController")
            .field("snapshot_revision", &self.app.snapshot().revision)
            .field("retained_artifacts", &self.artifacts.lock().len())
            .field("active_observers", &self.observers.load(Ordering::Acquire))
            .field("maximum_observers", &self.maximum_observers)
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish()
    }
}

/// Derives the finite permission inventory exclusively from the artifact's
/// own signed `requires` tags. Native callers cannot select this profile or
/// supply capability names -- what the manifest declares is what it gets.
pub(crate) fn installation_capability_requests(
    handle: &VerifiedArtifactHandle,
) -> Result<Vec<CapabilityRequest>, String> {
    let requests = handle
        .manifest()
        .requirements()
        .map(|domain| {
            Capability::new(domain)
                .map(|capability| CapabilityRequest {
                    capability,
                    requirement: CapabilityRequirement::Required,
                })
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    if requests.len() > MAXIMUM_PERMISSION_DECISIONS {
        return Err(format!(
            "verified capability profile has {} domains; the maximum is {}",
            requests.len(),
            MAXIMUM_PERMISSION_DECISIONS
        ));
    }
    Ok(requests)
}

fn installed_manifest_event_id(build: &InstalledBuild) -> Result<String, String> {
    let metadata: serde_json::Value = serde_json::from_str(build.manifest_metadata.as_str())
        .map_err(|error| format!("installed manifest metadata is invalid JSON: {error}"))?;
    let event_id = metadata
        .get("event_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "installed manifest metadata has no verified event_id".to_owned())?;
    nmp_native_artifact::Sha256Digest::parse(event_id)
        .map_err(|error| format!("installed manifest event_id is invalid: {error}"))?;
    Ok(event_id.to_owned())
}

fn installed_confirmation(
    artifact: &VerifiedArtifact,
    build: &InstalledBuild,
    provenance: Vec<RuntimeCatalogProvenance>,
) -> RuntimeCatalogConfirmation {
    RuntimeCatalogConfirmation {
        event_id: artifact.handle.index().event_id().as_str().to_owned(),
        coordinate: format!(
            "35129:{}:{}",
            build.principal.manifest_author(),
            build.principal.d_tag()
        ),
        manifest_author: build.principal.manifest_author().to_owned(),
        d_tag: Some(build.principal.d_tag().to_owned()),
        title: Some(build.title.to_string()),
        aggregate_hash: build.principal.aggregate_hash().to_owned(),
        capabilities: build
            .capability_requests
            .iter()
            .map(|request| RuntimeCatalogCapability {
                domain: request.capability.as_str().to_owned(),
                requirement: match request.requirement {
                    CapabilityRequirement::Required => RuntimePermissionRequirement::Required,
                    CapabilityRequirement::Optional => RuntimePermissionRequirement::Optional,
                },
            })
            .collect(),
        provenance,
    }
}

#[uniffi::export]
impl RuntimeController {
    #[uniffi::constructor]
    pub fn open(
        config: RuntimeConfig,
        artifact_source: Box<dyn ArtifactSource>,
    ) -> Result<Arc<Self>, RuntimeOpenError> {
        open_runtime_controller(config, artifact_source, None, None, None, None)
    }

    #[uniffi::constructor]
    pub fn open_with_appearance(
        config: RuntimeConfig,
        artifact_source: Box<dyn ArtifactSource>,
        appearance_source: Box<dyn NativeAppearanceSource>,
    ) -> Result<Arc<Self>, RuntimeOpenError> {
        open_runtime_controller(
            config,
            artifact_source,
            Some(Arc::from(appearance_source)),
            None,
            None,
            None,
        )
    }

    #[uniffi::constructor]
    pub fn open_with_settings(
        config: RuntimeConfig,
        artifact_source: Box<dyn ArtifactSource>,
        settings_executor: Box<dyn NativeSettingsExecutor>,
    ) -> Result<Arc<Self>, RuntimeOpenError> {
        open_runtime_controller(
            config,
            artifact_source,
            None,
            Some(Arc::from(settings_executor)),
            None,
            None,
        )
    }

    #[uniffi::constructor]
    pub fn open_with_native_capabilities(
        config: RuntimeConfig,
        artifact_source: Box<dyn ArtifactSource>,
        appearance_source: Box<dyn NativeAppearanceSource>,
        settings_executor: Box<dyn NativeSettingsExecutor>,
    ) -> Result<Arc<Self>, RuntimeOpenError> {
        open_runtime_controller(
            config,
            artifact_source,
            Some(Arc::from(appearance_source)),
            Some(Arc::from(settings_executor)),
            None,
            None,
        )
    }

    #[uniffi::constructor]
    pub fn open_with_all_native_capabilities(
        config: RuntimeConfig,
        artifact_source: Box<dyn ArtifactSource>,
        appearance_source: Box<dyn NativeAppearanceSource>,
        settings_executor: Box<dyn NativeSettingsExecutor>,
        inc_action_executor: Box<dyn NativeIncActionExecutor>,
        intent_activation_executor: Box<dyn NativeIntentActivationExecutor>,
    ) -> Result<Arc<Self>, RuntimeOpenError> {
        open_runtime_controller(
            config,
            artifact_source,
            Some(Arc::from(appearance_source)),
            Some(Arc::from(settings_executor)),
            Some(Arc::from(inc_action_executor)),
            Some(Arc::from(intent_activation_executor)),
        )
    }
}

fn open_runtime_controller(
    config: RuntimeConfig,
    artifact_source: Box<dyn ArtifactSource>,
    appearance_source: Option<Arc<dyn NativeAppearanceSource>>,
    settings_executor: Option<Arc<dyn NativeSettingsExecutor>>,
    inc_action_executor: Option<Arc<dyn NativeIncActionExecutor>>,
    intent_activation_executor: Option<Arc<dyn NativeIntentActivationExecutor>>,
) -> Result<Arc<RuntimeController>, RuntimeOpenError> {
    let config = config.validated()?;
    let runtime_store = Arc::new(
        RuntimeStore::open(&config.runtime_store_path, StoreLimits::default()).map_err(
            |error| RuntimeOpenError::RuntimeStore {
                detail: error.to_string(),
            },
        )?,
    );
    let artifact_cache = Arc::new(
        FileArtifactCache::open(&config.artifact_cache_path).map_err(|error| {
            RuntimeOpenError::ArtifactCache {
                detail: error.to_string(),
            }
        })?,
    );
    let data_plane = Arc::new(
        NmpDataPlane::open(
            EngineConfig {
                store_path: config.nmp_store_path,
                indexer_relays: config.indexer_relays,
                app_relays: config.app_relays,
                fallback_relays: config.fallback_relays,
                allowed_local_relay_hosts: config.allowed_local_relay_hosts,
                max_relays: config.maximum_nmp_relays,
                ..EngineConfig::default()
            },
            config.maximum_bridge_workers,
        )
        .map_err(|error| RuntimeOpenError::Nmp {
            detail: error.to_string(),
        })?,
    );
    let catalog = Arc::new(
        RuntimeCatalogService::new(
            Arc::clone(&data_plane),
            Arc::clone(&artifact_cache),
            config.artifact_limits,
            config.maximum_manifest_bytes,
            config.maximum_blob_sources,
        )
        .map_err(|error| RuntimeOpenError::Runtime {
            detail: format!("catalog: {error}"),
        })?,
    );
    let diagnostics = Arc::new(RuntimeDiagnosticsService::new(&data_plane));
    let shell_provider = Arc::new(
        ShellProvider::new(
            Arc::new(RuntimeShellEnvironment),
            ShellProviderLimits::default(),
        )
        .map_err(|error| RuntimeOpenError::Runtime {
            detail: error.to_string(),
        })?,
    );
    let storage_provider: Arc<dyn Provider> = Arc::new(
        StorageProvider::new(Arc::clone(&runtime_store), StorageProviderLimits::default())
            .map_err(|error| RuntimeOpenError::Runtime {
                detail: error.to_string(),
            })?,
    );
    let identity_source: Arc<dyn IdentityDataPlane> = data_plane.clone();
    let identity_provider: Arc<dyn Provider> = IdentityProvider::connect(
        identity_source,
        Arc::new(NoopIdentityDiagnostics),
        IdentityProviderLimits::default(),
    )
    .map_err(|error| RuntimeOpenError::Runtime {
        detail: error.to_string(),
    })?;
    let inc_provider_concrete: Arc<IncProvider> = match inc_action_executor {
        Some(callback) => Arc::new(
            IncProvider::with_native_actions(
                Arc::new(AllowAllIncAcl),
                Arc::new(NoopIncActivity),
                Arc::new(CallbackIncNativeActions { callback }),
                IncProviderLimits::default(),
            )
            .map_err(|error| RuntimeOpenError::Runtime {
                detail: error.to_string(),
            })?,
        ),
        None => Arc::new(
            IncProvider::new(
                Arc::new(AllowAllIncAcl),
                Arc::new(NoopIncActivity),
                IncProviderLimits::default(),
            )
            .map_err(|error| RuntimeOpenError::Runtime {
                detail: error.to_string(),
            })?,
        ),
    };
    let inc_provider: Arc<dyn Provider> = inc_provider_concrete.clone();
    let artifacts: Arc<Mutex<BTreeMap<Principal, Arc<VerifiedArtifactHandle>>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let intent_dispatcher =
        RuntimeIntentDispatcher::new(Arc::clone(&inc_provider_concrete), Arc::clone(&artifacts));
    if let Some(callback) = intent_activation_executor {
        let activation: Arc<dyn IntentActivationSink> =
            Arc::new(CallbackIntentActivation { callback });
        intent_dispatcher.set_activation(Some(activation));
    }
    let intent_provider = Arc::new(
        IntentProvider::new(
            Arc::new(DefaultOnlyIntentPolicy),
            Arc::new(CancelIntentChoice),
            intent_dispatcher.clone() as Arc<dyn NativeIntentDispatcher>,
            Arc::new(NoopIntentActivity),
            IntentProviderLimits::default(),
        )
        .map_err(|error| RuntimeOpenError::Runtime {
            detail: error.to_string(),
        })?,
    );
    let intent_provider_erased: Arc<dyn Provider> = intent_provider.clone();
    let nostr_providers =
        NapNostrProviderSet::new(data_plane.clone(), NapNostrProviderLimits::default()).map_err(
            |error| RuntimeOpenError::Runtime {
                detail: error.to_string(),
            },
        )?;
    let outbox_provider: Arc<dyn Provider> = nostr_providers.outbox;
    let relay_provider: Arc<dyn Provider> = nostr_providers.relay;
    let (theme_source, theme_provider) = match appearance_source.and_then(|source| source.current())
    {
        Some(appearance) => {
            let snapshot =
                theme_from_appearance(appearance).map_err(|detail| RuntimeOpenError::Runtime {
                    detail: format!("native appearance source was invalid: {detail}"),
                })?;
            let source = Arc::new(RuntimeThemeSource::new(snapshot));
            let provider = Arc::new(
                ThemeProvider::new(source.clone(), ThemeProviderLimits::default()).map_err(
                    |error| RuntimeOpenError::Runtime {
                        detail: error.to_string(),
                    },
                )?,
            );
            (Some(source), Some(provider))
        }
        None => (None, None),
    };
    let config_provider = settings_executor
        .map(|callback| {
            ConfigProvider::new(
                Arc::clone(&runtime_store),
                Arc::new(CallbackSettingsExecutor { callback }),
                ConfigProviderLimits::default(),
            )
            .map(Arc::new)
            .map_err(|error| RuntimeOpenError::Runtime {
                detail: error.to_string(),
            })
        })
        .transpose()?;
    let mut providers = vec![
        storage_provider,
        identity_provider,
        inc_provider,
        intent_provider_erased,
        outbox_provider,
        relay_provider,
    ];
    if let Some(provider) = &theme_provider {
        let provider: Arc<dyn Provider> = provider.clone();
        providers.push(provider);
    }
    if let Some(provider) = &config_provider {
        let provider: Arc<dyn Provider> = provider.clone();
        providers.push(provider);
    }
    let app_limits = AppLimits::default();
    let maximum_envelope_bytes = app_limits.maximum_envelope_bytes;
    let app = RuntimeApp::open(RuntimeAppConfig {
        limits: app_limits,
        resource_limits: ResourceLimits::default(),
        grant_limits: GrantLimits::default(),
        bridge_limits: BridgeLimits::default(),
        binding_limits: BindingLimits::default(),
        store: runtime_store.clone(),
        data_plane: data_plane.clone(),
        clock: Arc::new(SystemClock),
        shell_provider,
        providers,
    })
    .map_err(|error| RuntimeOpenError::Runtime {
        detail: error.to_string(),
    })?;
    intent_dispatcher.bind(&app, &intent_provider);
    let (signal, _) = watch::channel(0_u64);
    let controller = Arc::new(RuntimeController {
        app,
        data_plane,
        runtime_store,
        artifact_cache,
        catalog,
        diagnostics,
        artifact_source: CallbackArtifactSource {
            callback: Arc::from(artifact_source),
        },
        artifact_limits: config.artifact_limits,
        maximum_manifest_bytes: config.maximum_manifest_bytes,
        maximum_verified_read_bytes: config.maximum_verified_read_bytes,
        maximum_blob_sources: config.maximum_blob_sources,
        maximum_command_items: config.maximum_command_items,
        maximum_command_string_bytes: config.maximum_command_string_bytes,
        maximum_envelope_bytes,
        theme_source,
        theme_provider,
        config_provider,
        intent_provider,
        artifacts,
        boundary_refusals: Mutex::new(VecDeque::with_capacity(config.maximum_boundary_events)),
        maximum_boundary_events: config.maximum_boundary_events,
        signal,
        observers: Arc::new(AtomicUsize::new(0)),
        maximum_observers: config.maximum_observers,
        closed: AtomicBool::new(false),
    });
    Ok(controller)
}

impl RuntimeController {
    /// Registers this napplet as a NAP-INTENT handler for every archetype it
    /// declared. Refusals are recorded, not propagated -- an invalid or
    /// oversized archetype declaration should never block the rest of an
    /// otherwise-valid install.
    fn register_intent_handler(&self, principal: &Principal, handle: &VerifiedArtifactHandle) {
        let mut by_slug: BTreeMap<Arc<str>, BTreeSet<Arc<str>>> = BTreeMap::new();
        for declaration in handle.manifest().archetypes() {
            by_slug
                .entry(Arc::clone(&declaration.slug))
                .or_default()
                .insert(Arc::clone(&declaration.protocol));
        }
        if by_slug.is_empty() {
            return;
        }
        let declarations = by_slug
            .into_iter()
            .map(|(archetype, conventions)| IntentHandlerDeclaration {
                archetype,
                title: None,
                actions: BTreeSet::from([Arc::from("open")]),
                conventions,
            })
            .collect();
        if let Err(error) = self
            .intent_provider
            .register_handler(principal.clone(), declarations)
        {
            self.record_refusal("intent-handler-registration", error.to_string());
        }
    }

    fn verified_installed_artifact(
        &self,
        build: &InstalledBuild,
        handle: Arc<VerifiedArtifactHandle>,
    ) -> Result<Arc<VerifiedArtifact>, RuntimeCatalogFailure> {
        let expected_event_id = installed_manifest_event_id(build)
            .map_err(|detail| runtime_catalog_failure("installed-metadata-invalid", detail))?;
        let index = handle.index();
        if index.kind() != 35_129
            || index.event_id().as_str() != expected_event_id
            || index.author().as_str() != build.principal.manifest_author()
            || index.d_tag() != Some(build.principal.d_tag())
            || index.aggregate().as_str() != build.principal.aggregate_hash()
        {
            return Err(runtime_catalog_failure(
                "installed-artifact-mismatch",
                "the verifier handle does not match the persisted exact signed manifest",
            ));
        }
        let artifact = Arc::new(VerifiedArtifact {
            handle,
            principal: Some(build.principal.clone()),
        });
        let requests = installation_capability_requests(&artifact.handle)
            .map_err(|detail| runtime_catalog_failure("installed-capability-mismatch", detail))?;
        if requests != build.capability_requests {
            return Err(runtime_catalog_failure(
                "installed-capability-mismatch",
                "the verified manifest capability inventory differs from the persisted installation",
            ));
        }
        Ok(artifact)
    }

    /// Reconstructs a verified artifact handle purely from local state: the
    /// exact signed manifest event bytes retained in `installed`'s metadata
    /// at original install time, and the sealed artifact bytes already
    /// committed to `self.artifact_cache`. No network access. Re-verifies
    /// the event signature exactly as a fresh install would, so a corrupted
    /// or substituted retained event is refused the same way any other
    /// invalid manifest would be.
    fn reopen_sealed_artifact(
        &self,
        principal: &Principal,
        installed: &InstalledBuild,
    ) -> Result<Arc<VerifiedArtifactHandle>, RuntimeCatalogFailure> {
        let metadata: serde_json::Value =
            serde_json::from_str(installed.manifest_metadata.as_str()).map_err(|error| {
                runtime_catalog_failure(
                    "installed-manifest-event-unavailable",
                    format!("installed manifest metadata is invalid JSON: {error}"),
                )
            })?;
        let signed_event_b64 = metadata
            .get("signed_event_b64")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                runtime_catalog_failure(
                    "installed-manifest-event-unavailable",
                    "this build was installed before offline reopen was supported; reinstall it once to enable reopening after a restart",
                )
            })?;
        let event_json = base64::engine::general_purpose::STANDARD
            .decode(signed_event_b64)
            .map_err(|error| {
                runtime_catalog_failure(
                    "installed-manifest-event-unavailable",
                    format!("retained signed event is not valid base64: {error}"),
                )
            })?;
        let coordinate = ManifestCoordinate::named(principal.manifest_author(), principal.d_tag())
            .map_err(|error| {
                runtime_catalog_failure("invalid-exact-build-coordinate", error.to_string())
            })?;
        let verifier = ManifestEventVerifier::new(ManifestEventLimits {
            maximum_event_bytes: self.maximum_manifest_bytes,
            ..ManifestEventLimits::default()
        })
        .map_err(|error| runtime_catalog_failure("invalid-limits", error.to_string()))?;
        let handle =
            reopen_verified_artifact(&verifier, &event_json, &coordinate, &self.artifact_cache)
                .map_err(|error| match error {
                    ManifestError::Artifact(_) => {
                        runtime_catalog_failure("sealed-bytes-unavailable", error.to_string())
                    }
                    _ => runtime_catalog_failure(
                        "installed-manifest-event-unavailable",
                        error.to_string(),
                    ),
                })?;
        Ok(Arc::new(handle))
    }

    fn refusal(&self, code: impl Into<String>, detail: impl Into<String>) -> RuntimeRefusal {
        RuntimeRefusal {
            code: code.into(),
            detail: detail.into(),
            occurred_at_millis: now_millis(),
        }
    }

    fn library_principal(&self, coordinate: RuntimeExactBuildCoordinate) -> Option<Principal> {
        match Principal::new(
            coordinate.manifest_author,
            coordinate.d_tag,
            coordinate.aggregate_hash,
        ) {
            Ok(principal) => Some(principal),
            Err(error) => {
                self.record_refusal("invalid-exact-build-coordinate", error.to_string());
                None
            }
        }
    }

    fn workspace_refusal(
        &self,
        code: impl Into<String>,
        detail: impl Into<String>,
    ) -> RuntimeRefusal {
        let refusal = self.refusal(code, detail);
        let mut refusals = self.boundary_refusals.lock();
        if refusals.len() == self.maximum_boundary_events {
            refusals.pop_front();
        }
        refusals.push_back(refusal.clone());
        drop(refusals);
        bump_signal(&self.signal);
        refusal
    }

    fn account_update(&self, handle: Option<LocalAccountHandle>) -> RuntimeAccountUpdate {
        let projected_handle = handle.map(project_account_handle);
        bump_signal(&self.signal);
        match self.data_plane.local_account_snapshot() {
            Ok(snapshot) => RuntimeAccountUpdate {
                accepted: true,
                handle: projected_handle,
                snapshot: Some(project_account_snapshot(snapshot)),
                failure: None,
            },
            Err(error) => RuntimeAccountUpdate {
                accepted: true,
                handle: projected_handle,
                snapshot: None,
                failure: Some(project_account_error(error)),
            },
        }
    }

    fn provider_refusal(
        &self,
        code: impl Into<String>,
        detail: impl Into<String>,
    ) -> RuntimeProviderUpdate {
        let refusal = self.refusal(code, detail);
        let mut refusals = self.boundary_refusals.lock();
        if refusals.len() == self.maximum_boundary_events {
            refusals.pop_front();
        }
        refusals.push_back(refusal.clone());
        drop(refusals);
        bump_signal(&self.signal);
        RuntimeProviderUpdate {
            accepted: false,
            attempted: 0,
            delivered: 0,
            refused: 0,
            refusal: Some(refusal),
        }
    }

    fn record_refusal(&self, code: impl Into<String>, detail: impl Into<String>) {
        let refusal = self.refusal(code, detail);
        let mut refusals = self.boundary_refusals.lock();
        if refusals.len() == self.maximum_boundary_events {
            refusals.pop_front();
        }
        refusals.push_back(refusal);
        drop(refusals);
        bump_signal(&self.signal);
    }

    fn project_snapshot(&self, snapshot: &AppSnapshot) -> RuntimeSnapshot {
        RuntimeSnapshot {
            revision: snapshot.revision,
            closed: snapshot.closed,
            installed_library: RuntimeInstalledLibrarySnapshot {
                query: snapshot.library.query.to_string(),
                total_installed: snapshot.library.total_installed as u64,
                builds: snapshot
                    .library
                    .builds
                    .iter()
                    .map(|view| RuntimeInstalledBuildSnapshot {
                        coordinate: RuntimeExactBuildCoordinate {
                            manifest_author: view.build.principal.manifest_author().to_owned(),
                            d_tag: view.build.principal.d_tag().to_owned(),
                            aggregate_hash: view.build.principal.aggregate_hash().to_owned(),
                        },
                        title: view.build.title.to_string(),
                        manifest_metadata_json: view.build.manifest_metadata.as_str().to_owned(),
                        availability: match view.availability {
                            InstalledBuildAvailability::MetadataOnly => {
                                RuntimeInstalledBuildAvailability::MetadataOnly
                            }
                            InstalledBuildAvailability::SealedExactBytesReady => {
                                RuntimeInstalledBuildAvailability::SealedExactBytesReady
                            }
                        },
                        active_session_ids: view
                            .active_sessions
                            .iter()
                            .map(|session| session.0)
                            .collect(),
                        assigned_workspace_ids: snapshot
                            .workspaces
                            .iter()
                            .filter(|workspace| {
                                workspace.assigned_builds.contains(&view.build.principal)
                            })
                            .map(|workspace| workspace.id.to_string())
                            .collect(),
                    })
                    .collect(),
            },
            sessions: snapshot
                .sessions
                .iter()
                .map(|session| RuntimeSessionSnapshot {
                    id: session.id.0,
                    author: session.principal.manifest_author().to_owned(),
                    d_tag: session.principal.d_tag().to_owned(),
                    aggregate_hash: session.principal.aggregate_hash().to_owned(),
                    profile: project_profile(session.profile),
                    state: format!("{:?}", session.state).to_ascii_lowercase(),
                    domains: snapshot
                        .session_domains
                        .iter()
                        .find(|view| view.session == session.id)
                        .map(|view| {
                            view.domains
                                .iter()
                                .map(|domain| domain.as_str().to_owned())
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect(),
            bindings: snapshot
                .bindings
                .iter()
                .map(|binding| RuntimeBindingSnapshot {
                    id: binding.id.to_string(),
                    schema: binding.schema.to_string(),
                    logical_source_id: binding.logical_source_id.as_deref().map(str::to_owned),
                    revision: binding.revision,
                })
                .collect(),
            pending_writes: snapshot
                .pending_writes
                .iter()
                .map(|pending| RuntimePendingWriteSnapshot {
                    operation_id: pending.operation.0,
                    approval_id: pending.approval_id.to_string(),
                    author: pending.principal.manifest_author().to_owned(),
                    d_tag: pending.principal.d_tag().to_owned(),
                    aggregate_hash: pending.principal.aggregate_hash().to_owned(),
                    session_id: pending.session.0,
                    account: pending.account.0.to_string(),
                    draft_json: pending.draft.as_str().to_owned(),
                })
                .collect(),
            receipts: snapshot
                .receipts
                .iter()
                .map(|receipt| RuntimeReceiptSnapshot {
                    receipt_id: receipt.receipt_id.0.to_string(),
                    status: match receipt.delivery {
                        ReceiptDeliveryState::Observing | ReceiptDeliveryState::NotFound => {
                            RuntimeReceiptStatus::Pending
                        }
                        ReceiptDeliveryState::Closed => RuntimeReceiptStatus::Delivered,
                    },
                    delivery: format!("{:?}", receipt.delivery).to_ascii_lowercase(),
                    latest_state_json: receipt
                        .latest
                        .as_ref()
                        .map(|latest| latest.state.as_str().to_owned()),
                })
                .collect(),
            workspaces: snapshot
                .workspaces
                .iter()
                .filter_map(|workspace| workspace_from_view(workspace).ok())
                .collect(),
            recent_activity: snapshot
                .recent_activity
                .iter()
                .map(|fact| RuntimeActivitySnapshot {
                    author: fact.principal.manifest_author().to_owned(),
                    d_tag: fact.principal.d_tag().to_owned(),
                    aggregate_hash: fact.principal.aggregate_hash().to_owned(),
                    category: fact.category.to_string(),
                    operation: fact.operation.to_string(),
                    outcome: fact.outcome.to_string(),
                    occurred_at_millis: fact.occurred_at_millis,
                })
                .collect(),
            recent_errors: snapshot
                .recent_errors
                .iter()
                .map(|fact| RuntimeErrorSnapshot {
                    code: format!("{:?}", fact.code).to_ascii_lowercase(),
                    author: fact
                        .principal
                        .as_ref()
                        .map(|principal| principal.manifest_author().to_owned()),
                    d_tag: fact
                        .principal
                        .as_ref()
                        .map(|principal| principal.d_tag().to_owned()),
                    aggregate_hash: fact
                        .principal
                        .as_ref()
                        .map(|principal| principal.aggregate_hash().to_owned()),
                    session_id: fact.session.map(|session| session.0),
                    detail: fact.detail.to_string(),
                    occurred_at_millis: fact.occurred_at_millis,
                })
                .collect(),
            boundary_refusals: self.boundary_refusals.lock().iter().cloned().collect(),
            active_resources: snapshot.resources.admitted as u64,
            resource_high_watermark: snapshot.resources.high_watermark as u64,
            resource_refusal_count: snapshot.resources.refusal_count,
        }
    }
}

impl Drop for RuntimeController {
    fn drop(&mut self) {
        self.close();
    }
}

struct ObserverPermit(Arc<AtomicUsize>);

impl Drop for ObserverPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct SystemClock;

impl KernelClock for SystemClock {
    fn now_millis(&self) -> u64 {
        now_millis()
    }
}

#[derive(Debug)]
struct RuntimeShellEnvironment;

impl ShellEnvironmentSource for RuntimeShellEnvironment {
    fn environment(
        &self,
        _principal: &Principal,
        _session: SessionId,
        offered_domains: &BTreeSet<Capability>,
    ) -> Result<ShellEnvironment, ShellEnvironmentError> {
        ShellEnvironment::new(
            offered_domains.iter().cloned(),
            std::iter::empty::<Arc<str>>(),
            ShellEnvironmentLimits::default(),
        )
    }
}

fn media_type_for(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn validate_string(name: &str, value: &str, maximum: usize) -> Result<(), RuntimeOpenError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(RuntimeOpenError::InvalidConfig {
            detail: format!("{name} must be non-empty, control-free, and at most {maximum} bytes"),
        });
    }
    Ok(())
}

fn nonzero_usize(value: u64, name: &str) -> Result<usize, RuntimeOpenError> {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| RuntimeOpenError::InvalidConfig {
            detail: format!("{name} must fit usize and be non-zero"),
        })
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn bump_signal(signal: &watch::Sender<u64>) {
    signal.send_modify(|revision| *revision = revision.wrapping_add(1));
}

#[cfg(test)]
mod tests;
