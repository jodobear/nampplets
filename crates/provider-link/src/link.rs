use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushError, ProviderPushSender, ProviderRequest, ProviderSession,
    ProviderSessionContext, ProviderSessionEnd,
};
use nmp_native_runtime_core::{
    BoundedJson, Cancellation, Capability, Principal, SessionId, WorkLease,
};
use parking_lot::Mutex;
use serde_json::Value;
use thiserror::Error;
use url::{Host, Url};

use crate::PINNED_NAP_PROTOCOL;

pub const LINK_DOMAIN: &str = "link";

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

#[derive(Debug)]
pub struct LinkProvider {
    policy: Arc<dyn LinkPolicy>,
    opener: Arc<dyn NativeLinkOpener>,
    activity: Arc<dyn LinkActivitySink>,
    limits: LinkProviderLimits,
    descriptor: ProviderDescriptor,
    state: Mutex<LinkState>,
}

#[derive(Debug, Default)]
struct LinkState {
    sessions: BTreeMap<SessionId, LinkSession>,
    pending: BTreeMap<LinkOperationToken, PendingLink>,
    next_token: u64,
}

#[derive(Clone, Debug)]
struct LinkSession {
    principal: Principal,
    outbound: ProviderPushSender,
    ready: bool,
}

#[derive(Debug)]
struct PendingLink {
    principal: Principal,
    session: SessionId,
    correlation_id: Arc<str>,
    native_handle: Option<Arc<str>>,
    work: WorkLease,
}

