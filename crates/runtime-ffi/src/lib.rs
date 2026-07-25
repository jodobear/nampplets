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
    PlatformEvent, ProviderOperationId, RuntimeApp, RuntimeAppConfig, WorkspaceView,
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
const GOOD_MORNING_AUTHOR: &str =
    "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5";
const GOOD_MORNING_D_TAG: &str = "good-morning";
const GOOD_MORNING_AGGREGATE_HASH: &str =
    "828a6df02afd56782ea20f805084acce65c53f7c37554948c1e0a64aa5a2b0a8";
const GOOD_MORNING_CAPABILITY_PROFILE: &[(&str, CapabilityRequirement)] = &[
    ("identity", CapabilityRequirement::Required),
    ("inc", CapabilityRequirement::Required),
    ("outbox", CapabilityRequirement::Required),
    ("resource", CapabilityRequirement::Optional),
    ("theme", CapabilityRequirement::Optional),
    ("link", CapabilityRequirement::Optional),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimePermissionMode {
    Interactive,
    DemoPinnedGoodMorning,
}

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
    pub permission_mode: RuntimePermissionMode,
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
            permission_mode: self.permission_mode,
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
            permission_mode: RuntimePermissionMode::Interactive,
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
    permission_mode: RuntimePermissionMode,
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

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeReceiptSnapshot {
    pub receipt_id: String,
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
    artifacts: Mutex<BTreeMap<Principal, Arc<VerifiedArtifactHandle>>>,
    boundary_refusals: Mutex<VecDeque<RuntimeRefusal>>,
    maximum_boundary_events: usize,
    signal: watch::Sender<u64>,
    observers: Arc<AtomicUsize>,
    maximum_observers: usize,
    permission_mode: RuntimePermissionMode,
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

/// Derives the finite permission inventory exclusively from verified bytes and
/// Rust-owned compatibility policy.
///
/// Signed `requires` tags remain authoritative for general artifacts. The
/// published Good Morning fixture predates those tags, so its immutable exact
/// build receives the required/optional profile already pinned by the native
/// runtime compatibility corpus. Native callers cannot select this profile or
/// supply capability names.
fn installation_capability_requests(
    artifact: &VerifiedArtifact,
) -> Result<Vec<CapabilityRequest>, String> {
    let mut requests = artifact
        .handle
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

    let is_pinned_good_morning = artifact.handle.index().author().as_str() == GOOD_MORNING_AUTHOR
        && artifact.handle.index().d_tag() == Some(GOOD_MORNING_D_TAG)
        && artifact.handle.index().aggregate().as_str() == GOOD_MORNING_AGGREGATE_HASH;
    if is_pinned_good_morning {
        debug_assert!(requests.is_empty());
        for (domain, requirement) in GOOD_MORNING_CAPABILITY_PROFILE {
            requests.push(CapabilityRequest {
                capability: Capability::new(*domain).map_err(|error| error.to_string())?,
                requirement: *requirement,
            });
        }
    }
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
        open_runtime_controller(config, artifact_source, None, None, None)
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
        )
    }

    #[uniffi::constructor]
    pub fn open_with_all_native_capabilities(
        config: RuntimeConfig,
        artifact_source: Box<dyn ArtifactSource>,
        appearance_source: Box<dyn NativeAppearanceSource>,
        settings_executor: Box<dyn NativeSettingsExecutor>,
        inc_action_executor: Box<dyn NativeIncActionExecutor>,
    ) -> Result<Arc<Self>, RuntimeOpenError> {
        open_runtime_controller(
            config,
            artifact_source,
            Some(Arc::from(appearance_source)),
            Some(Arc::from(settings_executor)),
            Some(Arc::from(inc_action_executor)),
        )
    }
}

fn open_runtime_controller(
    config: RuntimeConfig,
    artifact_source: Box<dyn ArtifactSource>,
    appearance_source: Option<Arc<dyn NativeAppearanceSource>>,
    settings_executor: Option<Arc<dyn NativeSettingsExecutor>>,
    inc_action_executor: Option<Arc<dyn NativeIncActionExecutor>>,
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
    let inc_provider: Arc<dyn Provider> = match inc_action_executor {
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
        artifacts: Mutex::new(BTreeMap::new()),
        boundary_refusals: Mutex::new(VecDeque::with_capacity(config.maximum_boundary_events)),
        maximum_boundary_events: config.maximum_boundary_events,
        signal,
        observers: Arc::new(AtomicUsize::new(0)),
        maximum_observers: config.maximum_observers,
        permission_mode: config.permission_mode,
        closed: AtomicBool::new(false),
    });
    // Demo profiles are deliberately permissive for local end-to-end demos.
    // Re-apply that explicit policy to metadata restored from a prior process
    // too; otherwise a first run under interactive review can leave a denied
    // exact-build grant persisted forever, making NAP-OUTBOX appear absent on
    // the next demo launch. The helper remains a no-op for interactive
    // profiles and still binds every decision to the restored exact principal.
    controller.grant_demo_permissions_for_installed_builds();
    Ok(controller)
}

#[uniffi::export]
impl RuntimeController {
    /// Applies one event-driven native appearance change. The source is a
    /// single latest value; provider delivery uses finite conflating lanes.
    pub fn update_appearance(&self, appearance: NativeAppearanceSnapshot) -> RuntimeProviderUpdate {
        if self.closed.load(Ordering::Acquire) {
            return self.provider_refusal("closed", "runtime is closed");
        }
        let (Some(source), Some(provider)) = (&self.theme_source, &self.theme_provider) else {
            return self.provider_refusal(
                "theme-unavailable",
                "no native appearance source was registered",
            );
        };
        let snapshot = match theme_from_appearance(appearance) {
            Ok(snapshot) => snapshot,
            Err(detail) => return self.provider_refusal("invalid-appearance", detail),
        };
        source.replace(snapshot.clone());
        match provider.publish_changed(&snapshot) {
            Ok(report) => RuntimeProviderUpdate::accepted(report),
            Err(error) => self.provider_refusal("theme-push-refused", error.to_string()),
        }
    }

    /// Trusted native settings commit. Rust rechecks the exact active session,
    /// exact-build principal, schema, values, and persistence before pushing.
    pub fn commit_config_values(&self, commit: NativeConfigCommit) -> RuntimeProviderUpdate {
        if self.closed.load(Ordering::Acquire) {
            return self.provider_refusal("closed", "runtime is closed");
        }
        let Some(provider) = &self.config_provider else {
            return self.provider_refusal(
                "config-unavailable",
                "no native settings executor was registered",
            );
        };
        if commit.values_json.len() > ConfigProviderLimits::default().maximum_values_bytes {
            return self.provider_refusal(
                "config-values-too-large",
                "native settings values exceed the configured byte limit",
            );
        }
        let principal =
            match Principal::new(commit.manifest_author, commit.d_tag, commit.aggregate_hash) {
                Ok(principal) => principal,
                Err(error) => return self.provider_refusal("invalid-principal", error.to_string()),
            };
        let session = SessionId(commit.session_id);
        let active = self.app.snapshot().sessions.iter().any(|candidate| {
            candidate.id == session
                && candidate.principal == principal
                && candidate.state == SessionState::Running
        });
        if !active {
            return self.provider_refusal(
                "settings-session-closed",
                "the exact settings session is no longer running",
            );
        }
        let values = match serde_json::from_str(&commit.values_json) {
            Ok(values) => values,
            Err(_) => {
                return self.provider_refusal(
                    "invalid-config-values",
                    "native settings returned invalid JSON",
                );
            }
        };
        match provider.commit_values(&principal, &values) {
            Ok(report) => RuntimeProviderUpdate::accepted(report),
            Err(error) => self.provider_refusal("config-commit-refused", error.to_string()),
        }
    }

    /// Registers a local signing account through NMP without retaining or
    /// reflecting the supplied secret. Registration does not silently switch
    /// the active account; native UI must explicitly activate the returned
    /// exact installation handle.
    pub fn register_local_account(&self, secret_key: String) -> RuntimeAccountUpdate {
        if secret_key.is_empty()
            || secret_key.len() > 1_024
            || secret_key.chars().any(char::is_control)
        {
            return RuntimeAccountUpdate {
                accepted: false,
                handle: None,
                snapshot: None,
                failure: Some(RuntimeAccountFailure::InvalidSecretKey),
            };
        }
        match self.data_plane.register_local_account(&secret_key) {
            Ok(handle) => self.account_update(Some(handle)),
            Err(error) => RuntimeAccountUpdate {
                accepted: false,
                handle: None,
                snapshot: None,
                failure: Some(project_account_error(error)),
            },
        }
    }

    /// Registers a keyless read-only identity from canonical hexadecimal or
    /// `npub` input. Registration remains separate from activation.
    ///
    /// NIP-05-shaped input receives a typed refusal because the pinned NMP
    /// public facade has no governed resolver; this boundary never performs
    /// application-owned HTTP, DNS, or NIP-05 verification.
    pub fn register_read_only_account(&self, public_identity: String) -> RuntimeAccountUpdate {
        if public_identity.is_empty()
            || public_identity.len() > 1_024
            || public_identity.chars().any(char::is_control)
        {
            return RuntimeAccountUpdate {
                accepted: false,
                handle: None,
                snapshot: None,
                failure: Some(RuntimeAccountFailure::InvalidPublicKey),
            };
        }
        match self.data_plane.register_read_only_account(&public_identity) {
            Ok(handle) => self.account_update(Some(handle)),
            Err(error) => RuntimeAccountUpdate {
                accepted: false,
                handle: None,
                snapshot: None,
                failure: Some(project_account_error(error)),
            },
        }
    }

    /// Selects one exact, currently-owned local account installation.
    pub fn activate_local_account(&self, handle: RuntimeAccountHandle) -> RuntimeAccountUpdate {
        let handle = local_account_handle(handle);
        match self.data_plane.activate_local_account(&handle) {
            Ok(_) => self.account_update(Some(handle)),
            Err(error) => RuntimeAccountUpdate {
                accepted: false,
                handle: None,
                snapshot: None,
                failure: Some(project_account_error(error)),
            },
        }
    }

    /// Signs out without removing any registered signer. Already accepted
    /// writes remain frozen to the account selected at acceptance.
    pub fn logout_local_account(&self) -> RuntimeAccountUpdate {
        match self.data_plane.logout_local_account() {
            Ok(_) => self.account_update(None),
            Err(error) => RuntimeAccountUpdate {
                accepted: false,
                handle: None,
                snapshot: None,
                failure: Some(project_account_error(error)),
            },
        }
    }

    /// Removes only the exact local account installation named by the opaque
    /// public handle. Forged, replaced, or stale handles are refused.
    pub fn remove_local_account(&self, handle: RuntimeAccountHandle) -> RuntimeAccountUpdate {
        let handle = local_account_handle(handle);
        match self.data_plane.remove_local_account(&handle) {
            Ok(_) => self.account_update(None),
            Err(error) => RuntimeAccountUpdate {
                accepted: false,
                handle: None,
                snapshot: None,
                failure: Some(project_account_error(error)),
            },
        }
    }

    pub fn account_snapshot(&self) -> RuntimeAccountUpdate {
        match self.data_plane.local_account_snapshot() {
            Ok(snapshot) => RuntimeAccountUpdate {
                accepted: true,
                handle: None,
                snapshot: Some(project_account_snapshot(snapshot)),
                failure: None,
            },
            Err(error) => RuntimeAccountUpdate {
                accepted: false,
                handle: None,
                snapshot: None,
                failure: Some(project_account_error(error)),
            },
        }
    }

    /// Reads the latest replacement from the profile's permanent finite NMP
    /// manifest feed. A non-empty query filters that replacement locally; it
    /// never opens another relay subscription or claims NIP-50 completeness.
    pub fn catalog_browse(&self, query: String) -> RuntimeCatalogPageResult {
        if self.closed.load(Ordering::Acquire) {
            return RuntimeCatalogPageResult {
                page: None,
                failure: Some(runtime_catalog_failure("closed", "runtime is closed")),
            };
        }
        if query.len() > 256 || query.chars().any(char::is_control) {
            return RuntimeCatalogPageResult {
                page: None,
                failure: Some(runtime_catalog_failure(
                    "invalid-query",
                    "catalog query exceeds 256 UTF-8 bytes or contains control characters",
                )),
            };
        }
        match self.catalog.browse(Some(&query)) {
            Ok(page) => RuntimeCatalogPageResult {
                page: Some(page),
                failure: None,
            },
            Err(error) => RuntimeCatalogPageResult {
                page: None,
                failure: Some(project_catalog_error(error)),
            },
        }
    }

    /// Returns the latest unfiltered catalog feed replacement and revision.
    /// The underlying NMP subscription remains profile-owned and permanent.
    pub fn catalog_feed_snapshot(&self) -> RuntimeCatalogFeedSnapshot {
        self.catalog.feed_snapshot(None)
    }

    /// Freezes an exact review from one entry in the most recent bounded page.
    pub fn catalog_review_entry(&self, event_id: String) -> RuntimeCatalogReviewResult {
        if self.closed.load(Ordering::Acquire) {
            return RuntimeCatalogReviewResult {
                review: None,
                failure: Some(runtime_catalog_failure("closed", "runtime is closed")),
            };
        }
        match self.catalog.begin_review_for_entry(&event_id) {
            Ok(review) => RuntimeCatalogReviewResult {
                review: Some(review),
                failure: None,
            },
            Err(error) => RuntimeCatalogReviewResult {
                review: None,
                failure: Some(project_catalog_error(error)),
            },
        }
    }

    /// Parses and freezes an exact public manifest coordinate entirely in
    /// Rust. Native presentation never interprets Nostr coordinate identity.
    pub fn catalog_review_manual(&self, coordinate: String) -> RuntimeCatalogReviewResult {
        if self.closed.load(Ordering::Acquire) {
            return RuntimeCatalogReviewResult {
                review: None,
                failure: Some(runtime_catalog_failure("closed", "runtime is closed")),
            };
        }
        let coordinate = match parse_catalog_coordinate(&coordinate) {
            Ok(coordinate) => coordinate,
            Err(detail) => {
                return RuntimeCatalogReviewResult {
                    review: None,
                    failure: Some(runtime_catalog_failure("invalid-coordinate", detail)),
                };
            }
        };
        match self.catalog.begin_review(coordinate) {
            Ok(review) => RuntimeCatalogReviewResult {
                review: Some(review),
                failure: None,
            },
            Err(error) => RuntimeCatalogReviewResult {
                review: None,
                failure: Some(project_catalog_error(error)),
            },
        }
    }

    /// Cancels and discards one opaque exact review without side effects.
    pub fn catalog_cancel_review(&self, token: String) -> RuntimeCatalogCancellationResult {
        match self.catalog.cancel_review(&token) {
            Ok(()) => RuntimeCatalogCancellationResult {
                cancelled: true,
                failure: None,
            },
            Err(error) => RuntimeCatalogCancellationResult {
                cancelled: false,
                failure: Some(project_catalog_error(error)),
            },
        }
    }

    /// Cancels transient exact review/acquisition work. The profile-owned
    /// catalog feed stays subscribed until the profile closes.
    pub fn catalog_cancel_pending(&self) -> RuntimeCatalogCancellationResult {
        self.catalog.cancel_pending();
        RuntimeCatalogCancellationResult {
            cancelled: true,
            failure: None,
        }
    }

