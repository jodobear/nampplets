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

use nmp::EngineConfig;
use nmp_native_artifact::{
    ArtifactLimits, ArtifactMode, ArtifactSourcePolicy, BlobFetchRequest, BlobFetchResponse,
    BlobSourceError, FileArtifactCache, ManifestBlobSource, ManifestCoordinate,
    ManifestEventLimits, ManifestEventVerifier, SignedArtifactResolver, VerifiedArtifactHandle,
};
use nmp_native_nap_bridge::{BridgeLimits, Provider};
use nmp_native_nmp_adapter::NmpDataPlane;
use nmp_native_providers::{
    ShellEnvironment, ShellEnvironmentError, ShellEnvironmentLimits, ShellEnvironmentSource,
    ShellProvider, ShellProviderLimits, StorageProvider, StorageProviderLimits,
};
use nmp_native_runtime_app::{
    AppLimits, AppSnapshot, ExecutableArtifact, KernelClock, PlatformCommand, PlatformEvent,
    RuntimeApp, RuntimeAppConfig,
};
use nmp_native_runtime_core::{
    BoundedJson, Capability, ExecutionProfile, GrantDecision, GrantLimits, Principal,
    ResourceLimits, Sensitivity, SessionId,
};
use nmp_native_runtime_store::{InstalledBuild, RuntimeStore, StoreLimits};
use nmp_native_surface::BindingLimits;
use parking_lot::Mutex;
use tokio::sync::watch;

const DEFAULT_MAXIMUM_CONFIG_STRING_BYTES: u64 = 16 * 1_024;
const DEFAULT_MAXIMUM_CONFIG_ITEMS: u64 = 64;
const DEFAULT_MAXIMUM_MANIFEST_BYTES: u64 = 256 * 1_024;
const DEFAULT_MAXIMUM_ARTIFACT_READ_BYTES: u64 = 8 * 1_024 * 1_024;
const DEFAULT_MAXIMUM_OBSERVERS: u64 = 8;
const DEFAULT_MAXIMUM_BOUNDARY_EVENTS: u64 = 256;

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

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeExecutionProfile {
    Legacy,
    Renderer,
    Hybrid,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
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
    pub sessions: Vec<RuntimeSessionSnapshot>,
    pub bindings: Vec<RuntimeBindingSnapshot>,
    pub receipts: Vec<RuntimeReceiptSnapshot>,
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
    artifact_cache: FileArtifactCache,
    artifact_source: CallbackArtifactSource,
    artifact_limits: ArtifactLimits,
    maximum_manifest_bytes: usize,
    maximum_verified_read_bytes: usize,
    maximum_blob_sources: usize,
    maximum_command_items: usize,
    maximum_command_string_bytes: usize,
    maximum_envelope_bytes: usize,
    artifacts: Mutex<BTreeMap<Principal, Arc<VerifiedArtifactHandle>>>,
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

#[uniffi::export]
impl RuntimeController {
    #[uniffi::constructor]
    pub fn open(
        config: RuntimeConfig,
        artifact_source: Box<dyn ArtifactSource>,
    ) -> Result<Arc<Self>, RuntimeOpenError> {
        let config = config.validated()?;
        let runtime_store = Arc::new(
            RuntimeStore::open(&config.runtime_store_path, StoreLimits::default()).map_err(
                |error| RuntimeOpenError::RuntimeStore {
                    detail: error.to_string(),
                },
            )?,
        );
        let artifact_cache =
            FileArtifactCache::open(&config.artifact_cache_path).map_err(|error| {
                RuntimeOpenError::ArtifactCache {
                    detail: error.to_string(),
                }
            })?;
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
        let app_limits = AppLimits::default();
        let maximum_envelope_bytes = app_limits.maximum_envelope_bytes;
        let app = RuntimeApp::open(RuntimeAppConfig {
            limits: app_limits,
            resource_limits: ResourceLimits::default(),
            grant_limits: GrantLimits::default(),
            bridge_limits: BridgeLimits::default(),
            binding_limits: BindingLimits::default(),
            store: runtime_store,
            data_plane: data_plane.clone(),
            clock: Arc::new(SystemClock),
            shell_provider,
            providers: vec![storage_provider],
        })
        .map_err(|error| RuntimeOpenError::Runtime {
            detail: error.to_string(),
        })?;
        let (signal, _) = watch::channel(0_u64);
        Ok(Arc::new(Self {
            app,
            data_plane,
            artifact_cache,
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
            artifacts: Mutex::new(BTreeMap::new()),
            boundary_refusals: Mutex::new(VecDeque::with_capacity(config.maximum_boundary_events)),
            maximum_boundary_events: config.maximum_boundary_events,
            signal,
            observers: Arc::new(AtomicUsize::new(0)),
            maximum_observers: config.maximum_observers,
            closed: AtomicBool::new(false),
        }))
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
        });
        let metadata = match BoundedJson::from_value(&metadata, 256 * 1_024) {
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
        self.artifacts
            .lock()
            .insert(principal.clone(), Arc::clone(&artifact.handle));
        let executable: Arc<dyn ExecutableArtifact> = artifact.handle.clone();
        self.app.dispatch(PlatformCommand::InstallVerified {
            build: InstalledBuild {
                principal,
                title,
                manifest_metadata: metadata,
            },
            artifact: executable,
        });
        bump_signal(&self.signal);
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
        let requirements = artifact
            .handle
            .manifest()
            .requirements()
            .collect::<Vec<_>>();
        if requirements.len() > self.maximum_command_items {
            self.record_refusal(
                "required-domain-capacity",
                format!(
                    "verified manifest requires {} domains; the maximum is {}",
                    requirements.len(),
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
        for domain in requirements {
            let domain = match Capability::new(domain) {
                Ok(domain) => domain,
                Err(error) => {
                    self.record_refusal("invalid-capability", error.to_string());
                    return;
                }
            };
            domains.insert(domain);
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
            self.app.dispatch(PlatformCommand::Close);
            self.data_plane.close();
            bump_signal(&self.signal);
        }
    }
}

impl RuntimeController {
    fn refusal(&self, code: impl Into<String>, detail: impl Into<String>) -> RuntimeRefusal {
        RuntimeRefusal {
            code: code.into(),
            detail: detail.into(),
            occurred_at_millis: now_millis(),
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

fn map_profile(profile: RuntimeExecutionProfile) -> ExecutionProfile {
    match profile {
        RuntimeExecutionProfile::Legacy => ExecutionProfile::Legacy,
        RuntimeExecutionProfile::Renderer => ExecutionProfile::Renderer,
        RuntimeExecutionProfile::Hybrid => ExecutionProfile::Hybrid,
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
        PlatformEvent::GrantChanged { .. } => "grant-changed",
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
        PlatformEvent::BindingOpened { .. } => "binding-opened",
        PlatformEvent::BindingClosed { .. } => "binding-closed",
        PlatformEvent::WriteAccepted { .. } => "write-accepted",
        PlatformEvent::WorkspaceSaved { .. } => "workspace-saved",
        PlatformEvent::WorkspaceRestored { .. } => "workspace-restored",
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
        controller.launch(artifact, RuntimeExecutionProfile::Legacy);
        let runtime_snapshot = controller.snapshot();
        assert_eq!(runtime_snapshot.sessions[0].domains, ["shell"]);
        let session = runtime_snapshot.sessions[0].id;
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
}