impl LinkProvider {
    pub fn new(
        policy: Arc<dyn LinkPolicy>,
        opener: Arc<dyn NativeLinkOpener>,
        activity: Arc<dyn LinkActivitySink>,
        limits: LinkProviderLimits,
    ) -> Result<Self, LinkProviderBuildError> {
        validate_limits(limits)?;
        Ok(Self {
            policy,
            opener,
            activity,
            limits,
            descriptor: ProviderDescriptor {
                domain: Capability::new(LINK_DOMAIN).expect("static link capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: BTreeSet::from([Arc::from("open")]),
                sensitive: true,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            state: Mutex::new(LinkState::default()),
        })
    }

    pub fn pending_count(&self) -> usize {
        self.state.lock().pending.len()
    }

    pub fn complete(
        &self,
        token: LinkOperationToken,
        outcome: NativeLinkOutcome,
    ) -> Result<(), LinkCompletionError> {
        let (pending, outbound) = {
            let mut state = self.state.lock();
            let pending = state
                .pending
                .remove(&token)
                .ok_or(LinkCompletionError::UnknownOperation)?;
            let outbound = state
                .sessions
                .get(&pending.session)
                .filter(|session| session.ready && session.principal == pending.principal)
                .map(|session| session.outbound.clone());
            (pending, outbound)
        };
        let activity_outcome = match outcome {
            NativeLinkOutcome::Opened => LinkActivityOutcome::Opened,
            NativeLinkOutcome::Cancelled => LinkActivityOutcome::Cancelled,
            NativeLinkOutcome::Failed => LinkActivityOutcome::Refused,
        };
        self.activity.record(LinkActivity {
            principal: pending.principal.clone(),
            session: pending.session,
            outcome: activity_outcome,
        });
        drop(pending.work);
        let Some(outbound) = outbound else {
            return Ok(());
        };
        let terminal = match outcome {
            NativeLinkOutcome::Opened => LinkTerminal::Opened,
            NativeLinkOutcome::Cancelled => LinkTerminal::Denied {
                error: "user-denied",
            },
            NativeLinkOutcome::Failed => LinkTerminal::Rejected {
                error: "native-open-failed",
            },
        };
        let fields = terminal_fields(&pending.correlation_id, terminal);
        outbound
            .push("link.open.result", fields, None)
            .map(|_| ())
            .map_err(|error| {
                self.activity.record(LinkActivity {
                    principal: pending.principal,
                    session: pending.session,
                    outcome: LinkActivityOutcome::PushRefused,
                });
                LinkCompletionError::Push(error)
            })
    }

    fn open(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id: Arc<str> = Arc::from(correlation_id(&request, self.limits)?);
        let request_action = Arc::clone(&request.action);
        let object = request
            .payload
            .as_object()
            .ok_or_else(|| invalid(&request, "payload must be an object"))?;
        if !object.contains_key("url")
            || object
                .keys()
                .any(|key| !matches!(key.as_str(), "url" | "options"))
        {
            return Err(invalid(
                &request,
                "only `url` and optional `options` are allowed",
            ));
        }
        let url = object
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(&request, "`url` must be a string"))?;
        let label = parse_label(object.get("options"), self.limits.maximum_label_bytes)
            .map_err(|reason| invalid(&request, reason))?;
        let normalized_url = match validate_external_url(url, self.limits.maximum_url_bytes) {
            Ok(url) => url,
            Err(refusal) => {
                self.activity.record(LinkActivity {
                    principal: request.principal.clone(),
                    session: request.session,
                    outcome: LinkActivityOutcome::Denied,
                });
                return terminal_response(
                    &id,
                    LinkTerminal::Denied {
                        error: refusal.code(),
                    },
                    self.limits.maximum_response_bytes,
                    &request_action,
                );
            }
        };
        let policy_request = LinkPolicyRequest {
            principal: request.principal.clone(),
            session: request.session,
            normalized_url: Arc::clone(&normalized_url),
        };
        if self.policy.evaluate(&policy_request) == LinkPolicyDecision::Deny {
            self.activity.record(LinkActivity {
                principal: request.principal.clone(),
                session: request.session,
                outcome: LinkActivityOutcome::Denied,
            });
            return terminal_response(
                &id,
                LinkTerminal::Denied {
                    error: "blocked-by-policy",
                },
                self.limits.maximum_response_bytes,
                &request_action,
            );
        }

        let cancellation = request.work.cancellation().clone();
        let token =
            {
                let mut state = self.state.lock();
                ensure_ready_link_session(&state, &request)?;
                if state.pending.len() >= self.limits.maximum_pending_total
                    || state
                        .pending
                        .values()
                        .filter(|pending| pending.session == request.session)
                        .count()
                        >= self.limits.maximum_pending_per_session
                {
                    self.activity.record(LinkActivity {
                        principal: request.principal.clone(),
                        session: request.session,
                        outcome: LinkActivityOutcome::Refused,
                    });
                    return terminal_response(
                        &id,
                        LinkTerminal::Rejected {
                            error: "link operation capacity is full",
                        },
                        self.limits.maximum_response_bytes,
                        &request_action,
                    );
                }
                if state.pending.values().any(|pending| {
                    pending.session == request.session && pending.correlation_id == id
                }) {
                    return terminal_response(
                        &id,
                        LinkTerminal::Rejected {
                            error: "duplicate outstanding correlation id",
                        },
                        self.limits.maximum_response_bytes,
                        &request_action,
                    );
                }
                let Some(next) = state.next_token.checked_add(1) else {
                    return terminal_response(
                        &id,
                        LinkTerminal::Rejected {
                            error: "link operation id space is exhausted",
                        },
                        self.limits.maximum_response_bytes,
                        &request_action,
                    );
                };
                state.next_token = next;
                let token = LinkOperationToken(next);
                state.pending.insert(
                    token,
                    PendingLink {
                        principal: request.principal.clone(),
                        session: request.session,
                        correlation_id: Arc::clone(&id),
                        native_handle: None,
                        work: request.work,
                    },
                );
                token
            };
        let native_request = NativeLinkOpenRequest {
            token,
            principal: request.principal.clone(),
            session: request.session,
            normalized_url,
            label,
            confirmation_required: true,
            cancellation,
        };
        match self.opener.try_open(native_request) {
            Ok(handle) => {
                if !valid_text(&handle, self.limits.maximum_native_handle_bytes) {
                    let pending = self.state.lock().pending.remove(&token);
                    if let Some(pending) = pending {
                        drop(pending.work);
                    }
                    self.opener.cancel(&handle);
                    self.activity.record(LinkActivity {
                        principal: request.principal,
                        session: request.session,
                        outcome: LinkActivityOutcome::Refused,
                    });
                    return terminal_response(
                        &id,
                        LinkTerminal::Rejected {
                            error: "native opener returned an invalid handle",
                        },
                        self.limits.maximum_response_bytes,
                        &request_action,
                    );
                }
                let retained = {
                    let mut state = self.state.lock();
                    state.pending.get_mut(&token).is_some_and(|pending| {
                        pending.native_handle = Some(Arc::clone(&handle));
                        true
                    })
                };
                if !retained {
                    self.opener.cancel(&handle);
                }
                self.activity.record(LinkActivity {
                    principal: request.principal,
                    session: request.session,
                    outcome: LinkActivityOutcome::Started,
                });
                Ok(ProviderCall::completed(None))
            }
            Err(error) => {
                let pending = self.state.lock().pending.remove(&token);
                if let Some(pending) = pending {
                    drop(pending.work);
                }
                self.activity.record(LinkActivity {
                    principal: request.principal.clone(),
                    session: request.session,
                    outcome: LinkActivityOutcome::Refused,
                });
                let error = error.to_string();
                terminal_response(
                    &id,
                    LinkTerminal::Rejected { error: &error },
                    self.limits.maximum_response_bytes,
                    &request_action,
                )
            }
        }
    }

    fn remove_session(&self, context: &ProviderSessionContext) {
        let cancelled = {
            let mut state = self.state.lock();
            if state
                .sessions
                .get(&context.session)
                .is_none_or(|session| session.principal != context.principal)
            {
                return;
            }
            state.sessions.remove(&context.session);
            let tokens = state
                .pending
                .iter()
                .filter_map(|(token, pending)| {
                    (pending.session == context.session).then_some(*token)
                })
                .collect::<Vec<_>>();
            tokens
                .into_iter()
                .filter_map(|token| state.pending.remove(&token))
                .collect::<Vec<_>>()
        };
        for pending in cancelled {
            pending.work.cancellation().cancel();
            if let Some(handle) = pending.native_handle {
                self.opener.cancel(&handle);
            }
            self.activity.record(LinkActivity {
                principal: pending.principal,
                session: pending.session,
                outcome: LinkActivityOutcome::LifecycleCancelled,
            });
        }
    }
}

impl Provider for LinkProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        match request.action.as_ref() {
            "open" => self.open(request),
            _ => Err(invalid(&request, "unknown action")),
        }
    }

    fn session_opened(&self, session: ProviderSession) -> Result<(), ProviderError> {
        let mut state = self.state.lock();
        if let Some(existing) = state.sessions.get(&session.context.session) {
            return if existing.principal == session.context.principal
                && existing.outbound.source_window() == session.context.source_window
            {
                Ok(())
            } else {
                Err(lifecycle_error("mapped link session identity changed"))
            };
        }
        if state.sessions.len() >= self.limits.maximum_sessions {
            return Err(lifecycle_error("link session capacity is full"));
        }
        state.sessions.insert(
            session.context.session,
            LinkSession {
                principal: session.context.principal,
                outbound: session.outbound,
                ready: false,
            },
        );
        Ok(())
    }

    fn session_ready(&self, context: &ProviderSessionContext) -> Result<(), ProviderError> {
        let mut state = self.state.lock();
        let session = state
            .sessions
            .get_mut(&context.session)
            .ok_or_else(|| lifecycle_error("link session was not opened"))?;
        if session.principal != context.principal
            || session.outbound.source_window() != context.source_window
        {
            return Err(lifecycle_error("mapped link session identity changed"));
        }
        session.ready = true;
        Ok(())
    }

    fn session_closed(&self, context: &ProviderSessionContext, _reason: ProviderSessionEnd) {
        self.remove_session(context);
    }

    fn session_revoked(&self, context: &ProviderSessionContext) {
        self.remove_session(context);
    }
}