    /// Confirms one opaque frozen review and installs its immutable exact
    /// bytes. The pinned Good Morning demo profile receives the Rust-owned
    /// exact-build grant set immediately so the native Workbench can exercise
    /// the complete journey; other builds remain review-gated. This never
    /// launches the napplet.
    pub fn catalog_confirm_install(
        &self,
        token: String,
        expected_author: String,
        expected_d_tag: String,
        expected_aggregate_hash: String,
    ) -> RuntimeCatalogConfirmationResult {
        if self.closed.load(Ordering::Acquire) {
            return RuntimeCatalogConfirmationResult {
                confirmation: None,
                artifact: None,
                failure: Some(runtime_catalog_failure("closed", "runtime is closed")),
            };
        }
        let confirmed = match self.catalog.confirm_review(&token) {
            Ok(confirmed) => confirmed,
            Err(error) => {
                return RuntimeCatalogConfirmationResult {
                    confirmation: None,
                    artifact: None,
                    failure: Some(project_catalog_error(error)),
                };
            }
        };
        let confirmation = confirmed.confirmation.clone();
        if confirmation.manifest_author != expected_author
            || confirmation.d_tag.as_deref() != Some(expected_d_tag.as_str())
            || confirmation.aggregate_hash != expected_aggregate_hash
        {
            return RuntimeCatalogConfirmationResult {
                confirmation: None,
                artifact: None,
                failure: Some(runtime_catalog_failure(
                    "confirmation-mismatch",
                    "native confirmation did not match the frozen exact review",
                )),
            };
        }
        let principal = match Principal::new(
            confirmation.manifest_author.clone(),
            expected_d_tag,
            confirmation.aggregate_hash.clone(),
        ) {
            Ok(principal) => principal,
            Err(error) => {
                return RuntimeCatalogConfirmationResult {
                    confirmation: None,
                    artifact: None,
                    failure: Some(runtime_catalog_failure(
                        "unsupported-manifest-identity",
                        error.to_string(),
                    )),
                };
            }
        };
        let artifact = Arc::new(VerifiedArtifact {
            handle: Arc::new(confirmed.into_handle()),
            principal: Some(principal.clone()),
        });
        self.install(Arc::clone(&artifact));
        let installed = self
            .app
            .snapshot()
            .library
            .builds
            .iter()
            .any(|candidate| candidate.build.principal == principal);
        if !installed {
            return RuntimeCatalogConfirmationResult {
                confirmation: None,
                artifact: None,
                failure: Some(runtime_catalog_failure(
                    "install-refused",
                    "the verified exact build was not accepted by the runtime library",
                )),
            };
        }
        RuntimeCatalogConfirmationResult {
            confirmation: Some(confirmation),
            artifact: Some(artifact),
            failure: None,
        }
    }

    /// Reopens one installed exact build.
    ///
    /// Native supplies only the exact library coordinate. Rust checks the
    /// unfiltered persistent installation and returns a handle only when its
    /// signed event, coordinate, aggregate, and capability inventory still
    /// match. If this process already holds the verified handle from a
    /// prior install or reopen, that handle is reused directly. Otherwise
    /// (typically: first reopen after a process restart) this reconstructs
    /// it entirely from local state -- the exact signed manifest event bytes
    /// retained at original install time, re-verified, and the sealed
    /// artifact bytes already committed to the local artifact cache. No
    /// network access, and this never resolves a newer replaceable manifest
    /// as a substitute for the installed event.
    ///
    /// This call is blocking and must be invoked away from a native UI thread.
    pub fn reacquire_installed_artifact(
        &self,
        coordinate: RuntimeExactBuildCoordinate,
    ) -> RuntimeCatalogConfirmationResult {
        if self.closed.load(Ordering::Acquire) {
            return RuntimeCatalogConfirmationResult {
                confirmation: None,
                artifact: None,
                failure: Some(runtime_catalog_failure("closed", "runtime is closed")),
            };
        }
        let principal = match Principal::new(
            coordinate.manifest_author,
            coordinate.d_tag,
            coordinate.aggregate_hash,
        ) {
            Ok(principal) => principal,
            Err(error) => {
                return RuntimeCatalogConfirmationResult {
                    confirmation: None,
                    artifact: None,
                    failure: Some(runtime_catalog_failure(
                        "invalid-exact-build-coordinate",
                        error.to_string(),
                    )),
                };
            }
        };
        let installed = match self.runtime_store.installed_builds() {
            Ok(installed) => installed
                .into_iter()
                .find(|candidate| candidate.principal == principal),
            Err(error) => {
                return RuntimeCatalogConfirmationResult {
                    confirmation: None,
                    artifact: None,
                    failure: Some(runtime_catalog_failure(
                        "installed-library-unavailable",
                        error.to_string(),
                    )),
                };
            }
        };
        let Some(installed) = installed else {
            return RuntimeCatalogConfirmationResult {
                confirmation: None,
                artifact: None,
                failure: Some(runtime_catalog_failure(
                    "not-installed",
                    "the exact build is not present in the runtime library",
                )),
            };
        };
        let retained_handle = { self.artifacts.lock().get(&principal).cloned() };
        let handle = match retained_handle {
            Some(handle) => handle,
            None => match self.reopen_sealed_artifact(&principal, &installed) {
                Ok(handle) => {
                    self.artifacts
                        .lock()
                        .insert(principal.clone(), Arc::clone(&handle));
                    let executable: Arc<dyn ExecutableArtifact> = handle.clone();
                    self.app.dispatch(PlatformCommand::InstallVerified {
                        build: installed.clone(),
                        artifact: executable,
                    });
                    handle
                }
                Err(failure) => {
                    return RuntimeCatalogConfirmationResult {
                        confirmation: None,
                        artifact: None,
                        failure: Some(failure),
                    };
                }
            },
        };
        let artifact = match self.verified_installed_artifact(&installed, handle) {
            Ok(artifact) => artifact,
            Err(failure) => {
                return RuntimeCatalogConfirmationResult {
                    confirmation: None,
                    artifact: None,
                    failure: Some(failure),
                };
            }
        };
        RuntimeCatalogConfirmationResult {
            confirmation: Some(installed_confirmation(&artifact, &installed, Vec::new())),
            artifact: Some(artifact),
            failure: None,
        }
    }

    /// Persists one complete, versioned native workspace definition.
    ///
    /// The boundary refuses partial/unknown schemas before dispatch. The
    /// runtime store performs the replacement atomically and the returned
    /// value is projected from the Rust-owned snapshot, never echoed from the
    /// Swift request.
    pub fn save_workspace(&self, workspace: RuntimeWorkspaceDefinition) -> RuntimeWorkspaceUpdate {
        if self.closed.load(Ordering::Acquire) {
            return RuntimeWorkspaceUpdate {
                accepted: false,
                workspace: None,
                refusal: Some(self.refusal("closed", "runtime is closed")),
            };
        }
        let workspace_id = workspace.workspace_id.clone();
        let record = match workspace_record_from_ffi(workspace) {
            Ok(record) => record,
            Err(detail) => {
                return RuntimeWorkspaceUpdate {
                    accepted: false,
                    workspace: None,
                    refusal: Some(self.workspace_refusal("invalid-workspace", detail)),
                };
            }
        };
        let cursor = self.app.events_after(0).newest_available;
        self.app
            .dispatch(PlatformCommand::SaveWorkspace { workspace: record });
        let saved = self.app.events_after(cursor).events.iter().any(|event| {
            matches!(
                &event.event,
                PlatformEvent::WorkspaceSaved { workspace_id: saved }
                    if saved.as_ref() == workspace_id
            )
        });
        bump_signal(&self.signal);
        if !saved {
            let detail = self.app.snapshot().recent_errors.last().map_or_else(
                || "workspace persistence was refused".to_owned(),
                |error| error.detail.to_string(),
            );
            return RuntimeWorkspaceUpdate {
                accepted: false,
                workspace: None,
                refusal: Some(self.workspace_refusal("workspace-store", detail)),
            };
        }
        let projected = self
            .app
            .snapshot()
            .workspaces
            .iter()
            .find(|candidate| candidate.id.as_ref() == workspace_id)
            .and_then(|candidate| workspace_from_view(candidate).ok());
        match projected {
            Some(workspace) => RuntimeWorkspaceUpdate {
                accepted: true,
                workspace: Some(workspace),
                refusal: None,
            },
            None => RuntimeWorkspaceUpdate {
                accepted: false,
                workspace: None,
                refusal: Some(self.workspace_refusal(
                    "workspace-projection",
                    "saved workspace could not be projected through the versioned schema",
                )),
            },
        }
    }

    /// Validates every durable row before making any restored workspace
    /// visible. Unknown versions or malformed rows refuse the whole restore.
    pub fn restore_workspaces(&self) -> RuntimeWorkspaceRestore {
        if self.closed.load(Ordering::Acquire) {
            return RuntimeWorkspaceRestore {
                accepted: false,
                workspaces: Vec::new(),
                refusal: Some(self.refusal("closed", "runtime is closed")),
            };
        }
        let durable = match self.runtime_store.load_workspaces() {
            Ok(workspaces) => workspaces,
            Err(error) => {
                return RuntimeWorkspaceRestore {
                    accepted: false,
                    workspaces: Vec::new(),
                    refusal: Some(self.workspace_refusal("workspace-store", error.to_string())),
                };
            }
        };
        let mut validated = Vec::with_capacity(durable.len());
        for workspace in &durable {
            match workspace_from_record(workspace) {
                Ok(workspace) => validated.push(workspace),
                Err(detail) => {
                    return RuntimeWorkspaceRestore {
                        accepted: false,
                        workspaces: Vec::new(),
                        refusal: Some(self.workspace_refusal("invalid-workspace", detail)),
                    };
                }
            }
        }
        self.app.dispatch(PlatformCommand::RestoreWorkspaces);
        bump_signal(&self.signal);
        RuntimeWorkspaceRestore {
            accepted: true,
            workspaces: validated,
            refusal: None,
        }
    }

    pub fn verify_artifact(
        &self,
        event_json: Vec<u8>,
        coordinate: ArtifactCoordinate,
    ) -> ArtifactVerification {
        if self.closed.load(Ordering::Acquire) {
            return ArtifactVerification {
                artifact: None,
                refusal: Some(self.refusal("closed", "runtime is closed")),
            };
        }
        if event_json.len() > self.maximum_manifest_bytes {
            return ArtifactVerification {
                artifact: None,
                refusal: Some(self.refusal(
                    "manifest-too-large",
                    format!(
                        "manifest has {} bytes; the maximum is {}",
                        event_json.len(),
                        self.maximum_manifest_bytes
                    ),
                )),
            };
        }
        let coordinate = match map_coordinate(coordinate) {
            Ok(coordinate) => coordinate,
            Err(detail) => {
                return ArtifactVerification {
                    artifact: None,
                    refusal: Some(self.refusal("invalid-coordinate", detail)),
                };
            }
        };
        let verifier = match ManifestEventVerifier::new(ManifestEventLimits {
            maximum_event_bytes: self.maximum_manifest_bytes,
            ..ManifestEventLimits::default()
        }) {
            Ok(verifier) => verifier,
            Err(error) => {
                return ArtifactVerification {
                    artifact: None,
                    refusal: Some(self.refusal("invalid-limits", error.to_string())),
                };
            }
        };
        let policy = match ArtifactSourcePolicy::manifest_https_only(self.maximum_blob_sources) {
            Ok(policy) => policy,
            Err(error) => {
                return ArtifactVerification {
                    artifact: None,
                    refusal: Some(self.refusal("invalid-source-policy", error.to_string())),
                };
            }
        };
        let resolver = match SignedArtifactResolver::new(
            verifier,
            self.artifact_limits,
            policy,
            &self.artifact_source,
            &self.artifact_cache,
        ) {
            Ok(resolver) => resolver,
            Err(error) => {
                return ArtifactVerification {
                    artifact: None,
                    refusal: Some(self.refusal("invalid-resolver", error.to_string())),
                };
            }
        };
        match resolver.resolve_json(&event_json, &coordinate) {
            Ok(handle) => {
                let handle = Arc::new(handle);
                let principal = handle.index().d_tag().and_then(|d_tag| {
                    Principal::new(
                        handle.index().author().as_str(),
                        d_tag,
                        handle.index().aggregate().as_str(),
                    )
                    .ok()
                });
                ArtifactVerification {
                    artifact: Some(Arc::new(VerifiedArtifact { handle, principal })),
                    refusal: None,
                }
            }
            Err(error) => ArtifactVerification {
                artifact: None,
                refusal: Some(self.refusal("artifact-verification", error.to_string())),
            },
        }
    }

