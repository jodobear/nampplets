use std::{fmt, sync::Arc};

use nmp_native_nap_bridge::ProviderPushError;
use nmp_native_runtime_core::{Cancellation, Principal, SessionId};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkProviderLimits {
    pub maximum_sessions: usize,
    pub maximum_pending_per_session: usize,
    pub maximum_pending_total: usize,
    pub maximum_url_bytes: usize,
    pub maximum_label_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
    pub maximum_native_handle_bytes: usize,
    pub maximum_response_bytes: usize,
}

impl Default for LinkProviderLimits {
    fn default() -> Self {
        Self {
            maximum_sessions: 64,
            maximum_pending_per_session: 8,
            maximum_pending_total: 128,
            maximum_url_bytes: 8 * 1024,
            maximum_label_bytes: 4 * 1024,
            maximum_correlation_id_bytes: 1_024,
            maximum_native_handle_bytes: 256,
            maximum_response_bytes: 16 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPolicyRequest {
    pub principal: Principal,
    pub session: SessionId,
    pub normalized_url: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkPolicyDecision {
    Allow,
    Deny,
}

/// Product policy runs after structural URL validation and before native work.
pub trait LinkPolicy: Send + Sync + fmt::Debug {
    fn evaluate(&self, request: &LinkPolicyRequest) -> LinkPolicyDecision;
}

#[derive(Debug, Default)]
pub struct AllowExternalWebLinks;

impl LinkPolicy for AllowExternalWebLinks {
    fn evaluate(&self, _request: &LinkPolicyRequest) -> LinkPolicyDecision {
        LinkPolicyDecision::Allow
    }
}

#[derive(Clone, Debug)]
pub struct NativeLinkOpenRequest {
    pub token: LinkOperationToken,
    pub principal: Principal,
    pub session: SessionId,
    pub normalized_url: Arc<str>,
    /// Untrusted display text supplied by the napplet. Native code may render
    /// it as plain text but must never use it as policy or navigation input.
    pub label: Option<Arc<str>>,
    /// External link opens always require shell-owned user confirmation.
    pub confirmation_required: bool,
    pub cancellation: Cancellation,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum NativeLinkStartError {
    #[error("native link opener is saturated")]
    Saturated,
    #[error("native link opener is unavailable")]
    Unavailable,
    #[error("native link session is closed")]
    Closed,
}

/// Native executes only the exact, normalized URL supplied by Rust.
///
/// `try_open` must return without waiting for UI. Completion is reported to
/// [`LinkProvider::complete`]. `cancel` is idempotent and nonblocking.
pub trait NativeLinkOpener: Send + Sync + fmt::Debug {
    fn try_open(&self, request: NativeLinkOpenRequest) -> Result<Arc<str>, NativeLinkStartError>;
    fn cancel(&self, native_handle: &str);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkOperationToken(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeLinkOutcome {
    Opened,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkActivityOutcome {
    Started,
    Opened,
    Cancelled,
    Denied,
    Refused,
    PushRefused,
    LifecycleCancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkActivity {
    pub principal: Principal,
    pub session: SessionId,
    pub outcome: LinkActivityOutcome,
}

pub trait LinkActivitySink: Send + Sync + fmt::Debug {
    fn record(&self, fact: LinkActivity);
}

#[derive(Debug, Default)]
pub struct NoopLinkActivity;

impl LinkActivitySink for NoopLinkActivity {
    fn record(&self, _fact: LinkActivity) {}
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LinkProviderBuildError {
    #[error("link provider limits must be finite, non-zero, and internally consistent")]
    InvalidLimits,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LinkCompletionError {
    #[error("unknown or already-completed link operation")]
    UnknownOperation,
    #[error("link result delivery was refused: {0}")]
    Push(ProviderPushError),
}