fn validate_limits(limits: LinkProviderLimits) -> Result<(), LinkProviderBuildError> {
    if [
        limits.maximum_sessions,
        limits.maximum_pending_per_session,
        limits.maximum_pending_total,
        limits.maximum_url_bytes,
        limits.maximum_label_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_native_handle_bytes,
        limits.maximum_response_bytes,
    ]
    .contains(&0)
        || limits.maximum_pending_total < limits.maximum_pending_per_session
    {
        return Err(LinkProviderBuildError::InvalidLimits);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkUrlRefusal {
    InvalidUrl,
    UnsupportedScheme,
    BlockedByPolicy,
}

impl LinkUrlRefusal {
    fn code(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid-url",
            Self::UnsupportedScheme => "unsupported-scheme",
            Self::BlockedByPolicy => "blocked-by-policy",
        }
    }
}

fn validate_external_url(value: &str, maximum_bytes: usize) -> Result<Arc<str>, LinkUrlRefusal> {
    if !valid_text(value, maximum_bytes) {
        return Err(LinkUrlRefusal::InvalidUrl);
    }
    let parsed = Url::parse(value).map_err(|_| LinkUrlRefusal::InvalidUrl)?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err(LinkUrlRefusal::UnsupportedScheme);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(LinkUrlRefusal::BlockedByPolicy);
    }
    let host = parsed.host().ok_or(LinkUrlRefusal::InvalidUrl)?;
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
                || !domain.contains('.')
            {
                return Err(LinkUrlRefusal::BlockedByPolicy);
            }
        }
        Host::Ipv4(address) if !public_ipv4(address) => {
            return Err(LinkUrlRefusal::BlockedByPolicy);
        }
        Host::Ipv6(address) if !public_ipv6(address) => {
            return Err(LinkUrlRefusal::BlockedByPolicy);
        }
        Host::Ipv4(_) | Host::Ipv6(_) => {}
    }
    let normalized = parsed.to_string();
    if normalized.len() > maximum_bytes {
        return Err(LinkUrlRefusal::InvalidUrl);
    }
    Ok(Arc::from(normalized))
}