    pub fn install(&self, artifact: Arc<VerifiedArtifact>) {
        let Some(principal) = artifact.principal.clone() else {
            self.record_refusal(
                "unsupported-manifest-identity",
                "only verified named manifests currently mint exact-build principals",
            );
            return;
        };
        let metadata = serde_json::json!({
            "event_id": artifact.handle.index().event_id().as_str(),
            "kind": artifact.handle.index().kind(),
            "mode": match artifact.handle.index().mode() {
                ArtifactMode::SingleFile => "single-file",
                ArtifactMode::ExternalAssets => "external-assets",
            },
            "paths": artifact.handle.index().entries().len(),
            // Retained so `reacquire_installed_artifact` can reopen this
            // exact build after a process restart by re-verifying the same
            // signed event against the sealed cache, with no network fetch
            // and no risk of silently accepting a since-republished event.
            "signed_event_b64": base64::engine::general_purpose::STANDARD
                .encode(artifact.handle.manifest().signed_event_json()),
        });
        let metadata =
            match BoundedJson::from_value(&metadata, MAXIMUM_INSTALLED_MANIFEST_METADATA_BYTES) {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.record_refusal("manifest-metadata", error.to_string());
                    return;
                }
            };
        let title: Arc<str> = Arc::from(
            artifact
                .handle
                .manifest()
                .title()
                .unwrap_or("Untitled napplet"),
        );
        let capability_requests = match installation_capability_requests(&artifact) {
            Ok(requests) => requests,
            Err(error) => {
                self.record_refusal("invalid-capability-request", error);
                return;
            }
        };
        self.artifacts
            .lock()
            .insert(principal.clone(), Arc::clone(&artifact.handle));
        let executable: Arc<dyn ExecutableArtifact> = artifact.handle.clone();
        self.app.dispatch(PlatformCommand::InstallVerified {
            build: InstalledBuild {
                principal: principal.clone(),
                title,
                manifest_metadata: metadata,
                capability_requests,
            },
            artifact: executable,
        });
        self.grant_demo_permissions(
            principal.manifest_author(),
            principal.d_tag(),
            principal.aggregate_hash(),
        );
        bump_signal(&self.signal);
    }

    /// The explicit demo mode is intentionally permissive so a locally
    /// verified network napplet can be rendered and exercised end-to-end.
    /// Interactive production profiles still require the normal exact-build
    /// permission review.
    fn grant_demo_permissions(&self, author: &str, d_tag: &str, aggregate_hash: &str) {
        if self.permission_mode != RuntimePermissionMode::DemoPinnedGoodMorning {
            return;
        }
        let Ok(principal) = Principal::new(author, d_tag, aggregate_hash) else {
            return;
        };
        let Ok(review) = self.app.permission_review(&principal) else {
            return;
        };
        let decisions = review
            .capabilities
            .into_iter()
            .map(|capability| PermissionDecision {
                capability: capability.capability,
                decision: capability
                    .decision_options
                    .into_iter()
                    .find(|option| {
                        option.valid && option.decision == GrantDecision::AllowExactBuild
                    })
                    .map_or(GrantDecision::Denied, |option| option.decision),
            })
            .collect::<Vec<_>>();
        if !decisions.is_empty() {
            self.app.dispatch(PlatformCommand::ApplyPermissionBatch {
                principal: principal.clone(),
                decisions,
            });
        }
    }

    fn grant_demo_permissions_for_installed_builds(&self) {
        if self.permission_mode != RuntimePermissionMode::DemoPinnedGoodMorning {
            return;
        }
        let principals = self
            .app
            .snapshot()
            .library
            .builds
            .iter()
            .map(|view| view.build.principal.clone())
            .collect::<Vec<_>>();
        for principal in principals {
            self.grant_demo_permissions(
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
            );
        }
    }

    /// Applies the Rust-owned, finite installed-library filter. The resulting
    /// bounded view is emitted in `RuntimeSnapshot.installed_library`.
    pub fn set_library_filter(&self, query: String) {
        self.app.dispatch(PlatformCommand::SetLibraryFilter {
            query: Arc::from(query),
        });
        bump_signal(&self.signal);
    }

    /// Removes only runtime-owned state for one exact build. NMP canonical
    /// facts and durable receipts are unreachable from this command, and
    /// artifact-cache bytes remain until the artifact owner exposes a safe
    /// exact-build deletion API.
    pub fn uninstall_build(&self, coordinate: RuntimeExactBuildCoordinate) {
        let Some(principal) = self.library_principal(coordinate) else {
            return;
        };
        self.app.dispatch(PlatformCommand::Uninstall {
            principal: principal.clone(),
            cleanup: UninstallCleanupPolicy::RuntimeOwnedExactBuildState,
        });
        let remains_installed = self
            .app
            .snapshot()
            .library
            .builds
            .iter()
            .any(|candidate| candidate.build.principal == principal);
        if !remains_installed {
            self.artifacts.lock().remove(&principal);
        }
        bump_signal(&self.signal);
    }

    /// Assigns one installed exact build to an existing durable workspace.
    /// The runtime store validates both sides and enforces assignment bounds.
    pub fn assign_build_to_workspace(
        &self,
        workspace_id: String,
        coordinate: RuntimeExactBuildCoordinate,
    ) {
        if let Err(detail) = validate_workspace_name("workspace_id", &workspace_id) {
            self.record_refusal("invalid-workspace-assignment", detail);
            return;
        }
        let Some(principal) = self.library_principal(coordinate) else {
            return;
        };
        self.app.dispatch(PlatformCommand::AssignWorkspaceBuild {
            workspace_id: Arc::from(workspace_id),
            principal,
        });
        bump_signal(&self.signal);
    }

    /// Clears one exact build assignment without deleting the workspace,
    /// installation, artifact bytes, NMP facts, or retained receipt ids.
    pub fn clear_build_from_workspace(
        &self,
        workspace_id: String,
        coordinate: RuntimeExactBuildCoordinate,
    ) {
        if let Err(detail) = validate_workspace_name("workspace_id", &workspace_id) {
            self.record_refusal("invalid-workspace-assignment", detail);
            return;
        }
        let Some(principal) = self.library_principal(coordinate) else {
            return;
        };
        self.app.dispatch(PlatformCommand::RemoveWorkspaceBuild {
            workspace_id: Arc::from(workspace_id),
            principal,
        });
        bump_signal(&self.signal);
    }

    /// Returns one bounded Rust-owned review for an installed exact build.
    /// This never grants or launches the napplet.
    pub fn permission_review(
        &self,
        coordinate: RuntimeExactBuildCoordinate,
    ) -> RuntimePermissionReviewResult {
        if self.closed.load(Ordering::Acquire) {
            return RuntimePermissionReviewResult {
                review: None,
                refusal: Some(self.refusal("closed", "runtime is closed")),
            };
        }
        let principal = match Principal::new(
            coordinate.manifest_author,
            coordinate.d_tag,
            coordinate.aggregate_hash,
        ) {
            Ok(principal) => principal,
            Err(error) => {
                let refusal =
                    self.workspace_refusal("invalid-exact-build-coordinate", error.to_string());
                return RuntimePermissionReviewResult {
                    review: None,
                    refusal: Some(refusal),
                };
            }
        };
        match self.app.permission_review(&principal) {
            Ok(review) => RuntimePermissionReviewResult {
                review: Some(project_permission_review(review)),
                refusal: None,
            },
            Err(error) => {
                let refusal = self.workspace_refusal("permission-review", error.to_string());
                RuntimePermissionReviewResult {
                    review: None,
                    refusal: Some(refusal),
                }
            }
        }
    }

    /// Applies one complete exact-build decision set atomically in Rust.
    /// Success never launches the napplet; launch remains a separate command.
    pub fn apply_permission_decisions(
        &self,
        batch: RuntimePermissionDecisionBatch,
    ) -> RuntimePermissionBatchUpdate {
        if self.closed.load(Ordering::Acquire) {
            return RuntimePermissionBatchUpdate {
                applied: false,
                review: None,
                refusal: Some(self.refusal("closed", "runtime is closed")),
            };
        }
        if batch.decisions.is_empty() || batch.decisions.len() > MAXIMUM_PERMISSION_DECISIONS {
            let refusal = self.workspace_refusal(
                "invalid-permission-batch",
                format!(
                    "permission batch has {} decisions; the allowed range is 1..={MAXIMUM_PERMISSION_DECISIONS}",
                    batch.decisions.len()
                ),
            );
            return RuntimePermissionBatchUpdate {
                applied: false,
                review: None,
                refusal: Some(refusal),
            };
        }
        let principal = match Principal::new(
            batch.coordinate.manifest_author,
            batch.coordinate.d_tag,
            batch.coordinate.aggregate_hash,
        ) {
            Ok(principal) => principal,
            Err(error) => {
                let refusal =
                    self.workspace_refusal("invalid-exact-build-coordinate", error.to_string());
                return RuntimePermissionBatchUpdate {
                    applied: false,
                    review: None,
                    refusal: Some(refusal),
                };
            }
        };
        let mut domains = BTreeSet::new();
        let mut decisions = Vec::with_capacity(batch.decisions.len());
        for selection in batch.decisions {
            let capability = match Capability::new(selection.domain) {
                Ok(capability) => capability,
                Err(error) => {
                    let refusal =
                        self.workspace_refusal("invalid-permission-domain", error.to_string());
                    return RuntimePermissionBatchUpdate {
                        applied: false,
                        review: None,
                        refusal: Some(refusal),
                    };
                }
            };
            if !domains.insert(capability.clone()) {
                let refusal = self.workspace_refusal(
                    "duplicate-permission-domain",
                    format!("permission batch repeats capability {capability}"),
                );
                return RuntimePermissionBatchUpdate {
                    applied: false,
                    review: None,
                    refusal: Some(refusal),
                };
            }
            decisions.push(PermissionDecision {
                capability,
                decision: grant_decision(selection.decision),
            });
        }

        let cursor = self.app.events_after(0).newest_available;
        self.app.dispatch(PlatformCommand::ApplyPermissionBatch {
            principal: principal.clone(),
            decisions,
        });
        bump_signal(&self.signal);
        let events = self.app.events_after(cursor);
        let applied = events.events.iter().any(|event| {
            matches!(
                &event.event,
                PlatformEvent::PermissionBatchApplied {
                    principal: applied,
                    ..
                } if applied == &principal
            )
        });
        if !applied {
            let refusal = events
                .events
                .iter()
                .rev()
                .find_map(|event| match &event.event {
                    PlatformEvent::Refused(fact) if fact.principal.as_ref() == Some(&principal) => {
                        Some(RuntimeRefusal {
                            code: format!("{:?}", fact.code).to_ascii_lowercase(),
                            detail: fact.detail.to_string(),
                            occurred_at_millis: fact.occurred_at_millis,
                        })
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    self.refusal(
                        "permission-batch-refused",
                        "the runtime refused the permission batch without a matching outcome",
                    )
                });
            return RuntimePermissionBatchUpdate {
                applied: false,
                review: None,
                refusal: Some(refusal),
            };
        }
        match self.app.permission_review(&principal) {
            Ok(review) => RuntimePermissionBatchUpdate {
                applied: true,
                review: Some(project_permission_review(review)),
                refusal: None,
            },
            Err(error) => RuntimePermissionBatchUpdate {
                applied: true,
                review: None,
                refusal: Some(
                    self.workspace_refusal("permission-review-after-apply", error.to_string()),
                ),
            },
        }
    }

    pub fn set_grant(
        &self,
        artifact: Arc<VerifiedArtifact>,
        capability: String,
        sensitivity: RuntimeSensitivity,
        decision: RuntimeGrantDecision,
    ) {
        let Some(principal) = artifact.principal.clone() else {
            self.record_refusal(
                "unsupported-manifest-identity",
                "grant target has no exact-build principal",
            );
            return;
        };
        let capability = match Capability::new(capability) {
            Ok(capability) => capability,
            Err(error) => {
                self.record_refusal("invalid-capability", error.to_string());
                return;
            }
        };
        self.app.dispatch(PlatformCommand::SetGrant {
            principal,
            capability,
            sensitivity: match sensitivity {
                RuntimeSensitivity::Ordinary => Sensitivity::Ordinary,
                RuntimeSensitivity::Sensitive => Sensitivity::Sensitive,
            },
            decision: match decision {
                RuntimeGrantDecision::Denied => GrantDecision::Denied,
                RuntimeGrantDecision::AskEveryTime => GrantDecision::AskEveryTime,
                RuntimeGrantDecision::AllowSession => GrantDecision::AllowSession,
                RuntimeGrantDecision::AllowExactBuild => GrantDecision::AllowExactBuild,
            },
        });
        bump_signal(&self.signal);
    }

    pub fn revoke(&self, artifact: Arc<VerifiedArtifact>, capability: String) {
        let Some(principal) = artifact.principal.clone() else {
            self.record_refusal(
                "unsupported-manifest-identity",
                "revocation target has no exact-build principal",
            );
            return;
        };
        let capability = match Capability::new(capability) {
            Ok(capability) => capability,
            Err(error) => {
                self.record_refusal("invalid-capability", error.to_string());
                return;
            }
        };
        self.app.dispatch(PlatformCommand::Revoke {
            principal,
            capability,
        });
        bump_signal(&self.signal);
    }

    pub fn launch(&self, artifact: Arc<VerifiedArtifact>, profile: RuntimeExecutionProfile) {
        let capability_requests = match installation_capability_requests(&artifact) {
            Ok(requests) => requests,
            Err(error) => {
                self.record_refusal("invalid-capability-request", error);
                return;
            }
        };
        if capability_requests.len() > self.maximum_command_items {
            self.record_refusal(
                "required-domain-capacity",
                format!(
                    "verified capability profile has {} domains; the maximum is {}",
                    capability_requests.len(),
                    self.maximum_command_items
                ),
            );
            return;
        }
        let Some(principal) = artifact.principal.clone() else {
            self.record_refusal(
                "unsupported-manifest-identity",
                "launch target has no exact-build principal",
            );
            return;
        };
        let mut domains = BTreeSet::new();
        for request in capability_requests {
            if request.requirement == CapabilityRequirement::Required {
                domains.insert(request.capability);
            }
        }
        self.app.dispatch(PlatformCommand::Launch {
            principal,
            profile: map_profile(profile),
            required_domains: domains,
        });
        bump_signal(&self.signal);
    }

    pub fn stop(&self, session_id: u64) {
        self.app.dispatch(PlatformCommand::Stop {
            session: SessionId(session_id),
        });
        bump_signal(&self.signal);
    }

    /// Suspends one current session listed by its installed-build projection.
    /// Lifecycle policy and stale-session refusal remain inside RuntimeApp.
    pub fn suspend(&self, session_id: u64) {
        self.app.dispatch(PlatformCommand::Suspend {
            session: SessionId(session_id),
        });
        bump_signal(&self.signal);
    }

    /// Resumes one current suspended session. RuntimeApp validates the typed
    /// lifecycle transition and projects any refusal through state/events.
    pub fn resume(&self, session_id: u64) {
        self.app.dispatch(PlatformCommand::Resume {
            session: SessionId(session_id),
        });
        bump_signal(&self.signal);
    }

    pub fn crash(&self, session_id: u64, reason: String) {
        if reason.len() > 1_024 || reason.chars().any(char::is_control) {
            self.record_refusal(
                "invalid-crash-reason",
                "crash reason must be control-free and at most 1024 bytes",
            );
            return;
        }
        self.app.dispatch(PlatformCommand::Crash {
            session: SessionId(session_id),
            reason: Arc::from(reason),
        });
        bump_signal(&self.signal);
    }

    /// Resolves one Rust-retained provider write proposal. Native supplies
    /// only the bounded operation id and decision; the exact principal,
    /// account, correlation, and draft remain inside RuntimeApp.
    pub fn decide_provider_write(&self, operation_id: u64, approve: bool) {
        if operation_id == 0 {
            self.record_refusal(
                "invalid-provider-operation",
                "provider operation identifiers are positive",
            );
            return;
        }
        self.app.dispatch(PlatformCommand::DecideProviderWrite {
            operation: ProviderOperationId(operation_id),
            approve,
        });
        bump_signal(&self.signal);
    }

    pub fn mapped_envelope(&self, session_id: u64, bytes: Vec<u8>) {
        if bytes.len() > self.maximum_envelope_bytes {
            self.record_refusal(
                "envelope-too-large",
                format!(
                    "mapped envelope has {} bytes; the maximum is {}",
                    bytes.len(),
                    self.maximum_envelope_bytes
                ),
            );
            return;
        }
        self.app.dispatch(PlatformCommand::MappedEnvelope {
            session: SessionId(session_id),
            bytes: Arc::from(bytes),
        });
        bump_signal(&self.signal);
    }

    pub fn read_verified(
        &self,
        session_id: u64,
        logical_path: String,
        maximum_bytes: u64,
    ) -> VerifiedRead {
        if logical_path.len() > self.maximum_command_string_bytes {
            return VerifiedRead::Refused {
                refusal: self.refusal(
                    "logical-path-too-large",
                    format!(
                        "logical path exceeds {} bytes",
                        self.maximum_command_string_bytes
                    ),
                ),
            };
        }
        let maximum_bytes = match usize::try_from(maximum_bytes) {
            Ok(value) if value > 0 && value <= self.maximum_verified_read_bytes => value,
            _ => {
                return VerifiedRead::Refused {
                    refusal: self.refusal(
                        "invalid-read-limit",
                        format!(
                            "read limit must be 1..={}",
                            self.maximum_verified_read_bytes
                        ),
                    ),
                };
            }
        };
        let principal = self
            .app
            .snapshot()
            .sessions
            .iter()
            .find(|session| session.id == SessionId(session_id))
            .map(|session| session.principal.clone());
        let Some(principal) = principal else {
            return VerifiedRead::Refused {
                refusal: self.refusal("unknown-session", "no active mapped session"),
            };
        };
        let Some(artifact) = self.artifacts.lock().get(&principal).cloned() else {
            return VerifiedRead::Refused {
                refusal: self.refusal("unknown-artifact", "session artifact is not retained"),
            };
        };
        let Some(expected) = artifact
            .index()
            .entries()
            .find(|entry| entry.path() == logical_path)
            .map(|entry| entry.sha256().as_str().to_owned())
        else {
            return VerifiedRead::Refused {
                refusal: self.refusal(
                    "verified-read",
                    "logical path is not present in the sealed artifact index",
                ),
            };
        };
        match artifact.read_verified(&logical_path, maximum_bytes) {
            Ok(bytes) => VerifiedRead::Bytes {
                media_type: media_type_for(&logical_path).to_owned(),
                sha256: expected,
                bytes,
            },
            Err(error) => VerifiedRead::Refused {
                refusal: self.refusal("verified-read", error.to_string()),
            },
        }
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.project_snapshot(&self.app.snapshot())
    }

    /// The latest NMP-owned relay and wire-subscription read-out. It is only
    /// refreshed while an observation is open; check `observing`.
    pub fn relay_diagnostics(&self) -> RuntimeRelayDiagnosticsSnapshot {
        self.diagnostics.snapshot()
    }

    /// Open the NMP diagnostics observation for as long as the returned handle
    /// lives. The current read-out is delivered synchronously on registration.
    pub fn observe_relay_diagnostics(
        &self,
        observer: Box<dyn RuntimeRelayDiagnosticsObserver>,
    ) -> RuntimeRelayDiagnosticsObservationStart {
        match self.diagnostics.observe(Arc::from(observer)) {
            Ok(observation) => RuntimeRelayDiagnosticsObservationStart {
                observation: Some(observation),
                refusal: None,
            },
            Err(error) => RuntimeRelayDiagnosticsObservationStart {
                observation: None,
                refusal: Some(self.refusal("relay-diagnostics-observe", error.to_string())),
            },
        }
    }

    pub fn observe(self: Arc<Self>, observer: Box<dyn RuntimeObserver>) -> ObservationStart {
        let admitted = self
            .observers
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.maximum_observers).then_some(active + 1)
            });
        if admitted.is_err() {
            return ObservationStart {
                observation: None,
                refusal: Some(self.refusal(
                    "observer-capacity",
                    format!("observer capacity {} is full", self.maximum_observers),
                )),
            };
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let handle = Arc::new(RuntimeObservation {
            stopped: Arc::clone(&stopped),
            signal: self.signal.clone(),
        });
        let controller = Arc::clone(&self);
        let observer: Arc<dyn RuntimeObserver> = Arc::from(observer);
        let observers = Arc::clone(&self.observers);
        let mut app_observer = self.app.observe();
        let mut signal = self.signal.subscribe();
        let mut catalog_signal = self.catalog.subscribe();
        let spawn = thread::Builder::new()
            .name("runtime-ffi-observer".to_owned())
            .spawn(move || {
                let _permit = ObserverPermit(observers);
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    controller.record_refusal(
                        "observer-thread",
                        "could not construct the observation runtime",
                    );
                    return;
                };
                runtime.block_on(async move {
                    let mut event_cursor = 0_u64;
                    loop {
                        if stopped.load(Ordering::Acquire) {
                            break;
                        }
                        let batch = controller.app.events_after(event_cursor);
                        event_cursor = batch.newest_available;
                        observer.update(RuntimeObservationFrame {
                            snapshot: controller.project_snapshot(&app_observer.latest()),
                            catalog: controller.catalog.feed_snapshot(None),
                            events: batch
                                .events
                                .into_iter()
                                .map(|event| project_event(event.sequence, &event.event))
                                .collect(),
                            oldest_available_event: batch.oldest_available,
                            newest_available_event: batch.newest_available,
                            event_cursor_was_stale: batch.cursor_was_stale,
                        });
                        tokio::select! {
                            changed = app_observer.changed() => {
                                if changed.is_err() {
                                    break;
                                }
                            }
                            changed = signal.changed() => {
                                if changed.is_err() {
                                    break;
                                }
                            }
                            changed = catalog_signal.changed() => {
                                if changed.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            });
        if let Err(error) = spawn {
            handle.stop();
            self.observers.fetch_sub(1, Ordering::AcqRel);
            return ObservationStart {
                observation: None,
                refusal: Some(self.refusal("observer-thread", error.to_string())),
            };
        }
        ObservationStart {
            observation: Some(handle),
            refusal: None,
        }
    }

    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.catalog.close();
            self.diagnostics.close();
            self.app.dispatch(PlatformCommand::Close);
            self.data_plane.close();
            bump_signal(&self.signal);
        }
    }
}

impl RuntimeController {
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
        let requests = installation_capability_requests(&artifact)
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

fn theme_from_appearance(appearance: NativeAppearanceSnapshot) -> Result<ThemeSnapshot, String> {
    let background = match (
        appearance.dark,
        appearance.increased_contrast,
        appearance.reduced_transparency,
    ) {
        (true, true, _) | (true, _, true) => "#000000",
        (true, false, false) => "#1c1c1e",
        (false, true, _) | (false, _, true) => "#ffffff",
        (false, false, false) => "#f5f5f7",
    };
    let text = if appearance.dark {
        "#ffffff"
    } else {
        "#000000"
    };
    let primary = format!(
        "#{:02x}{:02x}{:02x}",
        appearance.accent_red, appearance.accent_green, appearance.accent_blue
    );
    let mode = if appearance.dark { "Dark" } else { "Light" };
    let contrast = if appearance.increased_contrast {
        " High Contrast"
    } else {
        ""
    };
    let transparency = if appearance.reduced_transparency {
        " Reduced Transparency"
    } else {
        ""
    };
    ThemeSnapshot::from_value(
        &serde_json::json!({
            "colors": {
                "background": background,
                "text": text,
                "primary": primary,
            },
            "title": format!("macOS {mode}{contrast}{transparency}"),
        }),
        ThemeProviderLimits::default(),
    )
    .map_err(|error| error.to_string())
}

fn map_coordinate(coordinate: ArtifactCoordinate) -> Result<ManifestCoordinate, String> {
    match coordinate {
        ArtifactCoordinate::Snapshot { event_id, author } => {
            ManifestCoordinate::snapshot(&event_id, &author)
        }
        ArtifactCoordinate::Root { author } => ManifestCoordinate::root(&author),
        ArtifactCoordinate::Named { author, d_tag } => ManifestCoordinate::named(&author, &d_tag),
    }
    .map_err(|error| error.to_string())
}

fn parse_catalog_coordinate(value: &str) -> Result<ManifestCoordinate, String> {
    if value.is_empty()
        || value.len() > 2_048
        || value.chars().any(char::is_control)
        || value.trim() != value
    {
        return Err(
            "coordinate must be 1..=2048 UTF-8 bytes without controls or surrounding whitespace"
                .to_owned(),
        );
    }
    let mut fields = value.splitn(3, ':');
    let kind = fields.next().unwrap_or_default();
    let first = fields
        .next()
        .ok_or_else(|| "coordinate is missing its author or event identifier".to_owned())?;
    let second = fields.next();
    let coordinate = match (kind, second) {
        ("5129", Some(author)) => ManifestCoordinate::snapshot(first, author),
        ("15129", None) => ManifestCoordinate::root(first),
        ("35129", Some(d_tag)) => ManifestCoordinate::named(first, d_tag),
        ("5129", None) => {
            return Err("snapshot coordinate must be 5129:event-id:author".to_owned());
        }
        ("35129", None) => {
            return Err("named coordinate must be 35129:author:d-tag".to_owned());
        }
        _ => {
            return Err(
                "supported coordinates are 5129:event-id:author, 15129:author, and 35129:author:d-tag"
                    .to_owned(),
            );
        }
    };
    coordinate.map_err(|error| error.to_string())
}

fn runtime_catalog_failure(
    code: impl Into<String>,
    detail: impl Into<String>,
) -> RuntimeCatalogFailure {
    RuntimeCatalogFailure {
        code: code.into(),
        detail: detail.into(),
        provenance: Vec::new(),
    }
}

fn map_profile(profile: RuntimeExecutionProfile) -> ExecutionProfile {
    match profile {
        RuntimeExecutionProfile::Legacy => ExecutionProfile::Legacy,
        RuntimeExecutionProfile::Renderer => ExecutionProfile::Renderer,
        RuntimeExecutionProfile::Hybrid => ExecutionProfile::Hybrid,
    }
}

fn grant_decision(decision: RuntimeGrantDecision) -> GrantDecision {
    match decision {
        RuntimeGrantDecision::Denied => GrantDecision::Denied,
        RuntimeGrantDecision::AskEveryTime => GrantDecision::AskEveryTime,
        RuntimeGrantDecision::AllowSession => GrantDecision::AllowSession,
        RuntimeGrantDecision::AllowExactBuild => GrantDecision::AllowExactBuild,
    }
}

fn project_grant_decision(decision: GrantDecision) -> RuntimePermissionExistingDecision {
    match decision {
        GrantDecision::Denied => RuntimePermissionExistingDecision::Denied,
        GrantDecision::AskEveryTime => RuntimePermissionExistingDecision::AskEveryTime,
        GrantDecision::AllowSession => RuntimePermissionExistingDecision::AllowSession,
        GrantDecision::AllowExactBuild => RuntimePermissionExistingDecision::AllowExactBuild,
        GrantDecision::Managed => RuntimePermissionExistingDecision::Managed,
    }
}

fn project_requested_grant_decision(decision: GrantDecision) -> Option<RuntimeGrantDecision> {
    match decision {
        GrantDecision::Denied => Some(RuntimeGrantDecision::Denied),
        GrantDecision::AskEveryTime => Some(RuntimeGrantDecision::AskEveryTime),
        GrantDecision::AllowSession => Some(RuntimeGrantDecision::AllowSession),
        GrantDecision::AllowExactBuild => Some(RuntimeGrantDecision::AllowExactBuild),
        GrantDecision::Managed => None,
    }
}

fn project_permission_review(review: PermissionReviewView) -> RuntimePermissionReviewSnapshot {
    // A Required capability with no registered provider on this runtime
    // build can never receive a decision (permission_decision_policy forces
    // it to Denied with every option invalid), so it must not permanently
    // block launch the way a genuinely available-but-denied capability does.
    // Launch drops such domains instead of injecting them; see
    // `RuntimeApp::launch`.
    let launch_permitted = review.capabilities.iter().all(|capability| {
        capability.requirement != CapabilityRequirement::Required
            || !matches!(
                capability.platform_availability,
                PermissionPlatformAvailability::Available
            )
            || capability.current_decision.allows_without_prompt()
    });
    RuntimePermissionReviewSnapshot {
        coordinate: RuntimeExactBuildCoordinate {
            manifest_author: review.principal.manifest_author().to_owned(),
            d_tag: review.principal.d_tag().to_owned(),
            aggregate_hash: review.principal.aggregate_hash().to_owned(),
        },
        title: review.title.to_string(),
        capabilities: review
            .capabilities
            .into_iter()
            .map(|capability| RuntimePermissionCapabilitySnapshot {
                domain: capability.capability.as_str().to_owned(),
                requirement: match capability.requirement {
                    CapabilityRequirement::Required => RuntimePermissionRequirement::Required,
                    CapabilityRequirement::Optional => RuntimePermissionRequirement::Optional,
                },
                sensitivity: match capability.sensitivity {
                    Some(Sensitivity::Ordinary) => RuntimePermissionSensitivity::Ordinary,
                    Some(Sensitivity::Sensitive) => RuntimePermissionSensitivity::Sensitive,
                    None => RuntimePermissionSensitivity::Unknown,
                },
                dependencies: capability
                    .dependencies
                    .into_iter()
                    .map(|dependency| dependency.as_str().to_owned())
                    .collect(),
                platform_availability: match capability.platform_availability {
                    PermissionPlatformAvailability::Available => {
                        RuntimePermissionPlatformAvailability::Available
                    }
                    PermissionPlatformAvailability::Unknown { reason } => {
                        RuntimePermissionPlatformAvailability::Unknown {
                            reason: reason.to_string(),
                        }
                    }
                    PermissionPlatformAvailability::Unavailable { reason } => {
                        RuntimePermissionPlatformAvailability::Unavailable {
                            reason: reason.to_string(),
                        }
                    }
                },
                existing_decision: project_grant_decision(capability.current_decision),
                requested_decision: capability
                    .requested_decision
                    .and_then(project_requested_grant_decision),
                decision_options: capability
                    .decision_options
                    .into_iter()
                    .filter_map(|option| {
                        project_requested_grant_decision(option.decision).map(|decision| {
                            RuntimePermissionDecisionOption {
                                decision,
                                valid: option.valid,
                                invalid_reason: option
                                    .invalid_reason
                                    .map(|reason| reason.to_string()),
                            }
                        })
                    })
                    .collect(),
            })
            .collect(),
        launch_permitted,
    }
}

fn project_profile(profile: ExecutionProfile) -> RuntimeExecutionProfile {
    match profile {
        ExecutionProfile::Legacy => RuntimeExecutionProfile::Legacy,
        ExecutionProfile::Renderer => RuntimeExecutionProfile::Renderer,
        ExecutionProfile::Hybrid => RuntimeExecutionProfile::Hybrid,
    }
}

fn project_event(sequence: u64, event: &PlatformEvent) -> RuntimeEvent {
    let kind = match event {
        PlatformEvent::Installed { .. } => "installed",
        PlatformEvent::LibraryFilterChanged { .. } => "library-filter-changed",
        PlatformEvent::Uninstalled { .. } => "uninstalled",
        PlatformEvent::GrantChanged { .. } => "grant-changed",
        PlatformEvent::PermissionBatchApplied { .. } => "permission-batch-applied",
        PlatformEvent::SessionChanged(_) => "session-changed",
        PlatformEvent::EnvelopeHandled {
            session, response, ..
        } => {
            return RuntimeEvent {
                sequence,
                kind: "envelope-handled".to_owned(),
                detail: format!("{event:?}"),
                session_id: Some(session.0),
                response_json: response.as_ref().map(|value| value.as_str().to_owned()),
            };
        }
        PlatformEvent::EnvelopeIgnored { .. } => "envelope-ignored",
        PlatformEvent::ProviderOperationFinished { .. } => "provider-operation-finished",
        PlatformEvent::ProviderPush {
            session, envelope, ..
        } => {
            return RuntimeEvent {
                sequence,
                kind: "provider-push".to_owned(),
                detail: format!("{event:?}"),
                session_id: Some(session.0),
                response_json: Some(envelope.as_str().to_owned()),
            };
        }
        PlatformEvent::ProviderPushLaneClosed { session, .. } => {
            return RuntimeEvent {
                sequence,
                kind: "provider-push-lane-closed".to_owned(),
                detail: format!("{event:?}"),
                session_id: Some(session.0),
                response_json: None,
            };
        }
        PlatformEvent::BindingOpened { .. } => "binding-opened",
        PlatformEvent::BindingClosed { .. } => "binding-closed",
        PlatformEvent::WriteAccepted { .. } => "write-accepted",
        PlatformEvent::WorkspaceSaved { .. } => "workspace-saved",
        PlatformEvent::WorkspaceRestored { .. } => "workspace-restored",
        PlatformEvent::WorkspaceAssignmentChanged { .. } => "workspace-assignment-changed",
        PlatformEvent::ReceiptReattached { .. } => "receipt-reattached",
        PlatformEvent::ReceiptNotFound { .. } => "receipt-not-found",
        PlatformEvent::Refused(_) => "refused",
        PlatformEvent::Closed => "closed",
    };
    RuntimeEvent {
        sequence,
        kind: kind.to_owned(),
        detail: format!("{event:?}"),
        session_id: None,
        response_json: None,
    }
}

fn local_account_handle(handle: RuntimeAccountHandle) -> LocalAccountHandle {
    LocalAccountHandle {
        installation_id: handle.installation_id,
        account: nmp_native_runtime_core::AccountRef(Arc::from(handle.public_key)),
        kind: match handle.kind {
            RuntimeAccountKind::LocalSigner => LocalAccountKind::LocalSigner,
            RuntimeAccountKind::ReadOnly => LocalAccountKind::ReadOnly,
        },
    }
}

fn project_account_handle(handle: LocalAccountHandle) -> RuntimeAccountHandle {
    RuntimeAccountHandle {
        installation_id: handle.installation_id,
        public_key: handle.account.0.to_string(),
        kind: match handle.kind {
            LocalAccountKind::LocalSigner => RuntimeAccountKind::LocalSigner,
            LocalAccountKind::ReadOnly => RuntimeAccountKind::ReadOnly,
        },
    }
}

fn project_account_snapshot(snapshot: LocalAccountSnapshot) -> RuntimeAccountSnapshot {
    RuntimeAccountSnapshot {
        generation: snapshot.identity.generation,
        active_public_key: snapshot
            .identity
            .account
            .map(|account| account.0.to_string()),
        local_accounts: snapshot
            .installations
            .into_iter()
            .map(project_account_handle)
            .collect(),
    }
}

fn project_account_error(error: AccountLifecycleError) -> RuntimeAccountFailure {
    match error {
        AccountLifecycleError::Closed => RuntimeAccountFailure::Closed,
        AccountLifecycleError::InvalidSecretKey => RuntimeAccountFailure::InvalidSecretKey,
        AccountLifecycleError::InvalidPublicKey => RuntimeAccountFailure::InvalidPublicKey,
        AccountLifecycleError::Nip05ResolutionUnavailable => {
            RuntimeAccountFailure::Nip05ResolutionUnavailable
        }
        AccountLifecycleError::Capacity { limit } => RuntimeAccountFailure::Capacity {
            limit: limit as u64,
        },
        AccountLifecycleError::InstanceExhausted => RuntimeAccountFailure::InstanceExhausted,
        AccountLifecycleError::StaleInstallation => RuntimeAccountFailure::StaleInstallation,
        AccountLifecycleError::Failed { reason } => RuntimeAccountFailure::Failed {
            reason: reason.to_string(),
        },
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoredWorkspaceAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoredWorkspaceRole {
    Feed,
    Detail,
    Profile,
    Thread,
    Composer,
    MediaPlayer,
    ToolWindow,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoredWorkspaceRenderer {
    Native,
    LegacyNapplet,
    Surface,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkspaceSlotV1 {
    slot_id: String,
    role: StoredWorkspaceRole,
    renderer: StoredWorkspaceRenderer,
    handler_id: String,
    manifest_author: Option<String>,
    d_tag: Option<String>,
    aggregate_hash: Option<String>,
    binding_parameters: serde_json::Value,
    navigation: serde_json::Value,
    visible: bool,
    order: u16,
    size_points: u16,
    minimum_points: u16,
    maximum_points: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkspaceV1 {
    schema_version: u16,
    axis: StoredWorkspaceAxis,
    slots: Vec<StoredWorkspaceSlotV1>,
    focused_slot_id: Option<String>,
    activity_drawer_visible: bool,
    preferences: serde_json::Value,
}

fn workspace_record_from_ffi(
    workspace: RuntimeWorkspaceDefinition,
) -> Result<WorkspaceRecord, String> {
    validate_workspace_name("workspace_id", &workspace.workspace_id)?;
    if workspace.schema_version != WORKSPACE_SCHEMA_VERSION {
        return Err(format!(
            "workspace schema version {} is unsupported; expected {WORKSPACE_SCHEMA_VERSION}",
            workspace.schema_version
        ));
    }
    if workspace.slots.is_empty() || workspace.slots.len() > MAXIMUM_WORKSPACE_SLOTS {
        return Err(format!(
            "workspace must contain 1..={MAXIMUM_WORKSPACE_SLOTS} slots"
        ));
    }
    if workspace.retained_receipt_ids.len() > MAXIMUM_WORKSPACE_RECEIPTS {
        return Err(format!(
            "workspace retains {} receipts; the maximum is {MAXIMUM_WORKSPACE_RECEIPTS}",
            workspace.retained_receipt_ids.len()
        ));
    }
    let preferences = parse_workspace_object("preferences_json", &workspace.preferences_json)?;
    let mut slot_ids = BTreeSet::new();
    let mut orders = BTreeSet::new();
    let mut stored_slots = Vec::with_capacity(workspace.slots.len());
    for slot in workspace.slots {
        validate_workspace_name("slot_id", &slot.slot_id)?;
        validate_workspace_name("handler_id", &slot.handler_id)?;
        if !slot_ids.insert(slot.slot_id.clone()) {
            return Err(format!("duplicate workspace slot id {:?}", slot.slot_id));
        }
        if !orders.insert(slot.order) {
            return Err(format!("duplicate workspace slot order {}", slot.order));
        }
        if slot.minimum_points == 0
            || slot.minimum_points > slot.size_points
            || slot.size_points > slot.maximum_points
            || slot.maximum_points > MAXIMUM_WORKSPACE_POINT_SIZE
        {
            return Err(format!(
                "slot {:?} size must satisfy 1 <= minimum <= size <= maximum <= {MAXIMUM_WORKSPACE_POINT_SIZE}",
                slot.slot_id
            ));
        }
        validate_workspace_handler(&slot)?;
        stored_slots.push(StoredWorkspaceSlotV1 {
            slot_id: slot.slot_id,
            role: stored_role(slot.role),
            renderer: stored_renderer(slot.renderer),
            handler_id: slot.handler_id,
            manifest_author: slot.manifest_author,
            d_tag: slot.d_tag,
            aggregate_hash: slot.aggregate_hash,
            binding_parameters: parse_workspace_object(
                "binding_parameters_json",
                &slot.binding_parameters_json,
            )?,
            navigation: parse_workspace_object("navigation_json", &slot.navigation_json)?,
            visible: slot.visible,
            order: slot.order,
            size_points: slot.size_points,
            minimum_points: slot.minimum_points,
            maximum_points: slot.maximum_points,
        });
    }
    if let Some(focused) = &workspace.focused_slot_id {
        validate_workspace_name("focused_slot_id", focused)?;
        if !stored_slots
            .iter()
            .any(|slot| slot.slot_id == *focused && slot.visible)
        {
            return Err("focused slot must name a visible workspace slot".to_owned());
        }
    }
    let mut receipt_ids = BTreeSet::new();
    let retained_receipts = workspace
        .retained_receipt_ids
        .into_iter()
        .map(|receipt_id| {
            validate_workspace_name("retained_receipt_id", &receipt_id)?;
            if !receipt_ids.insert(receipt_id.clone()) {
                return Err(format!("duplicate retained receipt id {receipt_id:?}"));
            }
            Ok(WriteReceiptId(Arc::from(receipt_id)))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let stored = StoredWorkspaceV1 {
        schema_version: WORKSPACE_SCHEMA_VERSION,
        axis: stored_axis(workspace.axis),
        slots: stored_slots,
        focused_slot_id: workspace.focused_slot_id,
        activity_drawer_visible: workspace.activity_drawer_visible,
        preferences,
    };
    let value = serde_json::to_value(stored)
        .map_err(|error| format!("workspace serialization failed: {error}"))?;
    let definition = BoundedJson::from_value(&value, MAXIMUM_WORKSPACE_JSON_BYTES)
        .map_err(|error| error.to_string())?;
    Ok(WorkspaceRecord {
        id: Arc::from(workspace.workspace_id),
        definition,
        retained_receipts,
    })
}

fn workspace_from_view(workspace: &WorkspaceView) -> Result<RuntimeWorkspaceDefinition, String> {
    workspace_from_parts(
        workspace.id.as_ref(),
        &workspace.definition,
        &workspace.retained_receipts,
    )
}

fn workspace_from_record(
    workspace: &WorkspaceRecord,
) -> Result<RuntimeWorkspaceDefinition, String> {
    workspace_from_parts(
        workspace.id.as_ref(),
        &workspace.definition,
        &workspace.retained_receipts,
    )
}

fn workspace_from_parts(
    workspace_id: &str,
    definition: &BoundedJson,
    retained_receipts: &[WriteReceiptId],
) -> Result<RuntimeWorkspaceDefinition, String> {
    validate_workspace_name("workspace_id", workspace_id)?;
    let stored: StoredWorkspaceV1 = serde_json::from_str(definition.as_str())
        .map_err(|error| format!("workspace definition is malformed: {error}"))?;
    if stored.schema_version != WORKSPACE_SCHEMA_VERSION {
        return Err(format!(
            "workspace schema version {} is unsupported; expected {WORKSPACE_SCHEMA_VERSION}",
            stored.schema_version
        ));
    }
    let projected = RuntimeWorkspaceDefinition {
        schema_version: stored.schema_version,
        workspace_id: workspace_id.to_owned(),
        axis: ffi_axis(stored.axis),
        slots: stored
            .slots
            .into_iter()
            .map(|slot| RuntimeWorkspaceSlot {
                slot_id: slot.slot_id,
                role: ffi_role(slot.role),
                renderer: ffi_renderer(slot.renderer),
                handler_id: slot.handler_id,
                manifest_author: slot.manifest_author,
                d_tag: slot.d_tag,
                aggregate_hash: slot.aggregate_hash,
                binding_parameters_json: serde_json::to_string(&slot.binding_parameters)
                    .expect("serializing a parsed JSON value cannot fail"),
                navigation_json: serde_json::to_string(&slot.navigation)
                    .expect("serializing a parsed JSON value cannot fail"),
                visible: slot.visible,
                order: slot.order,
                size_points: slot.size_points,
                minimum_points: slot.minimum_points,
                maximum_points: slot.maximum_points,
            })
            .collect(),
        focused_slot_id: stored.focused_slot_id,
        activity_drawer_visible: stored.activity_drawer_visible,
        preferences_json: serde_json::to_string(&stored.preferences)
            .expect("serializing a parsed JSON value cannot fail"),
        retained_receipt_ids: retained_receipts
            .iter()
            .map(|receipt| receipt.0.to_string())
            .collect(),
    };
    // Apply every ingress invariant to durable data before returning it to a
    // native caller. This catches corrupt or pre-versioned rows atomically.
    let _ = workspace_record_from_ffi(projected.clone())?;
    Ok(projected)
}

fn validate_workspace_handler(slot: &RuntimeWorkspaceSlot) -> Result<(), String> {
    match slot.renderer {
        RuntimeWorkspaceRenderer::Native | RuntimeWorkspaceRenderer::Unavailable => {
            if slot.manifest_author.is_some()
                || slot.d_tag.is_some()
                || slot.aggregate_hash.is_some()
            {
                return Err(format!(
                    "slot {:?} native/unavailable handlers cannot carry a napplet principal",
                    slot.slot_id
                ));
            }
        }
        RuntimeWorkspaceRenderer::LegacyNapplet | RuntimeWorkspaceRenderer::Surface => {
            let author = slot
                .manifest_author
                .as_deref()
                .ok_or_else(|| format!("slot {:?} is missing manifest_author", slot.slot_id))?;
            let d_tag = slot
                .d_tag
                .as_deref()
                .ok_or_else(|| format!("slot {:?} is missing d_tag", slot.slot_id))?;
            let aggregate = slot
                .aggregate_hash
                .as_deref()
                .ok_or_else(|| format!("slot {:?} is missing aggregate_hash", slot.slot_id))?;
            validate_hex64("manifest_author", author)?;
            validate_workspace_name("d_tag", d_tag)?;
            validate_hex64("aggregate_hash", aggregate)?;
        }
    }
    Ok(())
}

fn validate_workspace_name(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(format!(
            "{field} must be non-empty, control-free, and at most 256 bytes"
        ));
    }
    Ok(())
}

fn validate_hex64(field: &str, value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{field} must be exactly 64 hexadecimal characters"));
    }
    Ok(())
}

fn parse_workspace_object(field: &str, raw: &str) -> Result<serde_json::Value, String> {
    if raw.len() > MAXIMUM_WORKSPACE_FIELD_BYTES {
        return Err(format!(
            "{field} is {} bytes; the maximum is {MAXIMUM_WORKSPACE_FIELD_BYTES}",
            raw.len()
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("{field} is invalid JSON: {error}"))?;
    if !value.is_object() {
        return Err(format!("{field} must be a JSON object"));
    }
    Ok(value)
}

fn stored_axis(axis: RuntimeWorkspaceAxis) -> StoredWorkspaceAxis {
    match axis {
        RuntimeWorkspaceAxis::Horizontal => StoredWorkspaceAxis::Horizontal,
        RuntimeWorkspaceAxis::Vertical => StoredWorkspaceAxis::Vertical,
    }
}

fn ffi_axis(axis: StoredWorkspaceAxis) -> RuntimeWorkspaceAxis {
    match axis {
        StoredWorkspaceAxis::Horizontal => RuntimeWorkspaceAxis::Horizontal,
        StoredWorkspaceAxis::Vertical => RuntimeWorkspaceAxis::Vertical,
    }
}

fn stored_role(role: RuntimeWorkspaceRole) -> StoredWorkspaceRole {
    match role {
        RuntimeWorkspaceRole::Feed => StoredWorkspaceRole::Feed,
        RuntimeWorkspaceRole::Detail => StoredWorkspaceRole::Detail,
        RuntimeWorkspaceRole::Profile => StoredWorkspaceRole::Profile,
        RuntimeWorkspaceRole::Thread => StoredWorkspaceRole::Thread,
        RuntimeWorkspaceRole::Composer => StoredWorkspaceRole::Composer,
        RuntimeWorkspaceRole::MediaPlayer => StoredWorkspaceRole::MediaPlayer,
        RuntimeWorkspaceRole::ToolWindow => StoredWorkspaceRole::ToolWindow,
    }
}

fn ffi_role(role: StoredWorkspaceRole) -> RuntimeWorkspaceRole {
    match role {
        StoredWorkspaceRole::Feed => RuntimeWorkspaceRole::Feed,
        StoredWorkspaceRole::Detail => RuntimeWorkspaceRole::Detail,
        StoredWorkspaceRole::Profile => RuntimeWorkspaceRole::Profile,
        StoredWorkspaceRole::Thread => RuntimeWorkspaceRole::Thread,
        StoredWorkspaceRole::Composer => RuntimeWorkspaceRole::Composer,
        StoredWorkspaceRole::MediaPlayer => RuntimeWorkspaceRole::MediaPlayer,
        StoredWorkspaceRole::ToolWindow => RuntimeWorkspaceRole::ToolWindow,
    }
}

fn stored_renderer(renderer: RuntimeWorkspaceRenderer) -> StoredWorkspaceRenderer {
    match renderer {
        RuntimeWorkspaceRenderer::Native => StoredWorkspaceRenderer::Native,
        RuntimeWorkspaceRenderer::LegacyNapplet => StoredWorkspaceRenderer::LegacyNapplet,
        RuntimeWorkspaceRenderer::Surface => StoredWorkspaceRenderer::Surface,
        RuntimeWorkspaceRenderer::Unavailable => StoredWorkspaceRenderer::Unavailable,
    }
}

fn ffi_renderer(renderer: StoredWorkspaceRenderer) -> RuntimeWorkspaceRenderer {
    match renderer {
        StoredWorkspaceRenderer::Native => RuntimeWorkspaceRenderer::Native,
        StoredWorkspaceRenderer::LegacyNapplet => RuntimeWorkspaceRenderer::LegacyNapplet,
        StoredWorkspaceRenderer::Surface => RuntimeWorkspaceRenderer::Surface,
        StoredWorkspaceRenderer::Unavailable => RuntimeWorkspaceRenderer::Unavailable,
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
mod tests {
    use std::{collections::BTreeMap, fs};

    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;

    const EVENT: &[u8] =
        include_bytes!("../../../conformance/napplet-corpus/published/good-morning/event.json");
    const INDEX: &[u8] =
        include_bytes!("../../../conformance/napplet-corpus/published/good-morning/index.html");
    const AUTHOR: &str = "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5";
    const DIGEST: &str = "ffd35eea5c84d03cdda74c23e1bbb2c40500f503833503aa688036faa52f3808";

    struct FixtureSource(BTreeMap<String, Vec<u8>>);

    impl ArtifactSource for FixtureSource {
        fn fetch(&self, request: ArtifactFetchRequest) -> ArtifactFetchResponse {
            let bytes = self
                .0
                .get(&request.expected_sha256)
                .cloned()
                .unwrap_or_default();
            ArtifactFetchResponse::Body {
                source_url: request.candidate_urls[0].clone(),
                http_status: 200,
                bytes,
            }
        }
    }

    #[derive(Debug)]
    struct FixtureAppearance;

    impl NativeAppearanceSource for FixtureAppearance {
        fn current(&self) -> Option<NativeAppearanceSnapshot> {
            Some(NativeAppearanceSnapshot {
                dark: true,
                increased_contrast: false,
                reduced_transparency: false,
                accent_red: 88,
                accent_green: 166,
                accent_blue: 255,
            })
        }
    }

    #[derive(Debug)]
    struct RecordingSettings {
        requests: Arc<Mutex<Vec<NativeSettingsRequest>>>,
    }

    impl NativeSettingsExecutor for RecordingSettings {
        fn try_open(&self, request: NativeSettingsRequest) -> NativeSettingsOpenResult {
            self.requests.lock().push(request);
            NativeSettingsOpenResult::Accepted
        }
    }

    #[derive(Debug)]
    struct RecordingIncActions {
        requests: Arc<Mutex<Vec<NativeIncActionRequest>>>,
        ends: Arc<Mutex<Vec<NativeIncActionEnd>>>,
        result: NativeIncActionEnqueueResult,
    }

    impl NativeIncActionExecutor for RecordingIncActions {
        fn try_enqueue(&self, request: NativeIncActionRequest) -> NativeIncActionEnqueueResult {
            self.requests.lock().push(request);
            self.result
        }

        fn session_ended(&self, end: NativeIncActionEnd) {
            self.ends.lock().push(end);
        }
    }

    fn controller(temp: &TempDir) -> Arc<RuntimeController> {
        RuntimeController::open(
            RuntimeConfig {
                runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
                nmp_store_path: None,
                artifact_cache_path: temp.path().join("artifacts").display().to_string(),
                ..RuntimeConfig::default()
            },
            Box::new(FixtureSource(BTreeMap::from([(
                DIGEST.to_owned(),
                INDEX.to_vec(),
            )]))),
        )
        .unwrap()
    }

    fn controller_with_native_capabilities(
        temp: &TempDir,
        requests: Arc<Mutex<Vec<NativeSettingsRequest>>>,
    ) -> Arc<RuntimeController> {
        RuntimeController::open_with_native_capabilities(
            RuntimeConfig {
                runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
                nmp_store_path: None,
                artifact_cache_path: temp.path().join("artifacts").display().to_string(),
                ..RuntimeConfig::default()
            },
            Box::new(FixtureSource(BTreeMap::from([(
                DIGEST.to_owned(),
                INDEX.to_vec(),
            )]))),
            Box::new(FixtureAppearance),
            Box::new(RecordingSettings { requests }),
        )
        .unwrap()
    }

    fn controller_with_all_native_capabilities(
        temp: &TempDir,
        requests: Arc<Mutex<Vec<NativeIncActionRequest>>>,
        ends: Arc<Mutex<Vec<NativeIncActionEnd>>>,
        result: NativeIncActionEnqueueResult,
    ) -> Arc<RuntimeController> {
        RuntimeController::open_with_all_native_capabilities(
            RuntimeConfig {
                runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
                nmp_store_path: None,
                artifact_cache_path: temp.path().join("artifacts").display().to_string(),
                ..RuntimeConfig::default()
            },
            Box::new(FixtureSource(BTreeMap::from([(
                DIGEST.to_owned(),
                INDEX.to_vec(),
            )]))),
            Box::new(FixtureAppearance),
            Box::new(RecordingSettings {
                requests: Arc::new(Mutex::new(Vec::new())),
            }),
            Box::new(RecordingIncActions {
                requests,
                ends,
                result,
            }),
        )
        .unwrap()
    }

    fn workspace_definition(id: &str) -> RuntimeWorkspaceDefinition {
        RuntimeWorkspaceDefinition {
            schema_version: WORKSPACE_SCHEMA_VERSION,
            workspace_id: id.to_owned(),
            axis: RuntimeWorkspaceAxis::Horizontal,
            slots: vec![
                RuntimeWorkspaceSlot {
                    slot_id: "feed".to_owned(),
                    role: RuntimeWorkspaceRole::Feed,
                    renderer: RuntimeWorkspaceRenderer::LegacyNapplet,
                    handler_id: "good-morning".to_owned(),
                    manifest_author: Some(AUTHOR.to_owned()),
                    d_tag: Some("good-morning".to_owned()),
                    aggregate_hash: Some("a".repeat(64)),
                    binding_parameters_json: r#"{"window":{"limit":100}}"#.to_owned(),
                    navigation_json: r#"{"selection":null}"#.to_owned(),
                    visible: true,
                    order: 0,
                    size_points: 640,
                    minimum_points: 320,
                    maximum_points: 1_200,
                },
                RuntimeWorkspaceSlot {
                    slot_id: "detail".to_owned(),
                    role: RuntimeWorkspaceRole::Detail,
                    renderer: RuntimeWorkspaceRenderer::Native,
                    handler_id: "native-detail".to_owned(),
                    manifest_author: None,
                    d_tag: None,
                    aggregate_hash: None,
                    binding_parameters_json: "{}".to_owned(),
                    navigation_json: "{}".to_owned(),
                    visible: true,
                    order: 1,
                    size_points: 360,
                    minimum_points: 240,
                    maximum_points: 900,
                },
            ],
            focused_slot_id: Some("feed".to_owned()),
            activity_drawer_visible: true,
            preferences_json: r#"{"sidebar":"home"}"#.to_owned(),
            retained_receipt_ids: Vec::new(),
        }
    }

    fn exact_coordinate(artifact: &VerifiedArtifact) -> RuntimeExactBuildCoordinate {
        RuntimeExactBuildCoordinate {
            manifest_author: artifact.author(),
            d_tag: artifact.d_tag().expect("named fixture"),
            aggregate_hash: artifact.aggregate_hash(),
        }
    }

    fn install_permission_fixture(
        controller: &Arc<RuntimeController>,
    ) -> RuntimeExactBuildCoordinate {
        let artifact = controller
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .unwrap();
        let principal = artifact.principal.clone().unwrap();
        let executable: Arc<dyn ExecutableArtifact> = artifact.handle.clone();
        controller.app.dispatch(PlatformCommand::InstallVerified {
            build: InstalledBuild {
                principal,
                title: Arc::from("Good Morning Protocol"),
                manifest_metadata: BoundedJson::from_value(
                    &serde_json::json!({"kind": 35129}),
                    1_024,
                )
                .unwrap(),
                capability_requests: vec![
                    CapabilityRequest {
                        capability: Capability::new("identity").unwrap(),
                        requirement: CapabilityRequirement::Required,
                    },
                    CapabilityRequest {
                        capability: Capability::new("missing").unwrap(),
                        requirement: CapabilityRequirement::Optional,
                    },
                ],
            },
            artifact: executable,
        });
        exact_coordinate(&artifact)
    }

    fn install_and_launch(
        controller: &Arc<RuntimeController>,
        domains: &[&str],
    ) -> (Arc<VerifiedArtifact>, u64) {
        let artifact = controller
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .unwrap();
        controller.install(Arc::clone(&artifact));
        for domain in ["identity", "inc", "outbox"]
            .into_iter()
            .chain(domains.iter().copied())
        {
            controller.set_grant(
                Arc::clone(&artifact),
                domain.to_owned(),
                RuntimeSensitivity::Ordinary,
                RuntimeGrantDecision::AllowExactBuild,
            );
        }
        controller.launch(Arc::clone(&artifact), RuntimeExecutionProfile::Legacy);
        let session = controller.snapshot().sessions[0].id;
        controller.mapped_envelope(session, br#"{"type":"shell.ready"}"#.to_vec());
        (artifact, session)
    }

    fn response_of_type(controller: &RuntimeController, expected: &str) -> Value {
        controller
            .app
            .events_after(0)
            .events
            .into_iter()
            .rev()
            .find_map(|event| match event.event {
                PlatformEvent::EnvelopeHandled {
                    response: Some(response),
                    ..
                } if response.decode().ok()?.get("type")? == expected => response.decode().ok(),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing `{expected}` response"))
    }

    #[test]
    fn signed_artifact_crosses_only_as_sealed_handle_and_exact_reads() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);
        let verified = controller.verify_artifact(
            EVENT.to_vec(),
            ArtifactCoordinate::Named {
                author: AUTHOR.to_owned(),
                d_tag: "good-morning".to_owned(),
            },
        );
        let artifact = verified.artifact.expect("published fixture verifies");
        assert!(verified.refusal.is_none());
        assert!(artifact.requires().is_empty());
        controller.install(Arc::clone(&artifact));
        controller.set_grant(
            Arc::clone(&artifact),
            "shell".to_owned(),
            RuntimeSensitivity::Ordinary,
            RuntimeGrantDecision::AllowExactBuild,
        );
        for domain in ["identity", "inc", "outbox"] {
            controller.set_grant(
                Arc::clone(&artifact),
                domain.to_owned(),
                RuntimeSensitivity::Sensitive,
                RuntimeGrantDecision::AllowExactBuild,
            );
        }
        controller.launch(artifact, RuntimeExecutionProfile::Legacy);
        let runtime_snapshot = controller.snapshot();
        assert_eq!(
            runtime_snapshot.sessions[0].domains,
            ["identity", "inc", "outbox", "shell"]
        );
        let session = runtime_snapshot.sessions[0].id;
        controller.mapped_envelope(session, br#"{"type":"shell.ready"}"#.to_vec());
        controller.mapped_envelope(
            session,
            br#"{"type":"identity.getPublicKey","id":"identity-1"}"#.to_vec(),
        );
        let identity_response = controller
            .app
            .events_after(0)
            .events
            .into_iter()
            .find_map(|event| match event.event {
                PlatformEvent::EnvelopeHandled {
                    response: Some(response),
                    ..
                } if response.decode().ok()?.get("type")? == "identity.getPublicKey.result" => {
                    response.decode().ok()
                }
                _ => None,
            })
            .expect("registered identity provider responds through the runtime");
        assert_eq!(identity_response["id"], "identity-1");
        assert_eq!(identity_response["pubkey"], "");
        controller.mapped_envelope(
            session,
            br#"{"type":"inc.subscribe","id":"inc-1","topic":"profile:open"}"#.to_vec(),
        );
        let inc_response = controller
            .app
            .events_after(0)
            .events
            .into_iter()
            .find_map(|event| match event.event {
                PlatformEvent::EnvelopeHandled {
                    response: Some(response),
                    ..
                } if response.decode().ok()?.get("type")? == "inc.subscribe.result" => {
                    response.decode().ok()
                }
                _ => None,
            })
            .expect("registered INC provider responds through the runtime");
        assert_eq!(inc_response["id"], "inc-1");
        match controller.read_verified(session, "/index.html".to_owned(), 1_024 * 1_024) {
            VerifiedRead::Bytes { bytes, .. } => assert_eq!(bytes, INDEX),
            VerifiedRead::Refused { refusal } => panic!("{refusal:?}"),
        }
        assert!(matches!(
            controller.read_verified(session, "/../secret".to_owned(), 1_024),
            VerifiedRead::Refused { .. }
        ));
        controller.close();
        assert!(controller.snapshot().closed);
        assert!(fs::metadata(temp.path().join("runtime.sqlite3")).is_ok());
    }

    #[test]
    fn pinned_good_morning_installs_rust_owned_permission_profile() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);
        let artifact = controller
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: GOOD_MORNING_D_TAG.to_owned(),
                },
            )
            .artifact
            .expect("published fixture verifies");
        assert!(
            artifact.requires().is_empty(),
            "the immutable manifest remains unchanged"
        );

        controller.install(Arc::clone(&artifact));
        let review = controller
            .permission_review(exact_coordinate(&artifact))
            .review
            .expect("the installed exact build has a permission review");
        assert_eq!(
            review
                .capabilities
                .iter()
                .map(|capability| { (capability.domain.as_str(), capability.requirement) })
                .collect::<Vec<_>>(),
            vec![
                ("identity", RuntimePermissionRequirement::Required),
                ("inc", RuntimePermissionRequirement::Required),
                ("outbox", RuntimePermissionRequirement::Required),
                ("resource", RuntimePermissionRequirement::Optional),
                ("theme", RuntimePermissionRequirement::Optional),
                ("link", RuntimePermissionRequirement::Optional),
            ]
        );
        assert!(!review.launch_permitted);
        let outbox = review
            .capabilities
            .iter()
            .find(|capability| capability.domain == "outbox")
            .expect("outbox permission");
        assert_eq!(outbox.sensitivity, RuntimePermissionSensitivity::Sensitive);

        controller.launch(artifact, RuntimeExecutionProfile::Legacy);
        assert!(
            controller.snapshot().sessions.is_empty(),
            "required compatibility capabilities are enforced before execution"
        );
    }

    #[test]
    fn permission_review_and_atomic_batch_are_exact_typed_and_restart_safe() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let coordinate = install_permission_fixture(&runtime);

        let initial = runtime.permission_review(coordinate.clone());
        assert!(initial.refusal.is_none());
        let initial = initial.review.unwrap();
        assert_eq!(initial.coordinate, coordinate);
        assert_eq!(initial.capabilities.len(), 2);
        assert_eq!(initial.capabilities[0].domain, "identity");
        assert_eq!(
            initial.capabilities[0].platform_availability,
            RuntimePermissionPlatformAvailability::Available
        );
        assert_eq!(
            initial.capabilities[0].sensitivity,
            RuntimePermissionSensitivity::Sensitive
        );
        assert_eq!(initial.capabilities[0].decision_options.len(), 4);
        assert_eq!(
            initial.capabilities[1].platform_availability,
            RuntimePermissionPlatformAvailability::Unknown {
                reason: "no provider metadata is registered for this capability on this runtime"
                    .to_owned()
            }
        );

        let duplicate = runtime.apply_permission_decisions(RuntimePermissionDecisionBatch {
            coordinate: coordinate.clone(),
            decisions: vec![
                RuntimePermissionDecisionSelection {
                    domain: "identity".to_owned(),
                    decision: RuntimeGrantDecision::AllowExactBuild,
                },
                RuntimePermissionDecisionSelection {
                    domain: "identity".to_owned(),
                    decision: RuntimeGrantDecision::Denied,
                },
            ],
        });
        assert!(!duplicate.applied);
        assert_eq!(
            duplicate.refusal.unwrap().code,
            "duplicate-permission-domain"
        );

        let applied = runtime.apply_permission_decisions(RuntimePermissionDecisionBatch {
            coordinate: coordinate.clone(),
            decisions: vec![
                RuntimePermissionDecisionSelection {
                    domain: "identity".to_owned(),
                    decision: RuntimeGrantDecision::AllowExactBuild,
                },
                RuntimePermissionDecisionSelection {
                    domain: "missing".to_owned(),
                    decision: RuntimeGrantDecision::Denied,
                },
            ],
        });
        assert!(applied.applied);
        assert!(applied.refusal.is_none());
        let applied_review = applied.review.unwrap();
        assert!(applied_review.launch_permitted);
        assert_eq!(
            applied_review.capabilities[0].existing_decision,
            RuntimePermissionExistingDecision::AllowExactBuild
        );
        runtime.close();
        drop(runtime);

        let reopened = controller(&temp);
        let restored = reopened.permission_review(coordinate).review.unwrap();
        assert_eq!(restored.capabilities.len(), 2);
        assert_eq!(
            restored.capabilities[0].existing_decision,
            RuntimePermissionExistingDecision::AllowExactBuild
        );
        assert!(restored.launch_permitted);
    }

    #[test]
    fn good_morning_outbox_grant_survives_default_profile_restart() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let artifact = runtime
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("published fixture verifies");
        runtime.install(Arc::clone(&artifact));
        let coordinate = exact_coordinate(&artifact);
        let review = runtime
            .permission_review(coordinate.clone())
            .review
            .expect("installed Good Morning has a permission review");
        let update = runtime.apply_permission_decisions(RuntimePermissionDecisionBatch {
            coordinate: coordinate.clone(),
            decisions: review
                .capabilities
                .iter()
                .map(|capability| RuntimePermissionDecisionSelection {
                    domain: capability.domain.clone(),
                    decision: match capability.requirement {
                        RuntimePermissionRequirement::Required => {
                            RuntimeGrantDecision::AllowExactBuild
                        }
                        RuntimePermissionRequirement::Optional => RuntimeGrantDecision::Denied,
                    },
                })
                .collect(),
        });
        assert!(update.applied);
        assert!(update.review.unwrap().launch_permitted);
        runtime.close();
        drop(runtime);

        let reopened = controller(&temp);
        let artifact = reopened
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("published fixture verifies after restart");
        reopened.install(Arc::clone(&artifact));
        let review = reopened
            .permission_review(coordinate)
            .review
            .expect("Good Morning review restores after restart");
        for domain in ["identity", "inc", "outbox"] {
            let capability = review
                .capabilities
                .iter()
                .find(|capability| capability.domain == domain)
                .unwrap_or_else(|| panic!("missing required {domain} capability"));
            assert_eq!(
                capability.existing_decision,
                RuntimePermissionExistingDecision::AllowExactBuild
            );
        }
        assert!(review.launch_permitted);

        reopened.launch(artifact, RuntimeExecutionProfile::Legacy);
        let session = reopened.snapshot().sessions[0].clone();
        assert_eq!(
            session.domains,
            ["identity", "inc", "outbox", "shell"],
            "the restored exact-build grant must negotiate NAP-OUTBOX"
        );
        reopened.mapped_envelope(session.id, br#"{"type":"shell.ready"}"#.to_vec());
        assert_eq!(
            response_of_type(&reopened, "shell.init")["capabilities"]["domains"],
            serde_json::json!(["identity", "inc", "outbox", "shell"]),
            "the trusted shell must receive the same Rust-negotiated domain set"
        );
    }

    #[test]
    fn demo_profile_repairs_a_persisted_denied_outbox_grant() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let artifact = runtime
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("published fixture verifies");
        runtime.install(Arc::clone(&artifact));
        let coordinate = exact_coordinate(&artifact);
        let review = runtime
            .permission_review(coordinate.clone())
            .review
            .expect("installed Good Morning has a permission review");
        let denied = runtime.apply_permission_decisions(RuntimePermissionDecisionBatch {
            coordinate: coordinate.clone(),
            decisions: review
                .capabilities
                .iter()
                .map(|capability| RuntimePermissionDecisionSelection {
                    domain: capability.domain.clone(),
                    decision: RuntimeGrantDecision::Denied,
                })
                .collect(),
        });
        assert!(denied.applied);
        assert!(!denied.review.unwrap().launch_permitted);
        runtime.close();
        drop(runtime);

        let demo = RuntimeController::open(
            RuntimeConfig {
                runtime_store_path: temp.path().join("runtime.sqlite3").display().to_string(),
                nmp_store_path: None,
                artifact_cache_path: temp.path().join("artifacts").display().to_string(),
                permission_mode: RuntimePermissionMode::DemoPinnedGoodMorning,
                ..RuntimeConfig::default()
            },
            Box::new(FixtureSource(BTreeMap::from([(
                DIGEST.to_owned(),
                INDEX.to_vec(),
            )]))),
        )
        .unwrap();
        let repaired = demo
            .permission_review(coordinate)
            .review
            .expect("demo startup restores the installed exact build review");
        for domain in ["identity", "inc", "outbox"] {
            let capability = repaired
                .capabilities
                .iter()
                .find(|capability| capability.domain == domain)
                .unwrap_or_else(|| panic!("missing required {domain} capability"));
            assert_eq!(
                capability.existing_decision,
                RuntimePermissionExistingDecision::AllowExactBuild,
                "demo startup must repair persisted denial for {domain}"
            );
        }
        assert!(repaired.launch_permitted);
        demo.close();
    }

    #[test]
    fn installed_library_projects_filter_lifecycle_workspace_and_uninstall() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);
        let artifact = controller
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("fixture verifies");
        let coordinate = exact_coordinate(&artifact);

        assert_eq!(controller.snapshot().installed_library.total_installed, 0);
        controller.install(Arc::clone(&artifact));
        let installed = controller.snapshot().installed_library;
        assert_eq!(installed.query, "");
        assert_eq!(installed.total_installed, 1);
        assert_eq!(installed.builds.len(), 1);
        assert_eq!(installed.builds[0].coordinate, coordinate);
        assert_eq!(
            installed.builds[0].availability,
            RuntimeInstalledBuildAvailability::SealedExactBytesReady
        );
        assert!(installed.builds[0].active_session_ids.is_empty());
        assert!(installed.builds[0].assigned_workspace_ids.is_empty());
        assert!(serde_json::from_str::<Value>(&installed.builds[0].manifest_metadata_json).is_ok());

        controller.set_library_filter("no-match".to_owned());
        let filtered = controller.snapshot().installed_library;
        assert_eq!(filtered.query, "no-match");
        assert_eq!(filtered.total_installed, 1);
        assert!(filtered.builds.is_empty());
        controller.set_library_filter("GOOD-MORNING".to_owned());
        assert_eq!(controller.snapshot().installed_library.builds.len(), 1);

        assert!(
            controller
                .save_workspace(workspace_definition("library"))
                .accepted
        );
        controller.assign_build_to_workspace("library".to_owned(), coordinate.clone());
        assert_eq!(
            controller.snapshot().installed_library.builds[0].assigned_workspace_ids,
            ["library"]
        );

        controller.launch(Arc::clone(&artifact), RuntimeExecutionProfile::Legacy);
        assert!(
            controller.snapshot().sessions.is_empty(),
            "the pinned required profile must refuse before execution"
        );
        for domain in ["identity", "inc", "outbox"] {
            controller.set_grant(
                Arc::clone(&artifact),
                domain.to_owned(),
                RuntimeSensitivity::Sensitive,
                RuntimeGrantDecision::AllowExactBuild,
            );
        }
        controller.launch(Arc::clone(&artifact), RuntimeExecutionProfile::Legacy);
        let session = controller.snapshot().installed_library.builds[0].active_session_ids[0];
        controller.suspend(session);
        assert_eq!(controller.snapshot().sessions[0].state, "suspended");
        controller.resume(session);
        assert_eq!(controller.snapshot().sessions[0].state, "running");

        controller.clear_build_from_workspace("library".to_owned(), coordinate.clone());
        assert!(
            controller.snapshot().installed_library.builds[0]
                .assigned_workspace_ids
                .is_empty()
        );

        controller.uninstall_build(coordinate.clone());
        let uninstalled = controller.snapshot();
        assert_eq!(uninstalled.installed_library.total_installed, 0);
        assert!(uninstalled.installed_library.builds.is_empty());
        assert!(uninstalled.sessions.is_empty());
        assert!(uninstalled.workspaces.iter().any(|workspace| {
            workspace.workspace_id == "library" && workspace.retained_receipt_ids.is_empty()
        }));
        assert!(
            !controller.artifacts.lock().contains_key(
                &Principal::new(
                    coordinate.manifest_author,
                    coordinate.d_tag,
                    coordinate.aggregate_hash
                )
                .unwrap()
            ),
            "the boundary must release its live verifier handle after kernel-confirmed uninstall"
        );
    }

    #[test]
    fn installed_artifact_reacquisition_reuses_the_live_exact_handle() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let artifact = runtime
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("fixture verifies");
        let coordinate = exact_coordinate(&artifact);
        runtime.install(artifact);
        runtime.set_library_filter("does-not-match".to_owned());

        let reopened = runtime.reacquire_installed_artifact(coordinate);
        assert!(reopened.failure.is_none());
        let confirmation = reopened.confirmation.expect("exact confirmation");
        let event: Value = serde_json::from_slice(EVENT).unwrap();
        assert_eq!(
            confirmation.event_id,
            event["id"].as_str().expect("fixture event id")
        );
        assert_eq!(confirmation.manifest_author, AUTHOR);
        assert_eq!(confirmation.d_tag.as_deref(), Some("good-morning"));
        assert_eq!(confirmation.aggregate_hash, GOOD_MORNING_AGGREGATE_HASH);
        assert_eq!(
            reopened
                .artifact
                .expect("opaque artifact")
                .handle
                .read_verified(nmp_native_artifact::INDEX_PATH, INDEX.len())
                .unwrap(),
            INDEX
        );
    }

    #[test]
    fn persisted_install_reopens_offline_from_the_sealed_cache_after_restart() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let artifact = runtime
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("fixture verifies");
        runtime.install(Arc::clone(&artifact));
        let coordinate = exact_coordinate(&artifact);
        runtime.close();
        drop(runtime);

        let reopened = controller(&temp);
        assert_eq!(
            reopened.snapshot().installed_library.builds[0].availability,
            RuntimeInstalledBuildAvailability::MetadataOnly
        );
        let result = reopened.reacquire_installed_artifact(coordinate);
        assert!(result.failure.is_none());
        let confirmation = result.confirmation.expect("exact confirmation");
        assert_eq!(confirmation.manifest_author, AUTHOR);
        assert_eq!(confirmation.d_tag.as_deref(), Some("good-morning"));
        assert_eq!(confirmation.aggregate_hash, GOOD_MORNING_AGGREGATE_HASH);
        assert_eq!(
            result
                .artifact
                .expect("opaque artifact")
                .handle
                .read_verified(nmp_native_artifact::INDEX_PATH, INDEX.len())
                .unwrap(),
            INDEX
        );
        // No network fetch happened -- this is offline reopen from the
        // sealed cache -- and the runtime's own live handle map now has the
        // reconstructed artifact attached, same as after a fresh install.
        assert_eq!(
            reopened.snapshot().installed_library.builds[0].availability,
            RuntimeInstalledBuildAvailability::SealedExactBytesReady
        );
    }

    #[test]
    fn reacquire_refuses_a_legacy_install_with_no_retained_signed_event() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let artifact = runtime
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("fixture verifies");
        runtime.install(Arc::clone(&artifact));
        let coordinate = exact_coordinate(&artifact);
        // Simulate a build installed before offline reopen was supported:
        // its persisted manifest metadata has no retained signed event.
        let mut legacy_build = runtime
            .runtime_store
            .installed_builds()
            .unwrap()
            .into_iter()
            .find(|build| build.principal.manifest_author() == AUTHOR)
            .expect("installed build");
        legacy_build.manifest_metadata =
            BoundedJson::from_value(&serde_json::json!({"event_id": "legacy"}), 4_096).unwrap();
        runtime.runtime_store.install(&legacy_build).unwrap();
        runtime.close();
        drop(runtime);

        let reopened = controller(&temp);
        let result = reopened.reacquire_installed_artifact(coordinate);
        assert!(result.artifact.is_none());
        assert_eq!(
            result.failure.expect("typed refusal").code,
            "installed-manifest-event-unavailable"
        );
    }

    #[test]
    fn installed_artifact_reattach_refuses_signed_event_drift() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let artifact = runtime
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("fixture verifies");
        runtime.install(Arc::clone(&artifact));
        let mut installed = runtime.app.snapshot().library.builds[0].build.clone();
        installed.manifest_metadata = BoundedJson::from_value(
            &serde_json::json!({
                "event_id": "0".repeat(64),
                "kind": 35_129,
                "mode": "single-file",
                "paths": 1,
            }),
            1_024,
        )
        .unwrap();

        let failure = runtime
            .verified_installed_artifact(&installed, Arc::clone(&artifact.handle))
            .expect_err("a different signed event must not inherit the persisted install");
        assert_eq!(failure.code, "installed-artifact-mismatch");
    }

    #[test]
    fn installed_library_restores_metadata_only_and_refuses_invalid_inputs() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let artifact = runtime
            .verify_artifact(
                EVENT.to_vec(),
                ArtifactCoordinate::Named {
                    author: AUTHOR.to_owned(),
                    d_tag: "good-morning".to_owned(),
                },
            )
            .artifact
            .expect("fixture verifies");
        runtime.install(artifact);
        runtime.close();
        drop(runtime);

        let reopened = controller(&temp);
        let restored = reopened.snapshot().installed_library;
        assert_eq!(restored.total_installed, 1);
        assert_eq!(restored.builds.len(), 1);
        assert_eq!(
            restored.builds[0].availability,
            RuntimeInstalledBuildAvailability::MetadataOnly
        );
        assert!(restored.builds[0].active_session_ids.is_empty());

        reopened.uninstall_build(RuntimeExactBuildCoordinate {
            manifest_author: AUTHOR.to_ascii_uppercase(),
            d_tag: "good-morning".to_owned(),
            aggregate_hash: restored.builds[0].coordinate.aggregate_hash.clone(),
        });
        let snapshot = reopened.snapshot();
        assert_eq!(snapshot.installed_library.total_installed, 1);
        assert_eq!(
            snapshot.boundary_refusals.last().unwrap().code,
            "invalid-exact-build-coordinate"
        );

        reopened.assign_build_to_workspace("\n".to_owned(), restored.builds[0].coordinate.clone());
        assert_eq!(
            reopened.snapshot().boundary_refusals.last().unwrap().code,
            "invalid-workspace-assignment"
        );

        reopened
            .set_library_filter("x".repeat(AppLimits::default().maximum_library_query_bytes + 1));
        let refused = reopened.snapshot();
        assert_eq!(refused.installed_library.query, "");
        assert_eq!(refused.recent_errors.last().unwrap().code, "capacity");
    }

    #[test]
    fn malformed_manifest_is_a_semantic_refusal() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);
        let result = controller.verify_artifact(
            b"{}".to_vec(),
            ArtifactCoordinate::Named {
                author: "0".repeat(64),
                d_tag: "fixture".to_owned(),
            },
        );
        assert!(result.artifact.is_none());
        assert_eq!(result.refusal.unwrap().code, "artifact-verification");
    }

    #[test]
    fn envelope_response_is_projected_as_exact_machine_readable_json() {
        let response =
            BoundedJson::from_raw(r#"{"type":"shell.init","capabilities":{}}"#, 1_024).unwrap();
        let event = PlatformEvent::EnvelopeHandled {
            session: SessionId(7),
            operation: None,
            response: Some(response.clone()),
        };

        let projected = project_event(11, &event);
        assert_eq!(projected.kind, "envelope-handled");
        assert_eq!(projected.session_id, Some(7));
        assert_eq!(projected.response_json.as_deref(), Some(response.as_str()));
    }

    #[test]
    fn provider_push_is_projected_as_exact_machine_readable_json() {
        let envelope =
            BoundedJson::from_raw(r#"{"type":"identity.changed","pubkey":"abc"}"#, 1_024).unwrap();
        let event = PlatformEvent::ProviderPush {
            session: SessionId(9),
            source_window: nmp_native_nap_bridge::SourceWindowId(3),
            provider_sequence: 4,
            domain: Capability::new("identity").unwrap(),
            envelope: envelope.clone(),
        };

        let projected = project_event(12, &event);
        assert_eq!(projected.kind, "provider-push");
        assert_eq!(projected.session_id, Some(9));
        assert_eq!(projected.response_json.as_deref(), Some(envelope.as_str()));
    }

    #[test]
    fn native_capabilities_are_absent_unless_supplied() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);
        let (_, session) = install_and_launch(&controller, &["theme", "config"]);
        let snapshot = controller.snapshot();
        let domains = &snapshot
            .sessions
            .iter()
            .find(|candidate| candidate.id == session)
            .unwrap()
            .domains;
        assert!(!domains.iter().any(|domain| domain == "theme"));
        assert!(!domains.iter().any(|domain| domain == "config"));
    }

    #[test]
    fn native_theme_and_settings_cross_the_exact_build_boundary() {
        let temp = TempDir::new().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let controller = controller_with_native_capabilities(&temp, Arc::clone(&requests));
        let (_, session) = install_and_launch(&controller, &["theme", "config"]);
        let domains = &controller.snapshot().sessions[0].domains;
        assert!(domains.iter().any(|domain| domain == "theme"));
        assert!(domains.iter().any(|domain| domain == "config"));

        controller.mapped_envelope(session, br#"{"type":"theme.get","id":"theme-1"}"#.to_vec());
        assert_eq!(
            response_of_type(&controller, "theme.get.result")["theme"]["colors"],
            serde_json::json!({
                "background": "#1c1c1e",
                "text": "#ffffff",
                "primary": "#58a6ff"
            })
        );
        let changed = controller.update_appearance(NativeAppearanceSnapshot {
            dark: false,
            increased_contrast: true,
            reduced_transparency: true,
            accent_red: 0,
            accent_green: 102,
            accent_blue: 204,
        });
        assert!(changed.accepted);
        assert_eq!(changed.attempted, 1);
        assert_eq!(changed.delivered, 1);

        let schema = serde_json::json!({
            "$version": 1,
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["quiet", "loud"],
                    "default": "quiet",
                    "x-napplet-section": "appearance"
                },
                "enabled": {"type": "boolean", "default": true}
            },
            "additionalProperties": false
        });
        controller.mapped_envelope(
            session,
            serde_json::to_vec(&serde_json::json!({
                "type": "config.registerSchema",
                "id": "schema-1",
                "schema": schema,
                "version": 1
            }))
            .unwrap(),
        );
        assert_eq!(
            response_of_type(&controller, "config.registerSchema.result")["ok"],
            true
        );
        controller.mapped_envelope(session, br#"{"type":"config.subscribe"}"#.to_vec());
        controller.mapped_envelope(
            session,
            br#"{"type":"config.openSettings","section":"appearance"}"#.to_vec(),
        );
        let request = requests.lock().pop().expect("native settings request");
        assert_eq!(request.manifest_author, AUTHOR);
        assert_eq!(request.d_tag, "good-morning");
        assert_eq!(request.session_id, session);
        assert_eq!(request.section.as_deref(), Some("appearance"));
        assert!(request.schema_json.len() <= 192 * 1_024);
        assert!(request.values_json.len() <= 192 * 1_024);

        let commit = controller.commit_config_values(NativeConfigCommit {
            manifest_author: request.manifest_author,
            d_tag: request.d_tag,
            aggregate_hash: request.aggregate_hash,
            session_id: request.session_id,
            values_json: r#"{"enabled":false,"mode":"loud"}"#.to_owned(),
        });
        assert!(commit.accepted);
        assert_eq!(commit.attempted, 1);
        assert_eq!(commit.delivered, 1);
        controller.mapped_envelope(session, br#"{"type":"config.get","id":"get-1"}"#.to_vec());
        assert_eq!(
            response_of_type(&controller, "config.values")["values"],
            serde_json::json!({"enabled": false, "mode": "loud"})
        );

        controller.stop(session);
        let refused = controller.commit_config_values(NativeConfigCommit {
            manifest_author: AUTHOR.to_owned(),
            d_tag: "good-morning".to_owned(),
            aggregate_hash: "828a6df02afd56782ea20f805084acce65c53f7c37554948c1e0a64aa5a2b0a8"
                .to_owned(),
            session_id: session,
            values_json: r#"{"enabled":true,"mode":"quiet"}"#.to_owned(),
        });
        assert!(!refused.accepted);
        assert_eq!(refused.refusal.unwrap().code, "settings-session-closed");

        controller.close();
        drop(controller);
        let reopened = controller_with_native_capabilities(&temp, Arc::new(Mutex::new(Vec::new())));
        let (_, reopened_session) = install_and_launch(&reopened, &["config"]);
        reopened.mapped_envelope(
            reopened_session,
            br#"{"type":"config.get","id":"get-after-restart"}"#.to_vec(),
        );
        assert_eq!(
            response_of_type(&reopened, "config.values")["values"],
            serde_json::json!({"enabled": false, "mode": "loud"})
        );
    }

    #[test]
    fn typed_workspace_round_trips_only_through_rust_owned_storage() {
        let temp = TempDir::new().unwrap();
        let runtime = controller(&temp);
        let expected = workspace_definition("primary");
        let saved = runtime.save_workspace(expected.clone());
        assert!(saved.accepted);
        assert_eq!(saved.workspace, Some(expected.clone()));
        assert_eq!(
            runtime.snapshot().workspaces,
            std::slice::from_ref(&expected)
        );
        runtime.close();
        drop(runtime);

        let reopened = controller(&temp);
        assert!(reopened.snapshot().workspaces.is_empty());
        let restored = reopened.restore_workspaces();
        assert!(restored.accepted);
        assert_eq!(restored.workspaces, std::slice::from_ref(&expected));
        assert_eq!(reopened.snapshot().workspaces, [expected]);
    }

    #[test]
    fn workspace_validation_refuses_unknown_duplicate_and_oversized_input() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);

        let mut unknown = workspace_definition("unknown");
        unknown.schema_version = WORKSPACE_SCHEMA_VERSION + 1;
        let refusal = controller.save_workspace(unknown).refusal.unwrap();
        assert_eq!(refusal.code, "invalid-workspace");

        let mut duplicate = workspace_definition("duplicate");
        duplicate.slots[1].slot_id = duplicate.slots[0].slot_id.clone();
        let refusal = controller.save_workspace(duplicate).refusal.unwrap();
        assert_eq!(refusal.code, "invalid-workspace");
        assert!(refusal.detail.contains("duplicate workspace slot id"));

        let mut oversized = workspace_definition("oversized");
        oversized.preferences_json = format!(
            r#"{{"value":"{}"}}"#,
            "x".repeat(MAXIMUM_WORKSPACE_FIELD_BYTES)
        );
        let refusal = controller.save_workspace(oversized).refusal.unwrap();
        assert_eq!(refusal.code, "invalid-workspace");
        assert!(refusal.detail.contains("maximum"));
        assert!(controller.snapshot().workspaces.is_empty());
    }

    #[test]
    fn workspace_restore_is_all_or_nothing_for_corrupt_or_future_rows() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);
        let valid = workspace_record_from_ffi(workspace_definition("a-valid")).unwrap();
        controller.runtime_store.save_workspace(&valid).unwrap();

        let mut future_value: serde_json::Value =
            serde_json::from_str(valid.definition.as_str()).unwrap();
        future_value["schema_version"] =
            serde_json::json!(WORKSPACE_SCHEMA_VERSION.saturating_add(1));
        controller
            .runtime_store
            .save_workspace(&WorkspaceRecord {
                id: Arc::from("z-future"),
                definition: BoundedJson::from_value(&future_value, MAXIMUM_WORKSPACE_JSON_BYTES)
                    .unwrap(),
                retained_receipts: Vec::new(),
            })
            .unwrap();

        let restored = controller.restore_workspaces();
        assert!(!restored.accepted);
        assert!(restored.workspaces.is_empty());
        assert_eq!(restored.refusal.unwrap().code, "invalid-workspace");
        assert!(controller.snapshot().workspaces.is_empty());
    }

    #[test]
    fn local_account_lifecycle_is_explicit_typed_and_stale_safe() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);

        let invalid = controller.register_local_account("not-a-secret".to_owned());
        assert!(!invalid.accepted);
        assert_eq!(
            invalid.failure,
            Some(RuntimeAccountFailure::InvalidSecretKey)
        );

        let registered = controller.register_local_account(format!("{:064x}", 7_u8));
        assert!(registered.accepted);
        let first = registered.handle.unwrap();
        assert_eq!(first.kind, RuntimeAccountKind::LocalSigner);
        assert_eq!(
            registered.snapshot.unwrap().active_public_key,
            None,
            "registration must not silently switch identity"
        );

        let activated = controller.activate_local_account(first.clone());
        assert!(activated.accepted);
        assert_eq!(
            activated.snapshot.unwrap().active_public_key.as_deref(),
            Some(first.public_key.as_str())
        );

        let replacement = controller
            .register_local_account(format!("{:064x}", 7_u8))
            .handle
            .unwrap();
        assert_ne!(first.installation_id, replacement.installation_id);
        let stale = controller.remove_local_account(first);
        assert!(!stale.accepted);
        assert_eq!(
            stale.failure,
            Some(RuntimeAccountFailure::StaleInstallation)
        );

        let activated = controller.activate_local_account(replacement.clone());
        assert!(activated.accepted);
        assert_eq!(
            controller
                .logout_local_account()
                .snapshot
                .unwrap()
                .active_public_key,
            None
        );
        let removed = controller.remove_local_account(replacement);
        assert!(removed.accepted);
        assert!(removed.snapshot.unwrap().local_accounts.is_empty());

        controller.close();
        assert_eq!(
            controller.account_snapshot().failure,
            Some(RuntimeAccountFailure::Closed)
        );
    }

    #[test]
    fn read_only_account_lifecycle_is_keyless_typed_and_explicit() {
        let temp = TempDir::new().unwrap();
        let controller = controller(&temp);
        let npub = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6";

        let registered = controller.register_read_only_account(npub.to_owned());
        assert!(registered.accepted);
        let handle = registered.handle.unwrap();
        assert_eq!(handle.kind, RuntimeAccountKind::ReadOnly);
        assert_eq!(handle.public_key.len(), 64);
        assert_eq!(
            registered.snapshot.unwrap().active_public_key,
            None,
            "read-only registration must not silently switch identity"
        );

        let activated = controller.activate_local_account(handle.clone());
        assert!(activated.accepted);
        assert_eq!(
            activated.snapshot.unwrap().active_public_key.as_deref(),
            Some(handle.public_key.as_str())
        );
        assert_eq!(
            controller
                .logout_local_account()
                .snapshot
                .unwrap()
                .active_public_key,
            None
        );
        assert!(controller.remove_local_account(handle).accepted);
        assert!(
            controller
                .account_snapshot()
                .snapshot
                .unwrap()
                .local_accounts
                .is_empty()
        );

        let nip05 = controller.register_read_only_account("pablo@example.com".to_owned());
        assert_eq!(
            nip05.failure,
            Some(RuntimeAccountFailure::Nip05ResolutionUnavailable)
        );
        let invalid = controller.register_read_only_account("not-a-key".to_owned());
        assert_eq!(
            invalid.failure,
            Some(RuntimeAccountFailure::InvalidPublicKey)
        );
    }

    #[test]
    fn native_inc_actions_cross_ffi_with_trusted_origin_and_teardown() {
        let temp = TempDir::new().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let ends = Arc::new(Mutex::new(Vec::new()));
        let controller = controller_with_all_native_capabilities(
            &temp,
            Arc::clone(&requests),
            Arc::clone(&ends),
            NativeIncActionEnqueueResult::Accepted,
        );
        let (_, session) = install_and_launch(&controller, &["inc"]);
        controller.mapped_envelope(
            session,
            serde_json::to_vec(&serde_json::json!({
                "type": "inc.emit",
                "topic": "profile:open",
                "payload": {"pubkey": AUTHOR}
            }))
            .unwrap(),
        );
        let request = requests.lock().pop().expect("native action request");
        assert_eq!(request.manifest_author, AUTHOR);
        assert_eq!(request.d_tag, "good-morning");
        assert_eq!(request.session_id, session);
        assert_eq!(request.source_window_id, session);
        assert_eq!(request.kind, "profile-open");
        assert_eq!(
            serde_json::from_str::<Value>(&request.payload_json).unwrap(),
            serde_json::json!({"pubkey": AUTHOR})
        );

        controller.stop(session);
        let end = ends.lock().pop().expect("session teardown callback");
        assert_eq!(end.session_id, session);
        assert!(end.reason.starts_with("closed-"));
    }

    #[test]
    fn native_inc_action_backpressure_is_an_exact_provider_refusal() {
        let temp = TempDir::new().unwrap();
        let controller = controller_with_all_native_capabilities(
            &temp,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            NativeIncActionEnqueueResult::Backpressure,
        );
        let (_, session) = install_and_launch(&controller, &["inc"]);
        controller.mapped_envelope(
            session,
            serde_json::to_vec(&serde_json::json!({
                "type": "inc.emit",
                "topic": "profile:open",
                "payload": {"pubkey": AUTHOR}
            }))
            .unwrap(),
        );
        let error = controller
            .snapshot()
            .recent_errors
            .into_iter()
            .last()
            .expect("provider refusal fact");
        assert_eq!(error.code, "bridge");
        assert!(
            error.detail.contains("native action capacity is full"),
            "unexpected refusal detail: {}",
            error.detail
        );
    }

    #[test]
    fn catalog_coordinates_are_parsed_only_at_the_rust_boundary() {
        let author = "a".repeat(64);
        let event_id = "b".repeat(64);
        assert!(matches!(
            parse_catalog_coordinate(&format!("35129:{author}:good-morning")).unwrap(),
            ManifestCoordinate::Named { .. }
        ));
        assert!(matches!(
            parse_catalog_coordinate(&format!("15129:{author}")).unwrap(),
            ManifestCoordinate::Root { .. }
        ));
        assert!(matches!(
            parse_catalog_coordinate(&format!("5129:{event_id}:{author}")).unwrap(),
            ManifestCoordinate::Snapshot { .. }
        ));
        for invalid in [
            "",
            "35129:author",
            "15129:author:extra",
            "unknown:author:d-tag",
            " 35129:author:d-tag",
        ] {
            assert!(
                parse_catalog_coordinate(invalid).is_err(),
                "unexpectedly accepted {invalid:?}"
            );
        }
    }
}
