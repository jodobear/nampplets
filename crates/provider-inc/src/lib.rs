//! Exact, bounded `@napplet/nap` 0.29.0 NAP-INC provider.
//!
//! Routing identity is always derived from the trusted mapped session supplied
//! by `nap-bridge`. Component envelopes cannot select a principal, session,
//! source window, or sender. All provider state is finite and is synchronously
//! removed on stop, crash, revocation, open rollback, and runtime close.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushError, ProviderPushSender, ProviderPushTermination, ProviderRequest,
    ProviderSession, ProviderSessionContext, ProviderSessionEnd, SourceWindowId,
};
use nmp_native_runtime_core::{BoundedJson, Capability, Principal, SessionId};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use thiserror::Error;

pub const DOMAIN: &str = "inc";
pub const PINNED_NAP_PROTOCOL: &str = "napplet-web@0.29.0";

const CHANNEL_ID_ATTEMPTS: usize = 8;
const REASON_PEER_DESTROYED: &str = "peer destroyed";
const REASON_ACL_REVOKED: &str = "ACL revoked";
pub const NOTE_OPEN_TOPIC: &str = "note:open";
pub const PROFILE_OPEN_TOPIC: &str = "profile:open";
pub const COMPOSE_OPEN_TOPIC: &str = "compose:open";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncProviderLimits {
    pub maximum_sessions: usize,
    pub maximum_subscriptions_per_session: usize,
    pub maximum_total_subscriptions: usize,
    pub maximum_channels_per_session: usize,
    pub maximum_total_channels: usize,
    pub maximum_topic_bytes: usize,
    pub maximum_payload_bytes: usize,
    pub maximum_response_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
    pub maximum_channel_id_bytes: usize,
    pub maximum_json_depth: usize,
    pub maximum_container_items: usize,
    pub maximum_string_bytes: usize,
}