fn parse_label(
    options: Option<&Value>,
    maximum_label_bytes: usize,
) -> Result<Option<Arc<str>>, &'static str> {
    let Some(options) = options else {
        return Ok(None);
    };
    let object = options
        .as_object()
        .ok_or("`options` must be an object when present")?;
    if object.keys().any(|key| key != "label") {
        return Err("`options` may contain only `label`");
    }
    let Some(label) = object.get("label") else {
        return Ok(None);
    };
    let label = label.as_str().ok_or("`options.label` must be a string")?;
    if label.len() > maximum_label_bytes {
        return Err("`options.label` exceeds the configured byte limit");
    }
    Ok(Some(Arc::from(label)))
}

fn public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, _, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
        || a >= 224)
}

fn public_ipv6(address: Ipv6Addr) -> bool {
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (address.segments()[0] & 0xfe00) == 0xfc00
        || (address.segments()[0] & 0xffc0) == 0xfe80
        || matches!(address.to_ipv4_mapped(), Some(ipv4) if !public_ipv4(ipv4)))
}

fn ensure_ready_link_session(
    state: &LinkState,
    request: &ProviderRequest,
) -> Result<(), ProviderError> {
    match state.sessions.get(&request.session) {
        Some(session) if session.principal == request.principal && session.ready => Ok(()),
        Some(_) => Err(denied(request, "mapped link session is not ready")),
        None => Err(denied(request, "link session is not mapped")),
    }
}

fn correlation_id(
    request: &ProviderRequest,
    limits: LinkProviderLimits,
) -> Result<&str, ProviderError> {
    let id = request
        .correlation_id
        .as_deref()
        .ok_or_else(|| invalid(request, "`id` is required"))?;
    if !valid_text(id, limits.maximum_correlation_id_bytes) {
        return Err(invalid(request, "`id` is invalid or too large"));
    }
    Ok(id)
}

fn valid_text(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && !value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkTerminal<'a> {
    Opened,
    Denied {
        error: &'a str,
    },
    /// The pinned shim rejects this terminal because it deliberately omits a
    /// public status rather than manufacturing `failed` or `cancelled`.
    Rejected {
        error: &'a str,
    },
}

fn terminal_fields(
    correlation_id: &str,
    terminal: LinkTerminal<'_>,
) -> serde_json::Map<String, Value> {
    let mut fields =
        serde_json::Map::from_iter([("id".to_owned(), Value::String(correlation_id.to_owned()))]);
    match terminal {
        LinkTerminal::Opened => {
            fields.insert("status".to_owned(), Value::String("opened".to_owned()));
        }
        LinkTerminal::Denied { error } => {
            fields.insert("status".to_owned(), Value::String("denied".to_owned()));
            fields.insert("error".to_owned(), Value::String(error.to_owned()));
        }
        LinkTerminal::Rejected { error } => {
            fields.insert("error".to_owned(), Value::String(error.to_owned()));
        }
    }
    fields
}

