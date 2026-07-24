use std::{collections::BTreeSet, fmt, net::IpAddr, sync::Arc};

use nmp_native_runtime_core::{Cancellation, Principal, SessionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_MAXIMUM_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
pub const DEFAULT_MAXIMUM_SVG_BYTES: usize = 5 * 1024 * 1024;
pub const DEFAULT_MAXIMUM_BLOB_BYTES: usize = 50 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceProviderLimits {
    pub maximum_sessions: usize,
    pub maximum_requests_per_napplet_per_minute: u32,
    pub maximum_in_flight_urls_per_napplet: usize,
    pub maximum_total_in_flight_urls: usize,
    pub maximum_urls_per_bulk: usize,
    pub maximum_url_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
    pub maximum_response_bytes: usize,
    pub maximum_bulk_response_bytes: usize,
    pub maximum_blob_bytes_per_request: usize,
    pub maximum_redirects: usize,
    pub maximum_resolved_addresses: usize,
    pub fetch_timeout_millis: u64,
    pub maximum_svg_bytes: usize,
    pub maximum_svg_output_dimension: u32,
    pub svg_timeout_millis: u64,
    pub maximum_blossom_servers: usize,
}

impl Default for ResourceProviderLimits {
    fn default() -> Self {
        Self {
            maximum_sessions: 64,
            maximum_requests_per_napplet_per_minute: 60,
            maximum_in_flight_urls_per_napplet: 10,
            maximum_total_in_flight_urls: 128,
            maximum_urls_per_bulk: 100,
            maximum_url_bytes: 16 * 1024,
            maximum_correlation_id_bytes: 1_024,
            maximum_response_bytes: DEFAULT_MAXIMUM_RESPONSE_BYTES,
            maximum_bulk_response_bytes: DEFAULT_MAXIMUM_BLOB_BYTES,
            maximum_blob_bytes_per_request: DEFAULT_MAXIMUM_BLOB_BYTES,
            maximum_redirects: 5,
            maximum_resolved_addresses: 32,
            fetch_timeout_millis: 30_000,
            maximum_svg_bytes: DEFAULT_MAXIMUM_SVG_BYTES,
            maximum_svg_output_dimension: 4_096,
            svg_timeout_millis: 2_000,
            maximum_blossom_servers: 16,
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ResourceProviderBuildError {
    #[error("resource-provider limits must be finite, non-zero, and internally consistent")]
    InvalidLimits,
    #[error("at least one bounded HTTPS Blossom server is required")]
    MissingBlossomServer,
    #[error("invalid Blossom server {server}: {reason}")]
    InvalidBlossomServer { server: Arc<str>, reason: Arc<str> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceScheme {
    Data,
    Https,
    Blossom,
}

impl ResourceScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Https => "https",
            Self::Blossom => "blossom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceErrorCode {
    InvalidRequest,
    NotFound,
    BlockedByPolicy,
    Timeout,
    TooLarge,
    UnsupportedScheme,
    DecodeFailed,
    NetworkError,
    QuotaExceeded,
}

impl ResourceErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid-request",
            Self::NotFound => "not-found",
            Self::BlockedByPolicy => "blocked-by-policy",
            Self::Timeout => "timeout",
            Self::TooLarge => "too-large",
            Self::UnsupportedScheme => "unsupported-scheme",
            Self::DecodeFailed => "decode-failed",
            Self::NetworkError => "network-error",
            Self::QuotaExceeded => "quota-exceeded",
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{code:?}: {message}")]
pub struct ResourceFailure {
    pub code: ResourceErrorCode,
    pub message: Arc<str>,
}

impl ResourceFailure {
    pub fn new(code: ResourceErrorCode, message: impl Into<Arc<str>>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceDeadline {
    pub monotonic_millis: u64,
}

pub trait ResourceClock: Send + Sync + fmt::Debug {
    fn monotonic_millis(&self) -> u64;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveRequest {
    pub host: Arc<str>,
    pub port: u16,
    pub deadline: ResourceDeadline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedHttpsRequest {
    pub url: Arc<str>,
    pub host: Arc<str>,
    pub port: u16,
    pub approved_addresses: Arc<[IpAddr]>,
    pub maximum_body_bytes: usize,
    pub deadline: ResourceDeadline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawHttpsResponse {
    pub status: u16,
    pub location: Option<Arc<str>>,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ResourceNetworkError {
    #[error("operation was cancelled")]
    Cancelled,
    #[error("operation timed out")]
    Timeout,
    #[error("resource was not found")]
    NotFound,
    #[error("network acquisition failed")]
    Failed,
    #[error("network response exceeded the supplied byte ceiling")]
    TooLarge,
}

/// Raw OS/network capability seam.
///
/// Rust validates every URL and resolved address and supplies the exact
/// admitted addresses. An implementation MUST pin the connection to
/// `approved_addresses`, preserve `host` for TLS/SNI and the HTTP Host header,
/// enforce `maximum_body_bytes` while reading, and stop promptly when
/// `cancellation` is signalled. It must not follow redirects.
pub trait ResourceNetwork: Send + Sync + fmt::Debug {
    fn resolve(
        &self,
        request: &ResolveRequest,
        cancellation: &Cancellation,
    ) -> Result<Vec<IpAddr>, ResourceNetworkError>;

    fn get(
        &self,
        request: &PinnedHttpsRequest,
        cancellation: &Cancellation,
    ) -> Result<RawHttpsResponse, ResourceNetworkError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvgRasterRequest {
    pub source: Arc<[u8]>,
    pub maximum_dimension: u32,
    pub maximum_output_bytes: usize,
    pub deadline: ResourceDeadline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RasterizedSvg {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum SvgRasterError {
    #[error("operation was cancelled")]
    Cancelled,
    #[error("rasterization timed out")]
    Timeout,
    #[error("SVG input could not be decoded safely")]
    DecodeFailed,
    #[error("rasterized output exceeded its limits")]
    TooLarge,
}

/// Sandboxed no-network SVG capability.
///
/// The implementation receives bytes only after Rust classifies them as SVG.
/// It MUST expose no network/file capability and MUST enforce the supplied
/// deadline and cancellation signal. Rust independently checks the returned
/// dimensions, bytes and MIME before delivery.
pub trait SvgRasterizer: Send + Sync + fmt::Debug {
    fn rasterize(
        &self,
        request: &SvgRasterRequest,
        cancellation: &Cancellation,
    ) -> Result<RasterizedSvg, SvgRasterError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceActivityAction {
    Info,
    Bytes,
    BytesMany,
    Cancel,
    LifecycleCleanup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceActivityOutcome {
    Completed,
    Active,
    Refused(ResourceErrorCode),
    Cancelled,
    PushRefused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceActivity {
    pub principal: Principal,
    pub session: SessionId,
    pub action: ResourceActivityAction,
    pub outcome: ResourceActivityOutcome,
    pub url_count: usize,
    pub delivered_bytes: usize,
}

/// Bounded synchronous activity sink. Facts intentionally omit URLs, DNS
/// names, IP addresses and resource bytes.
pub trait ResourceActivitySink: Send + Sync + fmt::Debug {
    fn record(&self, fact: ResourceActivity);
}

#[derive(Debug, Default)]
pub struct NoopResourceActivity;

impl ResourceActivitySink for NoopResourceActivity {
    fn record(&self, _fact: ResourceActivity) {}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AcquiredResource {
    pub bytes: Vec<u8>,
    pub mime: Arc<str>,
}

pub(crate) fn validate_limits(limits: ResourceProviderLimits) -> bool {
    let values = [
        limits.maximum_sessions,
        usize::try_from(limits.maximum_requests_per_napplet_per_minute).unwrap_or(0),
        limits.maximum_in_flight_urls_per_napplet,
        limits.maximum_total_in_flight_urls,
        limits.maximum_urls_per_bulk,
        limits.maximum_url_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_response_bytes,
        limits.maximum_bulk_response_bytes,
        limits.maximum_blob_bytes_per_request,
        limits.maximum_redirects,
        limits.maximum_resolved_addresses,
        usize::try_from(limits.fetch_timeout_millis).unwrap_or(0),
        limits.maximum_svg_bytes,
        usize::try_from(limits.maximum_svg_output_dimension).unwrap_or(0),
        usize::try_from(limits.svg_timeout_millis).unwrap_or(0),
        limits.maximum_blossom_servers,
    ];
    values.into_iter().all(|value| value > 0)
        && limits.maximum_in_flight_urls_per_napplet <= limits.maximum_total_in_flight_urls
        && limits.maximum_response_bytes <= limits.maximum_blob_bytes_per_request
        && limits.maximum_svg_bytes <= limits.maximum_response_bytes
        && limits.maximum_bulk_response_bytes <= limits.maximum_blob_bytes_per_request
}

pub(crate) fn supported_schemes() -> BTreeSet<ResourceScheme> {
    BTreeSet::from([
        ResourceScheme::Data,
        ResourceScheme::Https,
        ResourceScheme::Blossom,
    ])
}