impl Default for IncProviderLimits {
    fn default() -> Self {
        Self {
            maximum_sessions: 64,
            maximum_subscriptions_per_session: 64,
            maximum_total_subscriptions: 1_024,
            maximum_channels_per_session: 32,
            maximum_total_channels: 512,
            maximum_topic_bytes: 1_024,
            maximum_payload_bytes: 128 * 1024,
            maximum_response_bytes: 256 * 1024,
            maximum_correlation_id_bytes: 1_024,
            maximum_channel_id_bytes: 128,
            maximum_json_depth: 32,
            maximum_container_items: 1_024,
            maximum_string_bytes: 128 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopicAccess {
    Subscribe,
    Emit,
}

#[derive(Clone, Copy, Debug)]
pub struct TopicAclRequest<'a> {
    pub principal: &'a Principal,
    pub session: SessionId,
    pub access: TopicAccess,
    pub topic: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct ChannelAclRequest<'a> {
    pub opener: &'a Principal,
    pub opener_session: SessionId,
    pub peer: &'a Principal,
    pub peer_session: SessionId,
}

/// Product-owned INC policy. Authorization is evaluated for topic operations
/// and once when a channel opens, matching the pinned auth-on-open contract.
/// Implementations must be bounded, nonblocking, and must not call back into
/// this provider.
pub trait IncAcl: Send + Sync + fmt::Debug {
    fn allow_topic(&self, request: TopicAclRequest<'_>) -> bool;
    fn allow_channel(&self, request: ChannelAclRequest<'_>) -> bool;
}

#[derive(Debug, Default)]
pub struct AllowAllIncAcl;

impl IncAcl for AllowAllIncAcl {
    fn allow_topic(&self, _request: TopicAclRequest<'_>) -> bool {
        true
    }

    fn allow_channel(&self, _request: ChannelAclRequest<'_>) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncActivityAction {
    Subscribe,
    Unsubscribe,
    TopicEmit,
    ChannelOpen,
    ChannelList,
    ChannelEmit,
    ChannelBroadcast,
    ChannelClose,
    NativeAction,
    LifecycleCleanup,
    AclCleanup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IncActivityOutcome {
    Completed,
    Refused(IncRefusal),
    Delivered {
        attempted: usize,
        delivered: usize,
        refused: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncRefusal {
    Acl,
    Capacity,
    TargetNotFound,
    AmbiguousTarget,
    UnknownChannel,
    CrossPrincipal,
    PushBackpressure,
    NativeActionBackpressure,
    NativeActionClosed,
    IdGeneration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncActivity {
    pub principal: Principal,
    pub session: SessionId,
    pub action: IncActivityAction,
    pub outcome: IncActivityOutcome,
}

/// Provider-local activity seam. Facts deliberately omit topics and payloads.
/// Implementations must record synchronously without blocking or re-entering
/// this provider.
pub trait IncActivitySink: Send + Sync + fmt::Debug {
    fn record(&self, fact: IncActivity);
}

#[derive(Debug, Default)]
pub struct NoopIncActivity;

impl IncActivitySink for NoopIncActivity {
    fn record(&self, _fact: IncActivity) {}
}

/// Trusted origin attached by the provider to a native action. None of these
/// fields are accepted from an untrusted `inc.emit` envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncNativeActionOrigin {
    pub principal: Principal,
    pub session: SessionId,
    pub source_window: SourceWindowId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncNativeActionKind {
    NoteOpen,
    ProfileOpen,
    ComposeOpen,
}

impl IncNativeActionKind {
    pub fn topic(self) -> &'static str {
        match self {
            Self::NoteOpen => NOTE_OPEN_TOPIC,
            Self::ProfileOpen => PROFILE_OPEN_TOPIC,
            Self::ComposeOpen => COMPOSE_OPEN_TOPIC,
        }
    }
}

/// One validated native action. `payload` preserves the exact accepted legacy
/// payload so the host can project it across FFI without rebuilding or
/// widening the component-controlled envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncNativeAction {
    pub origin: IncNativeActionOrigin,
    pub kind: IncNativeActionKind,
    pub payload: BoundedJson,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum IncNativeActionSinkError {
    #[error("native action capacity is full")]
    Backpressure,
    #[error("native action sink is closed")]
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncNativeActionSessionEnd {
    Closed(ProviderSessionEnd),
    Revoked,
}

/// Bounded nonblocking execution seam for shell-owned native actions.
///
/// `try_enqueue` is invoked synchronously while the originating mapped session
/// is still live. Implementations MUST use finite capacity, MUST return
/// `Backpressure` rather than block or grow, and MUST NOT call back into this
/// provider. `session_ended` must synchronously purge any pending actions for
/// the exact origin and be idempotent.
pub trait IncNativeActionSink: Send + Sync + fmt::Debug {
    fn try_enqueue(&self, action: IncNativeAction) -> Result<(), IncNativeActionSinkError>;

    fn session_ended(&self, origin: &IncNativeActionOrigin, reason: IncNativeActionSessionEnd);
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ChannelIdError {
    #[error("secure channel id generation is unavailable")]
    Unavailable,
}

pub trait ChannelIdGenerator: Send + Sync + fmt::Debug {
    /// Produces one bounded opaque id without blocking or re-entering the
    /// provider. The provider performs a finite collision check.
    fn next_id(&self) -> Result<Arc<str>, ChannelIdError>;
}

#[derive(Debug, Default)]
pub struct SecureChannelIdGenerator;

impl ChannelIdGenerator for SecureChannelIdGenerator {
    fn next_id(&self) -> Result<Arc<str>, ChannelIdError> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| ChannelIdError::Unavailable)?;
        Ok(Arc::from(format!("c-{}", hex::encode(bytes))))
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum IncProviderBuildError {
    #[error("INC limits must be finite, non-zero, and internally consistent")]
    InvalidLimits,
}

#[derive(Clone, Debug, Error)]
pub enum IncNativePushError {
    #[error("target session is not a known INC session")]
    UnknownSession,
    #[error("target session has not subscribed to this topic")]
    NotSubscribed,
    #[error("push delivery was refused: {0}")]
    Push(#[from] ProviderPushError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncCensus {
    pub sessions: usize,
    pub ready_sessions: usize,
    pub subscriptions: usize,
    pub channels: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncAclCleanup {
    pub subscriptions_removed: usize,
    pub channels_closed: usize,
    pub close_notifications_delivered: usize,
    pub close_notifications_refused: usize,
}

#[derive(Debug)]
pub struct IncProvider {
    acl: Arc<dyn IncAcl>,
    activity: Arc<dyn IncActivitySink>,
    channel_ids: Arc<dyn ChannelIdGenerator>,
    native_actions: Option<Arc<dyn IncNativeActionSink>>,
    limits: IncProviderLimits,
    descriptor: ProviderDescriptor,
    state: Mutex<IncState>,
}

#[derive(Debug, Default)]
struct IncState {
    sessions: BTreeMap<SessionId, IncSession>,
    channels: BTreeMap<Arc<str>, IncChannel>,
}

#[derive(Clone, Debug)]
struct IncSession {
    principal: Principal,
    outbound: ProviderPushSender,
    ready: bool,
    subscriptions: BTreeSet<Arc<str>>,
}

#[derive(Clone, Debug)]
struct IncChannel {
    first: SessionId,
    second: SessionId,
}

impl IncChannel {
    fn contains(&self, session: SessionId) -> bool {
        self.first == session || self.second == session
    }

    fn peer(&self, session: SessionId) -> Option<SessionId> {
        if self.first == session {
            Some(self.second)
        } else if self.second == session {
            Some(self.first)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
struct CloseNotice {
    recipient: SessionId,
    channel_id: Arc<str>,
    reason: Option<&'static str>,
}

impl IncProvider {
    pub fn new(
        acl: Arc<dyn IncAcl>,
        activity: Arc<dyn IncActivitySink>,
        limits: IncProviderLimits,
    ) -> Result<Self, IncProviderBuildError> {
        Self::build(
            acl,
            activity,
            Arc::new(SecureChannelIdGenerator),
            None,
            limits,
        )
    }

    pub fn with_native_actions(
        acl: Arc<dyn IncAcl>,
        activity: Arc<dyn IncActivitySink>,
        native_actions: Arc<dyn IncNativeActionSink>,
        limits: IncProviderLimits,
    ) -> Result<Self, IncProviderBuildError> {
        Self::build(
            acl,
            activity,
            Arc::new(SecureChannelIdGenerator),
            Some(native_actions),
            limits,
        )
    }

    pub fn with_channel_ids(
        acl: Arc<dyn IncAcl>,
        activity: Arc<dyn IncActivitySink>,
        channel_ids: Arc<dyn ChannelIdGenerator>,
        limits: IncProviderLimits,
    ) -> Result<Self, IncProviderBuildError> {
        Self::build(acl, activity, channel_ids, None, limits)
    }

    pub fn with_channel_ids_and_native_actions(
        acl: Arc<dyn IncAcl>,
        activity: Arc<dyn IncActivitySink>,
        channel_ids: Arc<dyn ChannelIdGenerator>,
        native_actions: Arc<dyn IncNativeActionSink>,
        limits: IncProviderLimits,
    ) -> Result<Self, IncProviderBuildError> {
        Self::build(acl, activity, channel_ids, Some(native_actions), limits)
    }

    fn build(
        acl: Arc<dyn IncAcl>,
        activity: Arc<dyn IncActivitySink>,
        channel_ids: Arc<dyn ChannelIdGenerator>,
        native_actions: Option<Arc<dyn IncNativeActionSink>>,
        limits: IncProviderLimits,
    ) -> Result<Self, IncProviderBuildError> {
        validate_limits(limits)?;
        Ok(Self {
            acl,
            activity,
            channel_ids,
            native_actions,
            limits,
            descriptor: ProviderDescriptor {
                domain: Capability::new(DOMAIN).expect("static INC capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: [
                    "emit",
                    "subscribe",
                    "unsubscribe",
                    "channel.open",
                    "channel.list",
                    "channel.broadcast",
                    "channel.emit",
                    "channel.close",
                ]
                .into_iter()
                .map(Arc::from)
                .collect(),
                sensitive: true,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            state: Mutex::new(IncState::default()),
        })
    }

    pub fn census(&self) -> IncCensus {
        let state = self.state.lock();
        census(&state)
    }

    /// Delivers a native-originated (not component-emitted) `inc.event` push
    /// to one already-subscribed session. Used by NAP-INTENT dispatch to
    /// hand a launched/focused handler its invocation payload on the same
    /// topic convention it declared in its manifest -- reuses `emit`'s exact
    /// wire envelope so a handler cannot distinguish a native push from a
    /// component-emitted one.
    pub fn native_push(
        &self,
        target: SessionId,
        topic: &str,
        sender: &str,
        payload: &BoundedJson,
    ) -> Result<(), IncNativePushError> {
        let state = self.state.lock();
        let session = state
            .sessions
            .get(&target)
            .ok_or(IncNativePushError::UnknownSession)?;
        if !session.ready || !session.subscriptions.contains(topic) {
            return Err(IncNativePushError::NotSubscribed);
        }
        let payload = payload.decode().unwrap_or(Value::Null);
        let event = Map::from_iter([
            ("topic".to_owned(), Value::String(topic.to_owned())),
            ("sender".to_owned(), Value::String(sender.to_owned())),
            ("payload".to_owned(), payload),
        ]);
        session
            .outbound
            .push("inc.event", event, None)
            .map(|_| ())
            .map_err(IncNativePushError::Push)
    }

    /// Re-evaluates dynamic product ACL state without changing the pinned
    /// channel auth-on-open data path. Revoked topic subscriptions disappear
    /// atomically; revoked channels notify both live peers exactly once.
    pub fn enforce_acl(&self) -> IncAclCleanup {
        let mut state = self.state.lock();
        let mut subscriptions_removed = 0_usize;
        for (session_id, session) in &mut state.sessions {
            let principal = session.principal.clone();
            session.subscriptions.retain(|topic| {
                let allowed = self.acl.allow_topic(TopicAclRequest {
                    principal: &principal,
                    session: *session_id,
                    access: TopicAccess::Subscribe,
                    topic,
                });
                subscriptions_removed += usize::from(!allowed);
                allowed
            });
        }

        let revoked = state
            .channels
            .iter()
            .filter_map(|(id, channel)| {
                let first = state.sessions.get(&channel.first)?;
                let second = state.sessions.get(&channel.second)?;
                (!self.acl.allow_channel(ChannelAclRequest {
                    opener: &first.principal,
                    opener_session: channel.first,
                    peer: &second.principal,
                    peer_session: channel.second,
                }))
                .then_some(Arc::clone(id))
            })
            .collect::<Vec<_>>();
        let mut notices = Vec::with_capacity(revoked.len().saturating_mul(2));
        for channel_id in &revoked {
            if let Some(channel) = state.channels.remove(channel_id) {
                notices.push(CloseNotice {
                    recipient: channel.first,
                    channel_id: Arc::clone(channel_id),
                    reason: Some(REASON_ACL_REVOKED),
                });
                notices.push(CloseNotice {
                    recipient: channel.second,
                    channel_id: Arc::clone(channel_id),
                    reason: Some(REASON_ACL_REVOKED),
                });
            }
        }
        let (delivered, refused) = self.deliver_close_notices(&state, notices);
        for session in state.sessions.values() {
            if subscriptions_removed > 0 || !revoked.is_empty() {
                self.activity.record(IncActivity {
                    principal: session.principal.clone(),
                    session: session.outbound.session(),
                    action: IncActivityAction::AclCleanup,
                    outcome: IncActivityOutcome::Completed,
                });
            }
        }
        IncAclCleanup {
            subscriptions_removed,
            channels_closed: revoked.len(),
            close_notifications_delivered: delivered,
            close_notifications_refused: refused,
        }
    }

    fn subscribe(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        let mut fields = exact_fields(&request, &["topic"], &[])?;
        let topic = take_text(
            &request,
            &mut fields,
            "topic",
            self.limits.maximum_topic_bytes,
        )?;
        validate_topic(&request, &topic)?;

        let mut state = self.state.lock();
        let actor = exact_ready_session(&state, &request)?;
        if !self.acl.allow_topic(TopicAclRequest {
            principal: &actor.principal,
            session: request.session,
            access: TopicAccess::Subscribe,
            topic: &topic,
        }) {
            self.record(
                &request,
                IncActivityAction::Subscribe,
                IncActivityOutcome::Refused(IncRefusal::Acl),
            );
            return self.result_with_error("subscribe", &id, "topic rejected by ACL");
        }

        let total = subscription_count(&state);
        let actor = exact_ready_session_mut(&mut state, &request)?;
        if !actor.subscriptions.contains(topic.as_str())
            && (actor.subscriptions.len() >= self.limits.maximum_subscriptions_per_session
                || total >= self.limits.maximum_total_subscriptions)
        {
            self.record(
                &request,
                IncActivityAction::Subscribe,
                IncActivityOutcome::Refused(IncRefusal::Capacity),
            );
            return self.result_with_error("subscribe", &id, "subscription capacity is full");
        }
        actor.subscriptions.insert(Arc::from(topic));
        self.record(
            &request,
            IncActivityAction::Subscribe,
            IncActivityOutcome::Completed,
        );
        self.response(json!({"type":"inc.subscribe.result","id":id}))
    }

    fn unsubscribe(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        forbid_correlation_id(&request)?;
        let mut fields = exact_fields(&request, &["topic"], &[])?;
        let topic = take_text(
            &request,
            &mut fields,
            "topic",
            self.limits.maximum_topic_bytes,
        )?;
        validate_topic(&request, &topic)?;
        let mut state = self.state.lock();
        exact_ready_session_mut(&mut state, &request)?
            .subscriptions
            .remove(topic.as_str());
        self.record(
            &request,
            IncActivityAction::Unsubscribe,
            IncActivityOutcome::Completed,
        );
        Ok(ProviderCall::completed(None))
    }

    fn emit(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        forbid_correlation_id(&request)?;
        let mut fields = exact_fields(&request, &["topic"], &["payload"])?;
        let topic = take_text(
            &request,
            &mut fields,
            "topic",
            self.limits.maximum_topic_bytes,
        )?;
        validate_topic(&request, &topic)?;
        let payload = take_optional_payload(&request, &mut fields, self.limits)?;

        let state = self.state.lock();
        let actor = exact_ready_session(&state, &request)?;
        if !self.acl.allow_topic(TopicAclRequest {
            principal: &actor.principal,
            session: request.session,
            access: TopicAccess::Emit,
            topic: &topic,
        }) {
            self.record(
                &request,
                IncActivityAction::TopicEmit,
                IncActivityOutcome::Refused(IncRefusal::Acl),
            );
            return Err(denied(&request, "topic rejected by ACL"));
        }
        self.route_native_action(&request, actor, &topic, payload.as_ref())?;
        let sender = actor.principal.d_tag().to_owned();
        let targets = state
            .sessions
            .iter()
            .filter(|(session, endpoint)| {
                **session != request.session
                    && endpoint.ready
                    && endpoint.subscriptions.contains(topic.as_str())
                    && self.acl.allow_topic(TopicAclRequest {
                        principal: &endpoint.principal,
                        session: **session,
                        access: TopicAccess::Subscribe,
                        topic: &topic,
                    })
            })
            .map(|(session, endpoint)| (*session, endpoint.outbound.clone()))
            .collect::<Vec<_>>();
        let mut delivered = 0_usize;
        let mut refused = 0_usize;
        for (recipient, outbound) in &targets {
            let mut event = Map::from_iter([
                ("topic".to_owned(), Value::String(topic.clone())),
                ("sender".to_owned(), Value::String(sender.clone())),
            ]);
            if let Some(payload) = &payload {
                event.insert("payload".to_owned(), payload.clone());
            }
            match outbound.push("inc.event", event, None) {
                Ok(_) => delivered += 1,
                Err(reason) => {
                    refused += 1;
                    self.record_push_refusal(&state, *recipient, reason);
                    outbound.terminate(ProviderPushTermination::Backpressure);
                }
            }
        }
        self.record(
            &request,
            IncActivityAction::TopicEmit,
            IncActivityOutcome::Delivered {
                attempted: targets.len(),
                delivered,
                refused,
            },
        );
        Ok(ProviderCall::completed(None))
    }

    fn route_native_action(
        &self,
        request: &ProviderRequest,
        actor: &IncSession,
        topic: &str,
        payload: Option<&Value>,
    ) -> Result<(), ProviderError> {
        let Some(native_actions) = &self.native_actions else {
            return Ok(());
        };
        let Some((kind, payload)) = validate_native_action(request, topic, payload, self.limits)?
        else {
            return Ok(());
        };
        let action = IncNativeAction {
            origin: IncNativeActionOrigin {
                principal: actor.principal.clone(),
                session: request.session,
                source_window: actor.outbound.source_window(),
            },
            kind,
            payload,
        };
        match native_actions.try_enqueue(action) {
            Ok(()) => {
                self.record(
                    request,
                    IncActivityAction::NativeAction,
                    IncActivityOutcome::Completed,
                );
                Ok(())
            }
            Err(IncNativeActionSinkError::Backpressure) => {
                self.record(
                    request,
                    IncActivityAction::NativeAction,
                    IncActivityOutcome::Refused(IncRefusal::NativeActionBackpressure),
                );
                Err(failed(request, "native action capacity is full"))
            }
            Err(IncNativeActionSinkError::Closed) => {
                self.record(
                    request,
                    IncActivityAction::NativeAction,
                    IncActivityOutcome::Refused(IncRefusal::NativeActionClosed),
                );
                Err(failed(request, "native action sink is closed"))
            }
        }
    }

    fn channel_open(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        let mut fields = exact_fields(&request, &["target"], &[])?;
        let target = take_text(
            &request,
            &mut fields,
            "target",
            self.limits.maximum_string_bytes,
        )?;
        validate_d_tag(&request, &target)?;

        let mut state = self.state.lock();
        let actor = exact_ready_session(&state, &request)?;
        let candidates = state
            .sessions
            .iter()
            .filter(|(session, endpoint)| {
                **session != request.session
                    && endpoint.ready
                    && endpoint.principal.d_tag() == target
            })
            .map(|(session, _)| *session)
            .collect::<Vec<_>>();
        let peer_session = match candidates.as_slice() {
            [] => {
                self.record(
                    &request,
                    IncActivityAction::ChannelOpen,
                    IncActivityOutcome::Refused(IncRefusal::TargetNotFound),
                );
                return self.channel_open_error(&id, "target not found");
            }
            [peer] => *peer,
            _ => {
                self.record(
                    &request,
                    IncActivityAction::ChannelOpen,
                    IncActivityOutcome::Refused(IncRefusal::AmbiguousTarget),
                );
                return self.channel_open_error(&id, "target is ambiguous");
            }
        };
        let peer = state
            .sessions
            .get(&peer_session)
            .expect("candidate was derived from the same locked state");
        if !self.acl.allow_channel(ChannelAclRequest {
            opener: &actor.principal,
            opener_session: request.session,
            peer: &peer.principal,
            peer_session,
        }) {
            self.record(
                &request,
                IncActivityAction::ChannelOpen,
                IncActivityOutcome::Refused(IncRefusal::Acl),
            );
            return self.channel_open_error(&id, "channel rejected by ACL");
        }
        if state.channels.len() >= self.limits.maximum_total_channels
            || channel_count(&state, request.session) >= self.limits.maximum_channels_per_session
            || channel_count(&state, peer_session) >= self.limits.maximum_channels_per_session
        {
            self.record(
                &request,
                IncActivityAction::ChannelOpen,
                IncActivityOutcome::Refused(IncRefusal::Capacity),
            );
            return self.channel_open_error(&id, "channel capacity is full");
        }
        let Some(channel_id) = self.next_unique_channel_id(&state) else {
            self.record(
                &request,
                IncActivityAction::ChannelOpen,
                IncActivityOutcome::Refused(IncRefusal::IdGeneration),
            );
            return self.channel_open_error(&id, "channel id unavailable");
        };
        state.channels.insert(
            Arc::clone(&channel_id),
            IncChannel {
                first: request.session,
                second: peer_session,
            },
        );
        self.record(
            &request,
            IncActivityAction::ChannelOpen,
            IncActivityOutcome::Completed,
        );
        self.response(json!({
            "type":"inc.channel.open.result",
            "id":id,
            "channelId":channel_id,
            "peer":target,
        }))
    }

    fn channel_list(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        exact_fields(&request, &[], &[])?;
        let state = self.state.lock();
        exact_ready_session(&state, &request)?;
        let channels = state
            .channels
            .iter()
            .filter_map(|(channel_id, channel)| {
                let peer_id = channel.peer(request.session)?;
                let peer = state.sessions.get(&peer_id)?;
                Some(json!({"id":channel_id,"peer":peer.principal.d_tag()}))
            })
            .collect::<Vec<_>>();
        self.record(
            &request,
            IncActivityAction::ChannelList,
            IncActivityOutcome::Completed,
        );
        self.response(json!({
            "type":"inc.channel.list.result",
            "id":id,
            "channels":channels,
        }))
    }

    fn channel_emit(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        forbid_correlation_id(&request)?;
        let mut fields = exact_fields(&request, &["channelId"], &["payload"])?;
        let channel_id = take_text(
            &request,
            &mut fields,
            "channelId",
            self.limits.maximum_channel_id_bytes,
        )?;
        validate_channel_id(&request, &channel_id, self.limits)?;
        let payload = take_optional_payload(&request, &mut fields, self.limits)?;

        let mut state = self.state.lock();
        let actor = exact_ready_session(&state, &request)?;
        let sender = actor.principal.d_tag().to_owned();
        let Some(channel) = state.channels.get(channel_id.as_str()).cloned() else {
            self.record(
                &request,
                IncActivityAction::ChannelEmit,
                IncActivityOutcome::Refused(IncRefusal::UnknownChannel),
            );
            return Err(denied(&request, "unknown channel"));
        };
        let Some(peer_id) = channel.peer(request.session) else {
            self.record(
                &request,
                IncActivityAction::ChannelEmit,
                IncActivityOutcome::Refused(IncRefusal::CrossPrincipal),
            );
            return Err(denied(
                &request,
                "channel belongs to another mapped session",
            ));
        };
        let Some(peer) = state.sessions.get(&peer_id) else {
            state.channels.remove(channel_id.as_str());
            self.record(
                &request,
                IncActivityAction::ChannelEmit,
                IncActivityOutcome::Refused(IncRefusal::TargetNotFound),
            );
            return Err(failed(&request, "channel peer is unavailable"));
        };
        let mut event = Map::from_iter([
            ("channelId".to_owned(), Value::String(channel_id.clone())),
            ("sender".to_owned(), Value::String(sender)),
        ]);
        if let Some(payload) = payload {
            event.insert("payload".to_owned(), payload);
        }
        match peer.outbound.push("inc.channel.event", event, None) {
            Ok(_) => {
                self.record(
                    &request,
                    IncActivityAction::ChannelEmit,
                    IncActivityOutcome::Delivered {
                        attempted: 1,
                        delivered: 1,
                        refused: 0,
                    },
                );
            }
            Err(reason) => {
                peer.outbound
                    .terminate(ProviderPushTermination::Backpressure);
                self.record_push_refusal(&state, peer_id, reason);
                state.channels.remove(channel_id.as_str());
                self.push_close_to_actor(
                    &state,
                    request.session,
                    &channel_id,
                    Some(REASON_PEER_DESTROYED),
                );
                self.record(
                    &request,
                    IncActivityAction::ChannelEmit,
                    IncActivityOutcome::Delivered {
                        attempted: 1,
                        delivered: 0,
                        refused: 1,
                    },
                );
            }
        }
        Ok(ProviderCall::completed(None))
    }

    fn channel_broadcast(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        forbid_correlation_id(&request)?;
        let mut fields = exact_fields(&request, &[], &["payload"])?;
        let payload = take_optional_payload(&request, &mut fields, self.limits)?;
        let mut state = self.state.lock();
        let actor = exact_ready_session(&state, &request)?;
        let sender = actor.principal.d_tag().to_owned();
        let routes = state
            .channels
            .iter()
            .filter_map(|(channel_id, channel)| {
                channel
                    .peer(request.session)
                    .map(|peer| (Arc::clone(channel_id), peer))
            })
            .collect::<Vec<_>>();
        let mut delivered = 0_usize;
        let mut refused = 0_usize;
        let mut failed_channels = Vec::new();
        for (channel_id, peer_id) in &routes {
            let Some(peer) = state.sessions.get(peer_id) else {
                refused += 1;
                failed_channels.push(Arc::clone(channel_id));
                continue;
            };
            let mut event = Map::from_iter([
                (
                    "channelId".to_owned(),
                    Value::String(channel_id.to_string()),
                ),
                ("sender".to_owned(), Value::String(sender.clone())),
            ]);
            if let Some(payload) = &payload {
                event.insert("payload".to_owned(), payload.clone());
            }
            match peer.outbound.push("inc.channel.event", event, None) {
                Ok(_) => delivered += 1,
                Err(reason) => {
                    refused += 1;
                    peer.outbound
                        .terminate(ProviderPushTermination::Backpressure);
                    self.record_push_refusal(&state, *peer_id, reason);
                    failed_channels.push(Arc::clone(channel_id));
                }
            }
        }
        for channel_id in failed_channels {
            state.channels.remove(&channel_id);
            self.push_close_to_actor(
                &state,
                request.session,
                &channel_id,
                Some(REASON_PEER_DESTROYED),
            );
        }
        self.record(
            &request,
            IncActivityAction::ChannelBroadcast,
            IncActivityOutcome::Delivered {
                attempted: routes.len(),
                delivered,
                refused,
            },
        );
        Ok(ProviderCall::completed(None))
    }

    fn channel_close(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        forbid_correlation_id(&request)?;
        let mut fields = exact_fields(&request, &["channelId"], &[])?;
        let channel_id = take_text(
            &request,
            &mut fields,
            "channelId",
            self.limits.maximum_channel_id_bytes,
        )?;
        validate_channel_id(&request, &channel_id, self.limits)?;
        let mut state = self.state.lock();
        exact_ready_session(&state, &request)?;
        let Some(channel) = state.channels.get(channel_id.as_str()).cloned() else {
            self.record(
                &request,
                IncActivityAction::ChannelClose,
                IncActivityOutcome::Refused(IncRefusal::UnknownChannel),
            );
            return Err(denied(&request, "unknown channel"));
        };
        if !channel.contains(request.session) {
            self.record(
                &request,
                IncActivityAction::ChannelClose,
                IncActivityOutcome::Refused(IncRefusal::CrossPrincipal),
            );
            return Err(denied(
                &request,
                "channel belongs to another mapped session",
            ));
        }
        state.channels.remove(channel_id.as_str());
        let notices = [
            CloseNotice {
                recipient: channel.first,
                channel_id: Arc::from(channel_id.as_str()),
                reason: None,
            },
            CloseNotice {
                recipient: channel.second,
                channel_id: Arc::from(channel_id.as_str()),
                reason: None,
            },
        ];
        let (delivered, refused) = self.deliver_close_notices(&state, notices);
        self.record(
            &request,
            IncActivityAction::ChannelClose,
            IncActivityOutcome::Delivered {
                attempted: 2,
                delivered,
                refused,
            },
        );
        Ok(ProviderCall::completed(None))
    }

    fn remove_session(
        &self,
        context: &ProviderSessionContext,
        close_reason: &'static str,
        native_reason: IncNativeActionSessionEnd,
    ) {
        let mut state = self.state.lock();
        let Some(session) = state.sessions.get(&context.session) else {
            return;
        };
        if session.principal != context.principal
            || session.outbound.source_window() != context.source_window
        {
            return;
        }
        let principal = session.principal.clone();
        let native_origin = IncNativeActionOrigin {
            principal: principal.clone(),
            session: context.session,
            source_window: context.source_window,
        };
        state.sessions.remove(&context.session);
        let affected = state
            .channels
            .iter()
            .filter_map(|(id, channel)| {
                channel
                    .peer(context.session)
                    .map(|peer| (Arc::clone(id), peer))
            })
            .collect::<Vec<_>>();
        let mut notices = Vec::with_capacity(affected.len());
        for (channel_id, peer) in affected {
            state.channels.remove(&channel_id);
            notices.push(CloseNotice {
                recipient: peer,
                channel_id,
                reason: Some(close_reason),
            });
        }
        let (delivered, refused) = self.deliver_close_notices(&state, notices);
        drop(state);
        if let Some(native_actions) = &self.native_actions {
            native_actions.session_ended(&native_origin, native_reason);
        }
        self.activity.record(IncActivity {
            principal,
            session: context.session,
            action: IncActivityAction::LifecycleCleanup,
            outcome: IncActivityOutcome::Delivered {
                attempted: delivered.saturating_add(refused),
                delivered,
                refused,
            },
        });
    }

    fn deliver_close_notices(
        &self,
        state: &IncState,
        notices: impl IntoIterator<Item = CloseNotice>,
    ) -> (usize, usize) {
        let mut delivered = 0_usize;
        let mut refused = 0_usize;
        for notice in notices {
            let Some(recipient) = state.sessions.get(&notice.recipient) else {
                continue;
            };
            let mut fields = Map::from_iter([(
                "channelId".to_owned(),
                Value::String(notice.channel_id.to_string()),
            )]);
            if let Some(reason) = notice.reason {
                fields.insert("reason".to_owned(), Value::String(reason.to_owned()));
            }
            match recipient.outbound.push("inc.channel.closed", fields, None) {
                Ok(_) => delivered += 1,
                Err(reason) => {
                    refused += 1;
                    recipient
                        .outbound
                        .terminate(ProviderPushTermination::Backpressure);
                    self.record_push_refusal(state, notice.recipient, reason);
                }
            }
        }
        (delivered, refused)
    }

    fn push_close_to_actor(
        &self,
        state: &IncState,
        actor: SessionId,
        channel_id: &str,
        reason: Option<&'static str>,
    ) {
        let Some(endpoint) = state.sessions.get(&actor) else {
            return;
        };
        let mut fields =
            Map::from_iter([("channelId".to_owned(), Value::String(channel_id.to_owned()))]);
        if let Some(reason) = reason {
            fields.insert("reason".to_owned(), Value::String(reason.to_owned()));
        }
        if let Err(error) = endpoint.outbound.push("inc.channel.closed", fields, None) {
            endpoint
                .outbound
                .terminate(ProviderPushTermination::Backpressure);
            self.record_push_refusal(state, actor, error);
        }
    }

    fn next_unique_channel_id(&self, state: &IncState) -> Option<Arc<str>> {
        for _ in 0..CHANNEL_ID_ATTEMPTS {
            let id = self.channel_ids.next_id().ok()?;
            if valid_opaque_id(&id, self.limits.maximum_channel_id_bytes)
                && !state.channels.contains_key(&id)
            {
                return Some(id);
            }
        }
        None
    }

    fn channel_open_error(
        &self,
        correlation_id: &str,
        error: &str,
    ) -> Result<ProviderCall, ProviderError> {
        self.response(json!({
            "type":"inc.channel.open.result",
            "id":correlation_id,
            "error":error,
        }))
    }

    fn result_with_error(
        &self,
        action: &str,
        correlation_id: &str,
        error: &str,
    ) -> Result<ProviderCall, ProviderError> {
        self.response(json!({
            "type":format!("inc.{action}.result"),
            "id":correlation_id,
            "error":error,
        }))
    }

    fn response(&self, value: Value) -> Result<ProviderCall, ProviderError> {
        BoundedJson::from_value(&value, self.limits.maximum_response_bytes)
            .map(|response| ProviderCall::completed(Some(response)))
            .map_err(|_| ProviderError::Failed {
                domain: Arc::from(DOMAIN),
                action: Arc::from("response"),
                reason: Arc::from("response exceeds the configured byte limit"),
            })
    }

    fn record(
        &self,
        request: &ProviderRequest,
        action: IncActivityAction,
        outcome: IncActivityOutcome,
    ) {
        self.activity.record(IncActivity {
            principal: request.principal.clone(),
            session: request.session,
            action,
            outcome,
        });
    }

    fn record_push_refusal(
        &self,
        state: &IncState,
        recipient: SessionId,
        _reason: ProviderPushError,
    ) {
        if let Some(session) = state.sessions.get(&recipient) {
            self.activity.record(IncActivity {
                principal: session.principal.clone(),
                session: recipient,
                action: IncActivityAction::LifecycleCleanup,
                outcome: IncActivityOutcome::Refused(IncRefusal::PushBackpressure),
            });
        }
    }
}

impl Provider for IncProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        if request.work.cancellation().is_cancelled() {
            return Err(failed(&request, "mapped session work was cancelled"));
        }
        match request.action.as_ref() {
            "emit" => self.emit(request),
            "subscribe" => self.subscribe(request),
            "unsubscribe" => self.unsubscribe(request),
            "channel.open" => self.channel_open(request),
            "channel.list" => self.channel_list(request),
            "channel.broadcast" => self.channel_broadcast(request),
            "channel.emit" => self.channel_emit(request),
            "channel.close" => self.channel_close(request),
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
                Err(lifecycle_error("mapped INC session identity changed"))
            };
        }
        if state.sessions.len() >= self.limits.maximum_sessions {
            return Err(lifecycle_error("INC session capacity is full"));
        }
        state.sessions.insert(
            session.context.session,
            IncSession {
                principal: session.context.principal,
                outbound: session.outbound,
                ready: false,
                subscriptions: BTreeSet::new(),
            },
        );
        Ok(())
    }

    fn session_ready(&self, session: &ProviderSessionContext) -> Result<(), ProviderError> {
        let mut state = self.state.lock();
        let Some(existing) = state.sessions.get_mut(&session.session) else {
            return Err(lifecycle_error("INC session was not opened"));
        };
        if existing.principal != session.principal
            || existing.outbound.source_window() != session.source_window
        {
            return Err(lifecycle_error("mapped INC session identity changed"));
        }
        existing.ready = true;
        Ok(())
    }

    fn session_closed(&self, session: &ProviderSessionContext, reason: ProviderSessionEnd) {
        self.remove_session(
            session,
            REASON_PEER_DESTROYED,
            IncNativeActionSessionEnd::Closed(reason),
        );
    }

    fn session_revoked(&self, session: &ProviderSessionContext) {
        self.remove_session(
            session,
            REASON_ACL_REVOKED,
            IncNativeActionSessionEnd::Revoked,
        );
    }
}

fn validate_native_action(
    request: &ProviderRequest,
    topic: &str,
    payload: Option<&Value>,
    limits: IncProviderLimits,
) -> Result<Option<(IncNativeActionKind, BoundedJson)>, ProviderError> {
    let kind = match topic {
        NOTE_OPEN_TOPIC => IncNativeActionKind::NoteOpen,
        PROFILE_OPEN_TOPIC => IncNativeActionKind::ProfileOpen,
        COMPOSE_OPEN_TOPIC => IncNativeActionKind::ComposeOpen,
        _ => return Ok(None),
    };
    let Some(payload) = payload else {
        return Err(invalid(
            request,
            format!("`payload` is required for native action topic `{topic}`"),
        ));
    };
    match kind {
        IncNativeActionKind::NoteOpen => validate_note_open(request, payload, limits)?,
        IncNativeActionKind::ProfileOpen => validate_profile_open(request, payload)?,
        IncNativeActionKind::ComposeOpen => validate_compose_open(request, payload, limits)?,
    }
    let payload = BoundedJson::from_value(payload, limits.maximum_payload_bytes)
        .map_err(|_| invalid(request, "native action payload exceeds its byte limit"))?;
    Ok(Some((kind, payload)))
}

fn validate_profile_open(request: &ProviderRequest, payload: &Value) -> Result<(), ProviderError> {
    let fields = exact_nested_fields(request, payload, "`payload`", &["pubkey"], &[])?;
    require_hex_64(request, fields.get("pubkey"), "`payload.pubkey`")
}

fn validate_note_open(
    request: &ProviderRequest,
    payload: &Value,
    limits: IncProviderLimits,
) -> Result<(), ProviderError> {
    let fields = exact_nested_fields(
        request,
        payload,
        "`payload`",
        &["target"],
        &["relays", "source", "behavior"],
    )?;
    validate_note_target(
        request,
        fields
            .get("target")
            .expect("required nested field was checked"),
        limits,
    )?;
    if let Some(relays) = fields.get("relays") {
        let Some(relays) = relays.as_array() else {
            return Err(invalid(request, "`payload.relays` must be an array"));
        };
        for relay in relays {
            require_bounded_text(
                request,
                Some(relay),
                "`payload.relays[]`",
                limits.maximum_string_bytes,
                false,
            )?;
        }
    }
    if let Some(source) = fields.get("source") {
        validate_action_source(request, source, limits)?;
    }
    if let Some(behavior) = fields.get("behavior") {
        let behavior =
            exact_nested_fields(request, behavior, "`payload.behavior`", &["focus"], &[])?;
        if !matches!(behavior.get("focus"), Some(Value::Bool(_))) {
            return Err(invalid(
                request,
                "`payload.behavior.focus` must be a boolean",
            ));
        }
    }
    Ok(())
}

fn validate_note_target(
    request: &ProviderRequest,
    target: &Value,
    limits: IncProviderLimits,
) -> Result<(), ProviderError> {
    let Some(target_fields) = target.as_object() else {
        return Err(invalid(request, "`payload.target` must be an object"));
    };
    let Some(Value::String(target_type)) = target_fields.get("type") else {
        return Err(invalid(request, "`payload.target.type` must be a string"));
    };
    match target_type.as_str() {
        "event" => {
            let fields = exact_nested_fields(
                request,
                target,
                "`payload.target`",
                &["type", "id"],
                &["kind", "pubkey", "nip19"],
            )?;
            require_hex_64(request, fields.get("id"), "`payload.target.id`")?;
            if let Some(kind) = fields.get("kind") {
                require_u32(request, kind, "`payload.target.kind`")?;
            }
            if fields.contains_key("pubkey") {
                require_hex_64(request, fields.get("pubkey"), "`payload.target.pubkey`")?;
            }
            if fields.contains_key("nip19") {
                require_bounded_text(
                    request,
                    fields.get("nip19"),
                    "`payload.target.nip19`",
                    limits.maximum_string_bytes,
                    false,
                )?;
            }
        }
        "address" => {
            let fields = exact_nested_fields(
                request,
                target,
                "`payload.target`",
                &["type", "kind", "pubkey", "identifier"],
                &["nip19"],
            )?;
            require_u32(
                request,
                fields
                    .get("kind")
                    .expect("required nested field was checked"),
                "`payload.target.kind`",
            )?;
            require_hex_64(request, fields.get("pubkey"), "`payload.target.pubkey`")?;
            require_bounded_text(
                request,
                fields.get("identifier"),
                "`payload.target.identifier`",
                limits.maximum_string_bytes,
                true,
            )?;
            if fields.contains_key("nip19") {
                require_bounded_text(
                    request,
                    fields.get("nip19"),
                    "`payload.target.nip19`",
                    limits.maximum_string_bytes,
                    false,
                )?;
            }
        }
        _ => {
            return Err(invalid(
                request,
                "`payload.target.type` must be `event` or `address`",
            ));
        }
    }
    Ok(())
}

fn validate_compose_open(
    request: &ProviderRequest,
    payload: &Value,
    limits: IncProviderLimits,
) -> Result<(), ProviderError> {
    let fields = exact_nested_fields(
        request,
        payload,
        "`payload`",
        &["source", "intent", "replyTo"],
        &[],
    )?;
    validate_action_source(
        request,
        fields
            .get("source")
            .expect("required nested field was checked"),
        limits,
    )?;
    require_bounded_text(
        request,
        fields.get("intent"),
        "`payload.intent`",
        limits.maximum_string_bytes,
        false,
    )?;
    let reply = exact_nested_fields(
        request,
        fields
            .get("replyTo")
            .expect("required nested field was checked"),
        "`payload.replyTo`",
        &["id", "pubkey", "kind"],
        &["content", "created_at"],
    )?;
    require_hex_64(request, reply.get("id"), "`payload.replyTo.id`")?;
    require_hex_64(request, reply.get("pubkey"), "`payload.replyTo.pubkey`")?;
    require_u32(
        request,
        reply
            .get("kind")
            .expect("required nested field was checked"),
        "`payload.replyTo.kind`",
    )?;
    if reply.contains_key("content") {
        require_bounded_text(
            request,
            reply.get("content"),
            "`payload.replyTo.content`",
            limits.maximum_string_bytes,
            true,
        )?;
    }
    if let Some(created_at) = reply.get("created_at")
        && created_at.as_u64().is_none()
    {
        return Err(invalid(
            request,
            "`payload.replyTo.created_at` must be a non-negative integer",
        ));
    }
    Ok(())
}

fn validate_action_source(
    request: &ProviderRequest,
    source: &Value,
    limits: IncProviderLimits,
) -> Result<(), ProviderError> {
    let source = exact_nested_fields(request, source, "`payload.source`", &["napplet"], &[])?;
    require_bounded_text(
        request,
        source.get("napplet"),
        "`payload.source.napplet`",
        limits.maximum_string_bytes,
        false,
    )
}

fn exact_nested_fields<'a>(
    request: &ProviderRequest,
    value: &'a Value,
    path: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Map<String, Value>, ProviderError> {
    let Some(fields) = value.as_object() else {
        return Err(invalid(request, format!("{path} must be an object")));
    };
    for field in required {
        if !fields.contains_key(*field) {
            return Err(invalid(request, format!("{path}.{field} is required")));
        }
    }
    if let Some(field) = fields
        .keys()
        .find(|field| !required.contains(&field.as_str()) && !optional.contains(&field.as_str()))
    {
        return Err(invalid(request, format!("unknown field `{path}.{field}`")));
    }
    Ok(fields)
}

fn require_hex_64(
    request: &ProviderRequest,
    value: Option<&Value>,
    path: &str,
) -> Result<(), ProviderError> {
    let Some(Value::String(value)) = value else {
        return Err(invalid(request, format!("{path} must be a string")));
    };
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            request,
            format!("{path} must be 64 lowercase hexadecimal characters"),
        ));
    }
    Ok(())
}

fn require_u32(request: &ProviderRequest, value: &Value, path: &str) -> Result<(), ProviderError> {
    if value.as_u64().is_some_and(|value| value <= u32::MAX as u64) {
        Ok(())
    } else {
        Err(invalid(
            request,
            format!("{path} must be a non-negative 32-bit integer"),
        ))
    }
}

fn require_bounded_text(
    request: &ProviderRequest,
    value: Option<&Value>,
    path: &str,
    maximum: usize,
    allow_empty: bool,
) -> Result<(), ProviderError> {
    let Some(Value::String(value)) = value else {
        return Err(invalid(request, format!("{path} must be a string")));
    };
    if (!allow_empty && value.is_empty()) || value.len() > maximum {
        return Err(invalid(
            request,
            format!("{path} is empty or exceeds its byte limit"),
        ));
    }
    Ok(())
}

fn validate_limits(limits: IncProviderLimits) -> Result<(), IncProviderBuildError> {
    let nonzero = [
        limits.maximum_sessions,
        limits.maximum_subscriptions_per_session,
        limits.maximum_total_subscriptions,
        limits.maximum_channels_per_session,
        limits.maximum_total_channels,
        limits.maximum_topic_bytes,
        limits.maximum_payload_bytes,
        limits.maximum_response_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_channel_id_bytes,
        limits.maximum_json_depth,
        limits.maximum_container_items,
        limits.maximum_string_bytes,
    ]
    .into_iter()
    .all(|value| value > 0);
    if !nonzero
        || limits.maximum_subscriptions_per_session > limits.maximum_total_subscriptions
        || limits.maximum_channels_per_session > limits.maximum_total_channels
        || limits.maximum_payload_bytes > limits.maximum_response_bytes
        || limits.maximum_topic_bytes > limits.maximum_string_bytes
    {
        return Err(IncProviderBuildError::InvalidLimits);
    }
    Ok(())
}

fn census(state: &IncState) -> IncCensus {
    IncCensus {
        sessions: state.sessions.len(),
        ready_sessions: state
            .sessions
            .values()
            .filter(|session| session.ready)
            .count(),
        subscriptions: subscription_count(state),
        channels: state.channels.len(),
    }
}

fn subscription_count(state: &IncState) -> usize {
    state
        .sessions
        .values()
        .map(|session| session.subscriptions.len())
        .sum()
}

fn channel_count(state: &IncState, session: SessionId) -> usize {
    state
        .channels
        .values()
        .filter(|channel| channel.contains(session))
        .count()
}

fn exact_ready_session<'a>(
    state: &'a IncState,
    request: &ProviderRequest,
) -> Result<&'a IncSession, ProviderError> {
    let Some(session) = state.sessions.get(&request.session) else {
        return Err(denied(request, "mapped session is not an INC endpoint"));
    };
    if session.principal != request.principal {
        return Err(denied(
            request,
            "mapped session belongs to another exact-build principal",
        ));
    }
    if !session.ready {
        return Err(denied(request, "mapped session is not ready"));
    }
    Ok(session)
}

fn exact_ready_session_mut<'a>(
    state: &'a mut IncState,
    request: &ProviderRequest,
) -> Result<&'a mut IncSession, ProviderError> {
    let Some(session) = state.sessions.get_mut(&request.session) else {
        return Err(denied(request, "mapped session is not an INC endpoint"));
    };
    if session.principal != request.principal {
        return Err(denied(
            request,
            "mapped session belongs to another exact-build principal",
        ));
    }
    if !session.ready {
        return Err(denied(request, "mapped session is not ready"));
    }
    Ok(session)
}

fn exact_fields(
    request: &ProviderRequest,
    required: &[&str],
    optional: &[&str],
) -> Result<Map<String, Value>, ProviderError> {
    let Some(fields) = request.payload.as_object() else {
        return Err(invalid(request, "payload fields must be an object"));
    };
    for field in required {
        if !fields.contains_key(*field) {
            return Err(invalid(request, format!("`{field}` is required")));
        }
    }
    if let Some(field) = fields
        .keys()
        .find(|field| !required.contains(&field.as_str()) && !optional.contains(&field.as_str()))
        .cloned()
    {
        return Err(invalid(
            request,
            format!("unknown or authority-bearing field `{field}`"),
        ));
    }
    Ok(fields.clone())
}

fn take_text(
    request: &ProviderRequest,
    fields: &mut Map<String, Value>,
    name: &str,
    maximum: usize,
) -> Result<String, ProviderError> {
    let Some(Value::String(value)) = fields.remove(name) else {
        return Err(invalid(request, format!("`{name}` must be a string")));
    };
    if value.is_empty()
        || value.len() > maximum
        || value.chars().any(|character| character.is_control())
    {
        return Err(invalid(
            request,
            format!("`{name}` is empty, contains controls, or exceeds its byte limit"),
        ));
    }
    Ok(value)
}

fn take_optional_payload(
    request: &ProviderRequest,
    fields: &mut Map<String, Value>,
    limits: IncProviderLimits,
) -> Result<Option<Value>, ProviderError> {
    let Some(payload) = fields.remove("payload") else {
        return Ok(None);
    };
    validate_json_value(&payload, 0, limits).map_err(|reason| invalid(request, reason))?;
    BoundedJson::from_value(&payload, limits.maximum_payload_bytes)
        .map_err(|_| invalid(request, "`payload` exceeds its byte limit"))?;
    Ok(Some(payload))
}

fn validate_json_value(
    value: &Value,
    depth: usize,
    limits: IncProviderLimits,
) -> Result<(), &'static str> {
    if depth > limits.maximum_json_depth {
        return Err("`payload` exceeds its nesting-depth limit");
    }
    match value {
        Value::String(value) if value.len() > limits.maximum_string_bytes => {
            Err("`payload` contains a string above its byte limit")
        }
        Value::Array(values) => {
            if values.len() > limits.maximum_container_items {
                return Err("`payload` array exceeds its item limit");
            }
            for value in values {
                validate_json_value(value, depth.saturating_add(1), limits)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            if values.len() > limits.maximum_container_items {
                return Err("`payload` object exceeds its item limit");
            }
            for (key, value) in values {
                if key.len() > limits.maximum_string_bytes {
                    return Err("`payload` contains an object key above its byte limit");
                }
                validate_json_value(value, depth.saturating_add(1), limits)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn correlation_id(
    request: &ProviderRequest,
    limits: IncProviderLimits,
) -> Result<String, ProviderError> {
    let Some(id) = request.correlation_id.as_deref() else {
        return Err(invalid(request, "`id` is required"));
    };
    if !valid_opaque_id(id, limits.maximum_correlation_id_bytes) {
        return Err(invalid(request, "`id` is invalid or too large"));
    }
    Ok(id.to_owned())
}

fn forbid_correlation_id(request: &ProviderRequest) -> Result<(), ProviderError> {
    if request.correlation_id.is_some() {
        return Err(invalid(
            request,
            "`id` is forbidden for this fire-and-forget operation",
        ));
    }
    Ok(())
}

fn validate_topic(request: &ProviderRequest, topic: &str) -> Result<(), ProviderError> {
    if topic.trim() != topic {
        return Err(invalid(
            request,
            "`topic` cannot have leading or trailing whitespace",
        ));
    }
    Ok(())
}

fn validate_d_tag(request: &ProviderRequest, d_tag: &str) -> Result<(), ProviderError> {
    if d_tag.trim() != d_tag {
        return Err(invalid(
            request,
            "`target` cannot have leading or trailing whitespace",
        ));
    }
    Ok(())
}

fn validate_channel_id(
    request: &ProviderRequest,
    channel_id: &str,
    limits: IncProviderLimits,
) -> Result<(), ProviderError> {
    if !valid_opaque_id(channel_id, limits.maximum_channel_id_bytes) {
        return Err(invalid(
            request,
            "`channelId` is invalid or exceeds its byte limit",
        ));
    }
    Ok(())
}

fn valid_opaque_id(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

fn invalid(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn denied(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Denied {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn failed(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn lifecycle_error(reason: &'static str) -> ProviderError {
    ProviderError::Failed {
        domain: Arc::from(DOMAIN),
        action: Arc::from("lifecycle"),
        reason: Arc::from(reason),
    }
}

#[cfg(test)]
mod tests;