fn terminal_response(
    correlation_id: &str,
    terminal: LinkTerminal<'_>,
    maximum_bytes: usize,
    action: &Arc<str>,
) -> Result<ProviderCall, ProviderError> {
    let mut fields = terminal_fields(correlation_id, terminal);
    fields.insert(
        "type".to_owned(),
        Value::String("link.open.result".to_owned()),
    );
    let value = Value::Object(fields);
    BoundedJson::from_value(&value, maximum_bytes)
        .map(|value| ProviderCall::completed(Some(value)))
        .map_err(|_| ProviderError::Failed {
            domain: Arc::from(LINK_DOMAIN),
            action: Arc::clone(action),
            reason: Arc::from("response exceeds the configured byte limit"),
        })
}

fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(LINK_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn denied(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Denied {
        domain: Arc::from(LINK_DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn lifecycle_error(reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(LINK_DOMAIN),
        action: Arc::from("lifecycle"),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use nmp_native_nap_bridge::{
        ActivitySink, BridgeLimits, DispatchOutcome, ProviderActivity, ProviderPushObserver,
        ProviderRegistry, SessionContext, SourceWindowId,
    };
    use nmp_native_runtime_core::{
        ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceLimits, ResourceTracker,
        Sensitivity,
    };
    use serde_json::json;

    use super::*;

    #[derive(Debug, Default)]
    struct FakeOpener {
        requests: Mutex<Vec<NativeLinkOpenRequest>>,
        cancelled: Mutex<Vec<Arc<str>>>,
        start_error: Mutex<Option<NativeLinkStartError>>,
    }

    impl NativeLinkOpener for FakeOpener {
        fn try_open(
            &self,
            request: NativeLinkOpenRequest,
        ) -> Result<Arc<str>, NativeLinkStartError> {
            let handle: Arc<str> = Arc::from(format!("link-{}", request.token.0));
            self.requests.lock().push(request);
            if let Some(error) = self.start_error.lock().take() {
                return Err(error);
            }
            Ok(handle)
        }

        fn cancel(&self, native_handle: &str) {
            self.cancelled.lock().push(Arc::from(native_handle));
        }
    }

    #[derive(Debug)]
    struct NoBridgeActivity;

    impl ActivitySink for NoBridgeActivity {
        fn record(&self, _fact: ProviderActivity) {}
    }

    #[derive(Debug)]
    struct DenyAllLinks;

    impl LinkPolicy for DenyAllLinks {
        fn evaluate(&self, _request: &LinkPolicyRequest) -> LinkPolicyDecision {
            LinkPolicyDecision::Deny
        }
    }

    struct Rig {
        provider: Arc<LinkProvider>,
        opener: Arc<FakeOpener>,
        registry: ProviderRegistry,
        context: SessionContext,
        plan: nmp_native_nap_bridge::InjectionPlan,
        observer: ProviderPushObserver,
    }

    impl Rig {
        fn new() -> Self {
            Self::with_policy(Arc::new(AllowExternalWebLinks))
        }

        fn with_policy(policy: Arc<dyn LinkPolicy>) -> Self {
            let opener = Arc::new(FakeOpener::default());
            let provider = Arc::new(
                LinkProvider::new(
                    policy,
                    opener.clone(),
                    Arc::new(NoopLinkActivity),
                    LinkProviderLimits::default(),
                )
                .unwrap(),
            );
            let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
            let grants =
                Arc::new(GrantLedger::new(GrantLimits::default(), resources.clone()).unwrap());
            let mut registry = ProviderRegistry::new(
                BridgeLimits::default(),
                resources,
                grants.clone(),
                Arc::new(NoBridgeActivity),
            )
            .unwrap();
            registry.register(provider.clone()).unwrap();
            let context = SessionContext {
                id: SessionId(7),
                principal: principal("caller", 'b'),
                profile: ExecutionProfile::Legacy,
            };
            let capability = Capability::new(LINK_DOMAIN).unwrap();
            grants
                .set(
                    context.principal.clone(),
                    capability.clone(),
                    Sensitivity::Sensitive,
                    GrantDecision::AllowExactBuild,
                )
                .unwrap();
            let plan = registry
                .negotiate(
                    &context.principal,
                    context.profile,
                    &BTreeSet::from([capability]),
                )
                .unwrap();
            let observer = registry
                .open_session_bound(&context, &plan, SourceWindowId(77), 0)
                .unwrap();
            registry.mark_session_ready(context.id).unwrap();
            Self {
                provider,
                opener,
                registry,
                context,
                plan,
                observer,
            }
        }

        fn dispatch(&self, envelope: Value) -> Result<Option<Value>, String> {
            match self
                .registry
                .dispatch(
                    &self.context,
                    &self.plan,
                    &serde_json::to_vec(&envelope).unwrap(),
                    1,
                )
                .map_err(|error| error.to_string())?
            {
                DispatchOutcome::Handled(call) => Ok(call
                    .response
                    .map(|response| response.decode().expect("bounded JSON"))),
                DispatchOutcome::IgnoredUnknown => Err("unexpected unknown action".to_owned()),
            }
        }
    }

    fn principal(d_tag: &str, hash: char) -> Principal {
        Principal::new("a".repeat(64), d_tag, hash.to_string().repeat(64)).unwrap()
    }

    #[test]
    fn external_open_requires_confirmation_and_completes_by_push() {
        let rig = Rig::new();
        assert_eq!(
            rig.dispatch(json!({
                "type":"link.open",
                "id":"open-1",
                "url":"https://example.com/path",
                "options":{"label":"Read post"}
            }))
            .unwrap(),
            None
        );
        let requests = rig.opener.requests.lock();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].confirmation_required);
        assert_eq!(
            requests[0].normalized_url.as_ref(),
            "https://example.com/path"
        );
        assert_eq!(requests[0].label.as_deref(), Some("Read post"));
        let token = requests[0].token;
        drop(requests);

        rig.provider
            .complete(token, NativeLinkOutcome::Opened)
            .unwrap();
        let pushes = rig.observer.drain(8).unwrap().pushes;
        assert_eq!(pushes.len(), 1);
        assert_eq!(
            pushes[0].envelope.decode().unwrap(),
            json!({"type":"link.open.result","id":"open-1","status":"opened"})
        );
        assert_eq!(rig.provider.pending_count(), 0);
    }

    #[test]
    fn unsafe_schemes_credentials_and_private_hosts_are_denied_without_execution() {
        let rig = Rig::new();
        for (index, (url, error)) in [
            ("javascript:alert(1)", "unsupported-scheme"),
            ("file:///etc/passwd", "unsupported-scheme"),
            ("not an absolute url", "invalid-url"),
            ("https://user:pass@example.com/", "blocked-by-policy"),
            ("http://localhost:8080/", "blocked-by-policy"),
            ("http://127.0.0.1/", "blocked-by-policy"),
            ("http://192.168.1.2/", "blocked-by-policy"),
            ("https://printer.local/", "blocked-by-policy"),
            ("https://intranet/", "blocked-by-policy"),
        ]
        .into_iter()
        .enumerate()
        {
            let result = rig
                .dispatch(json!({
                    "type":"link.open",
                    "id":format!("bad-{index}"),
                    "url":url
                }))
                .unwrap()
                .unwrap();
            assert_eq!(
                result,
                json!({
                    "type":"link.open.result",
                    "id":format!("bad-{index}"),
                    "status":"denied",
                    "error":error
                })
            );
        }
        assert!(rig.opener.requests.lock().is_empty());
    }

    #[test]
    fn product_policy_denial_returns_the_pinned_denied_result_without_native_work() {
        let rig = Rig::with_policy(Arc::new(DenyAllLinks));
        assert_eq!(
            rig.dispatch(json!({
                "type":"link.open",
                "id":"policy-denied",
                "url":"https://example.com/"
            }))
            .unwrap()
            .unwrap(),
            json!({
                "type":"link.open.result",
                "id":"policy-denied",
                "status":"denied",
                "error":"blocked-by-policy"
            })
        );
        assert!(rig.opener.requests.lock().is_empty());
    }

    #[test]
    fn label_matches_the_pinned_optional_bounded_display_hint() {
        let rig = Rig::new();
        let maximum = "🙂".repeat(1_024);
        assert_eq!(maximum.len(), 4 * 1_024);
        assert_eq!(
            rig.dispatch(json!({
                "type":"link.open",
                "id":"label-maximum",
                "url":"https://example.com/",
                "options":{"label":maximum}
            }))
            .unwrap(),
            None
        );
        assert_eq!(
            rig.opener.requests.lock()[0].label.as_deref(),
            Some(maximum.as_str())
        );

        let error = rig
            .dispatch(json!({
                "type":"link.open",
                "id":"label-over-maximum",
                "url":"https://example.com/",
                "options":{"label":"🙂".repeat(1_025)}
            }))
            .unwrap_err();
        assert!(
            error.contains("`options.label` exceeds the configured byte limit"),
            "{error}"
        );
        let error = rig
            .dispatch(json!({
                "type":"link.open",
                "id":"unknown-option",
                "url":"https://example.com/",
                "options":{"target":"_blank"}
            }))
            .unwrap_err();
        assert!(
            error.contains("`options` may contain only `label`"),
            "{error}"
        );
        assert_eq!(rig.opener.requests.lock().len(), 1);
    }

    #[test]
    fn native_cancellation_denies_and_native_failure_rejects_without_new_statuses() {
        let rig = Rig::new();
        rig.dispatch(json!({
            "type":"link.open",
            "id":"cancelled",
            "url":"https://example.com/cancelled"
        }))
        .unwrap();
        let cancelled = rig.opener.requests.lock()[0].token;
        rig.provider
            .complete(cancelled, NativeLinkOutcome::Cancelled)
            .unwrap();

        rig.dispatch(json!({
            "type":"link.open",
            "id":"failed",
            "url":"https://example.com/failed"
        }))
        .unwrap();
        let failed = rig.opener.requests.lock()[1].token;
        rig.provider
            .complete(failed, NativeLinkOutcome::Failed)
            .unwrap();

        let pushes = rig.observer.drain(8).unwrap().pushes;
        assert_eq!(pushes.len(), 2);
        assert_eq!(
            pushes[0].envelope.decode().unwrap(),
            json!({
                "type":"link.open.result",
                "id":"cancelled",
                "status":"denied",
                "error":"user-denied"
            })
        );
        assert_eq!(
            pushes[1].envelope.decode().unwrap(),
            json!({
                "type":"link.open.result",
                "id":"failed",
                "error":"native-open-failed"
            })
        );
    }

    #[test]
    fn native_start_failure_rejects_without_an_invented_terminal_status() {
        let rig = Rig::new();
        *rig.opener.start_error.lock() = Some(NativeLinkStartError::Unavailable);
        assert_eq!(
            rig.dispatch(json!({
                "type":"link.open",
                "id":"unavailable",
                "url":"https://example.com/"
            }))
            .unwrap()
            .unwrap(),
            json!({
                "type":"link.open.result",
                "id":"unavailable",
                "error":"native link opener is unavailable"
            })
        );
        assert_eq!(rig.provider.pending_count(), 0);
    }

    #[test]
    fn teardown_cancels_exact_pending_native_operation() {
        let rig = Rig::new();
        rig.dispatch(json!({
            "type":"link.open",
            "id":"open-1",
            "url":"https://example.com/"
        }))
        .unwrap();
        let cancellation = rig.opener.requests.lock()[0].cancellation.clone();
        rig.registry.close_session(rig.context.id);
        assert!(cancellation.is_cancelled());
        assert_eq!(
            rig.opener.cancelled.lock().as_slice(),
            &[Arc::from("link-1")]
        );
        assert_eq!(rig.provider.pending_count(), 0);
    }

    #[test]
    fn normalized_public_ipv6_and_https_are_accepted() {
        assert!(validate_external_url("https://example.com", 1024).is_ok());
        assert!(validate_external_url("http://[2606:4700:4700::1111]/", 1024).is_ok());
        assert!(validate_external_url("http://[::1]/", 1024).is_err());
        assert!(validate_external_url("http://[fe80::1]/", 1024).is_err());
    }
}
