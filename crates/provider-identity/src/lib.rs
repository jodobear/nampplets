//! Exact `@napplet/nap` 0.28.0 identity-provider contract.
//!
//! The provider is read-only. It never receives signer capabilities or secret
//! key material. The data-plane port freezes the active public account before
//! each query and must scope every NMP read to that exact account.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Weak},
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushError, ProviderPushSender, ProviderRequest, ProviderSession,
    ProviderSessionContext, ProviderSessionEnd,
};
use nmp_native_runtime_core::{BoundedJson, Capability, Principal, SessionId};
pub use nmp_native_runtime_core::{
    PublicIdentity as FrozenIdentity, PublicIdentityChangeSink as IdentityChangeListener,
    PublicIdentityDataPlane as IdentityDataPlane, PublicIdentityError as IdentityDataError,
    PublicIdentityObservation as AccountObservationHandle, PublicIdentityQuery as IdentityQuery,
    PublicIdentityRead as IdentityRead, PublicIdentityReadLimits as IdentityReadLimits,
    PublicIdentitySubscription as AccountObservation,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

pub const DOMAIN: &str = "identity";
pub const PINNED_NAP_PROTOCOL: &str = "napplet-web@0.28.0";
pub const PINNED_NPM_TARBALL_SHA256: &str =
    "ff51a33cd35e06b5067b09407fb3e381c6bfe4ef229ce8c082b3beb156ebd5b6";

const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityProviderLimits {
    pub maximum_sessions: usize,
    pub maximum_response_bytes: usize,
    pub maximum_evidence_bytes: usize,
    pub maximum_correlation_id_bytes: usize,
    pub maximum_list_type_bytes: usize,
    pub maximum_items: usize,
    pub maximum_relays: usize,
    pub maximum_text_bytes: usize,
    pub maximum_thumbnails_per_badge: usize,
}

impl Default for IdentityProviderLimits {
    fn default() -> Self {
        Self {
            maximum_sessions: 64,
            maximum_response_bytes: 512 * 1024,
            maximum_evidence_bytes: 128 * 1024,
            maximum_correlation_id_bytes: 1_024,
            maximum_list_type_bytes: 128,
            maximum_items: 1_024,
            maximum_relays: 256,
            maximum_text_bytes: 16 * 1024,
            maximum_thumbnails_per_badge: 32,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nip05: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lud16: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayPermission {
    pub read: bool,
    pub write: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ZapReceipt {
    pub event_id: String,
    pub sender: String,
    pub amount: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Badge {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbs: Option<Vec<String>>,
    pub awarded_by: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityValue {
    Relays(BTreeMap<String, RelayPermission>),
    Profile(Option<ProfileData>),
    Follows(Vec<String>),
    List(Vec<String>),
    Zaps(Vec<ZapReceipt>),
    Mutes(Vec<String>),
    Blocked(Vec<String>),
    Badges(Vec<Badge>),
}

pub trait IdentityDiagnosticsSink: Send + Sync + fmt::Debug {
    fn record(&self, fact: IdentityDiagnostic);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityDiagnostic {
    Read {
        principal: Principal,
        session: SessionId,
        action: Arc<str>,
        frozen_pubkey: Option<Arc<str>>,
        scoped_evidence: BoundedJson,
    },
    PushRefused {
        principal: Principal,
        session: SessionId,
        reason: ProviderPushError,
    },
    ObservationClosed,
}

#[derive(Debug, Default)]
pub struct NoopIdentityDiagnostics;

impl IdentityDiagnosticsSink for NoopIdentityDiagnostics {
    fn record(&self, _fact: IdentityDiagnostic) {}
}

#[derive(Debug)]
pub struct IdentityProvider {
    source: Arc<dyn IdentityDataPlane>,
    diagnostics: Arc<dyn IdentityDiagnosticsSink>,
    limits: IdentityProviderLimits,
    descriptor: ProviderDescriptor,
    state: Mutex<IdentityState>,
    observation: Mutex<Option<Arc<dyn AccountObservationHandle>>>,
}

#[derive(Debug)]
struct IdentityState {
    current: FrozenIdentity,
    sessions: BTreeMap<SessionId, IdentitySession>,
    closed: bool,
}

#[derive(Clone, Debug)]
struct IdentitySession {
    principal: Principal,
    outbound: ProviderPushSender,
    ready: bool,
}

impl IdentityProvider {
    /// Creates a registerable provider only after the mandatory account-change
    /// observation has been installed without a snapshot/listener race.
    pub fn connect(
        source: Arc<dyn IdentityDataPlane>,
        diagnostics: Arc<dyn IdentityDiagnosticsSink>,
        limits: IdentityProviderLimits,
    ) -> Result<Arc<Self>, IdentityProviderBuildError> {
        validate_limits(limits)?;
        let provider = Arc::new(Self {
            source: Arc::clone(&source),
            diagnostics: Arc::clone(&diagnostics),
            limits,
            descriptor: ProviderDescriptor {
                domain: Capability::new(DOMAIN).expect("static identity capability is valid"),
                protocol_versions: BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)]),
                actions: [
                    "getPublicKey",
                    "getRelays",
                    "getProfile",
                    "getFollows",
                    "getList",
                    "getZaps",
                    "getMutes",
                    "getBlocked",
                    "getBadges",
                ]
                .into_iter()
                .map(Arc::from)
                .collect(),
                sensitive: true,
                dependencies: BTreeSet::new(),
                platform_availability: ProviderPlatformAvailability::Available,
            },
            state: Mutex::new(IdentityState {
                current: FrozenIdentity {
                    generation: 0,
                    account: None,
                },
                sessions: BTreeMap::new(),
                closed: false,
            }),
            observation: Mutex::new(None),
        });
        let listener: Arc<dyn IdentityChangeListener> = Arc::new(ChangeListener {
            provider: Arc::downgrade(&provider),
        });
        let observation = source
            .observe_public_identity(listener)
            .map_err(IdentityProviderBuildError::Observation)?;
        validate_identity(&observation.current).map_err(|_| {
            IdentityProviderBuildError::Observation(IdentityDataError::InvalidSourceData)
        })?;
        provider.state.lock().current = observation.current;
        *provider.observation.lock() = Some(observation.observation);
        Ok(provider)
    }

    pub fn close(&self) {
        let observation = {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.sessions.clear();
            self.observation.lock().take()
        };
        if let Some(observation) = observation {
            observation.close();
        }
    }

    pub fn active_sessions(&self) -> usize {
        self.state.lock().sessions.len()
    }

    fn on_identity_changed(&self, identity: FrozenIdentity) {
        if validate_identity(&identity).is_err() {
            return;
        }
        let sessions = {
            let mut state = self.state.lock();
            if state.closed || identity.generation <= state.current.generation {
                return;
            }
            let changed = identity.account != state.current.account;
            state.current = identity.clone();
            if !changed {
                return;
            }
            state
                .sessions
                .iter()
                .filter(|(_, session)| session.ready)
                .map(|(session, lane)| (*session, lane.clone()))
                .collect::<Vec<_>>()
        };
        for (session, lane) in sessions {
            if let Err(reason) = self.push_identity(&lane.outbound, &identity) {
                self.state.lock().sessions.remove(&session);
                self.diagnostics.record(IdentityDiagnostic::PushRefused {
                    principal: lane.principal,
                    session,
                    reason,
                });
            }
        }
    }

    fn push_identity(
        &self,
        outbound: &ProviderPushSender,
        identity: &FrozenIdentity,
    ) -> Result<(), ProviderPushError> {
        let message = BoundedJson::from_value(
            &json!({
                "type": "identity.changed",
                "pubkey": wire_pubkey(identity),
            }),
            self.limits.maximum_response_bytes,
        )
        .map_err(|error| ProviderPushError::Malformed(Arc::from(error.to_string())))?;
        outbound
            .push_envelope(&message, Some("identity.current"))
            .map(|_| ())
    }

    fn public_key(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        exact_payload(&request, &[])?;
        // The pinned operation cannot fail. A closed/unavailable identity
        // source truthfully projects as no connected signer.
        let pubkey = self
            .source
            .freeze_public_identity()
            .ok()
            .filter(|identity| validate_identity(identity).is_ok())
            .and_then(|identity| identity.account)
            .map(|account| account.0)
            .unwrap_or_else(|| Arc::from(""));
        response(
            json!({
                "type": "identity.getPublicKey.result",
                "id": id,
                "pubkey": pubkey,
            }),
            self.limits,
            &request,
        )
    }

    fn query(
        &self,
        request: ProviderRequest,
        query: IdentityQuery,
    ) -> Result<ProviderCall, ProviderError> {
        let id = correlation_id(&request, self.limits)?;
        let action = Arc::clone(&request.action);
        let frozen = match self.source.freeze_public_identity() {
            Ok(identity) if validate_identity(&identity).is_ok() => identity,
            Ok(_) => {
                return error_response(
                    action.as_ref(),
                    id,
                    IdentityDataError::InvalidSourceData,
                    self.limits,
                    &request,
                );
            }
            Err(error) => {
                return error_response(action.as_ref(), id, error, self.limits, &request);
            }
        };
        if request.work.cancellation().is_cancelled() {
            return error_response(
                action.as_ref(),
                id,
                IdentityDataError::Cancelled,
                self.limits,
                &request,
            );
        }
        let read = match self.source.read_public_identity(
            &frozen,
            query.clone(),
            request.work.cancellation(),
            IdentityReadLimits {
                maximum_items: self.limits.maximum_items,
                maximum_sources: self.limits.maximum_relays,
                maximum_frame_bytes: self.limits.maximum_evidence_bytes,
            },
        ) {
            Ok(read) => read,
            Err(error) => {
                return error_response(action.as_ref(), id, error, self.limits, &request);
            }
        };
        let value = match decode_identity_value(&query, &read.value) {
            Ok(value) => value,
            Err(()) => {
                return error_response(
                    action.as_ref(),
                    id,
                    IdentityDataError::InvalidSourceData,
                    self.limits,
                    &request,
                );
            }
        };
        if read.frozen_identity != frozen
            || validate_evidence(&read.scoped_evidence, self.limits).is_err()
            || validate_value(&query, &value, self.limits).is_err()
        {
            return error_response(
                action.as_ref(),
                id,
                IdentityDataError::InvalidSourceData,
                self.limits,
                &request,
            );
        }
        self.diagnostics.record(IdentityDiagnostic::Read {
            principal: request.principal.clone(),
            session: request.session,
            action,
            frozen_pubkey: frozen.account.map(|account| account.0),
            scoped_evidence: read.scoped_evidence,
        });
        success_response(id, value, self.limits, &request)
    }
}

impl Provider for IdentityProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn call(&self, request: ProviderRequest) -> Result<ProviderCall, ProviderError> {
        match request.action.as_ref() {
            "getPublicKey" => self.public_key(request),
            "getRelays" => {
                exact_payload(&request, &[])?;
                self.query(request, IdentityQuery::Relays)
            }
            "getProfile" => {
                exact_payload(&request, &[])?;
                self.query(request, IdentityQuery::Profile)
            }
            "getFollows" => {
                exact_payload(&request, &[])?;
                self.query(request, IdentityQuery::Follows)
            }
            "getList" => {
                let payload = exact_payload(&request, &["listType"])?;
                let list_type = required_string(payload, "listType", &request)?;
                if list_type.is_empty() || list_type.len() > self.limits.maximum_list_type_bytes {
                    return Err(invalid_payload(
                        &request,
                        format!(
                            "listType must be 1..={} bytes",
                            self.limits.maximum_list_type_bytes
                        ),
                    ));
                }
                let query = IdentityQuery::List {
                    list_type: Arc::from(list_type),
                };
                self.query(request, query)
            }
            "getZaps" => {
                exact_payload(&request, &[])?;
                self.query(request, IdentityQuery::Zaps)
            }
            "getMutes" => {
                exact_payload(&request, &[])?;
                self.query(request, IdentityQuery::Mutes)
            }
            "getBlocked" => {
                exact_payload(&request, &[])?;
                self.query(request, IdentityQuery::Blocked)
            }
            "getBadges" => {
                exact_payload(&request, &[])?;
                self.query(request, IdentityQuery::Badges)
            }
            _ => Err(invalid_payload(&request, "unknown action")),
        }
    }

    fn session_opened(&self, session: ProviderSession) -> Result<(), ProviderError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(ProviderError::Failed {
                domain: Arc::from(DOMAIN),
                action: Arc::from("session.open"),
                reason: Arc::from("identity provider is closed"),
            });
        }
        if session.outbound.domain().as_str() != DOMAIN
            || session.outbound.session() != session.context.session
        {
            return Err(ProviderError::Denied {
                domain: Arc::from(DOMAIN),
                action: Arc::from("session.open"),
                reason: Arc::from("outbound identity lane does not match the mapped session"),
            });
        }
        if let Some(existing) = state.sessions.get(&session.context.session) {
            return if existing.principal == session.context.principal
                && existing.outbound.source_window() == session.context.source_window
            {
                Ok(())
            } else {
                Err(ProviderError::Denied {
                    domain: Arc::from(DOMAIN),
                    action: Arc::from("session.open"),
                    reason: Arc::from("session id is already bound to another identity lane"),
                })
            };
        }
        if state.sessions.len() >= self.limits.maximum_sessions {
            return Err(ProviderError::Denied {
                domain: Arc::from(DOMAIN),
                action: Arc::from("session.open"),
                reason: Arc::from(format!(
                    "identity session capacity {} is full",
                    self.limits.maximum_sessions
                )),
            });
        }
        state.sessions.insert(
            session.context.session,
            IdentitySession {
                principal: session.context.principal,
                outbound: session.outbound,
                ready: false,
            },
        );
        Ok(())
    }

    fn session_ready(&self, context: &ProviderSessionContext) -> Result<(), ProviderError> {
        let (outbound, current) = {
            let mut state = self.state.lock();
            let Some(session) = state.sessions.get_mut(&context.session) else {
                return Err(ProviderError::Denied {
                    domain: Arc::from(DOMAIN),
                    action: Arc::from("session.ready"),
                    reason: Arc::from("identity session is not open"),
                });
            };
            if session.principal != context.principal
                || session.outbound.source_window() != context.source_window
            {
                return Err(ProviderError::Denied {
                    domain: Arc::from(DOMAIN),
                    action: Arc::from("session.ready"),
                    reason: Arc::from("ready identity does not match the mapped session"),
                });
            }
            if session.ready {
                return Ok(());
            }
            session.ready = true;
            (session.outbound.clone(), state.current.clone())
        };
        if let Err(reason) = self.push_identity(&outbound, &current) {
            self.state.lock().sessions.remove(&context.session);
            self.diagnostics.record(IdentityDiagnostic::PushRefused {
                principal: context.principal.clone(),
                session: context.session,
                reason: reason.clone(),
            });
            return Err(ProviderError::Failed {
                domain: Arc::from(DOMAIN),
                action: Arc::from("session.ready"),
                reason: Arc::from(reason.to_string()),
            });
        }
        Ok(())
    }

    fn session_closed(&self, context: &ProviderSessionContext, _reason: ProviderSessionEnd) {
        remove_exact_session(&mut self.state.lock(), context);
    }

    fn session_revoked(&self, context: &ProviderSessionContext) {
        remove_exact_session(&mut self.state.lock(), context);
    }
}

impl Drop for IdentityProvider {
    fn drop(&mut self) {
        if let Some(observation) = self.observation.get_mut().take() {
            observation.close();
        }
    }
}

#[derive(Debug)]
struct ChangeListener {
    provider: Weak<IdentityProvider>,
}

impl IdentityChangeListener for ChangeListener {
    fn changed(&self, identity: FrozenIdentity) {
        if let Some(provider) = self.provider.upgrade() {
            provider.on_identity_changed(identity);
        }
    }

    fn close(&self) {
        if let Some(provider) = self.provider.upgrade() {
            provider
                .diagnostics
                .record(IdentityDiagnostic::ObservationClosed);
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum IdentityProviderBuildError {
    #[error("identity provider limits must be finite and non-zero")]
    InvalidLimits,
    #[error("identity account observation could not start: {0}")]
    Observation(IdentityDataError),
}

fn remove_exact_session(state: &mut IdentityState, context: &ProviderSessionContext) {
    let matches = state.sessions.get(&context.session).is_some_and(|session| {
        session.principal == context.principal
            && session.outbound.source_window() == context.source_window
    });
    if matches {
        state.sessions.remove(&context.session);
    }
}

fn validate_limits(limits: IdentityProviderLimits) -> Result<(), IdentityProviderBuildError> {
    if [
        limits.maximum_sessions,
        limits.maximum_response_bytes,
        limits.maximum_evidence_bytes,
        limits.maximum_correlation_id_bytes,
        limits.maximum_list_type_bytes,
        limits.maximum_items,
        limits.maximum_relays,
        limits.maximum_text_bytes,
        limits.maximum_thumbnails_per_badge,
    ]
    .contains(&0)
    {
        Err(IdentityProviderBuildError::InvalidLimits)
    } else {
        Ok(())
    }
}

fn correlation_id(
    request: &ProviderRequest,
    limits: IdentityProviderLimits,
) -> Result<&str, ProviderError> {
    let id = request
        .correlation_id
        .as_deref()
        .ok_or_else(|| invalid_payload(request, "id is required"))?;
    if id.is_empty() || id.len() > limits.maximum_correlation_id_bytes {
        return Err(invalid_payload(
            request,
            format!(
                "id must be 1..={} bytes",
                limits.maximum_correlation_id_bytes
            ),
        ));
    }
    Ok(id)
}

fn exact_payload<'a>(
    request: &'a ProviderRequest,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, ProviderError> {
    let payload = request
        .payload
        .as_object()
        .ok_or_else(|| invalid_payload(request, "payload must be a flat object"))?;
    if payload.len() != allowed.len() || payload.keys().any(|key| !allowed.contains(&key.as_str()))
    {
        return Err(invalid_payload(
            request,
            format!("expected exactly these fields: {}", allowed.join(", ")),
        ));
    }
    Ok(payload)
}

fn required_string<'a>(
    payload: &'a Map<String, Value>,
    key: &str,
    request: &ProviderRequest,
) -> Result<&'a str, ProviderError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_payload(request, format!("{key} must be a string")))
}

fn invalid_payload(request: &ProviderRequest, reason: impl Into<Arc<str>>) -> ProviderError {
    ProviderError::InvalidPayload {
        domain: Arc::from(DOMAIN),
        action: Arc::clone(&request.action),
        reason: reason.into(),
    }
}

fn response(
    value: Value,
    limits: IdentityProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    let response =
        BoundedJson::from_value(&value, limits.maximum_response_bytes).map_err(|_| {
            ProviderError::Failed {
                domain: Arc::from(DOMAIN),
                action: Arc::clone(&request.action),
                reason: Arc::from("identity response exceeds its configured byte limit"),
            }
        })?;
    Ok(ProviderCall::completed(Some(response)))
}

fn success_response(
    id: &str,
    value: IdentityValue,
    limits: IdentityProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    let value = match value {
        IdentityValue::Relays(relays) => {
            json!({"type": "identity.getRelays.result", "id": id, "relays": relays})
        }
        IdentityValue::Profile(profile) => {
            json!({"type": "identity.getProfile.result", "id": id, "profile": profile})
        }
        IdentityValue::Follows(pubkeys) => {
            json!({"type": "identity.getFollows.result", "id": id, "pubkeys": pubkeys})
        }
        IdentityValue::List(entries) => {
            json!({"type": "identity.getList.result", "id": id, "entries": entries})
        }
        IdentityValue::Zaps(zaps) => {
            json!({"type": "identity.getZaps.result", "id": id, "zaps": zaps})
        }
        IdentityValue::Mutes(pubkeys) => {
            json!({"type": "identity.getMutes.result", "id": id, "pubkeys": pubkeys})
        }
        IdentityValue::Blocked(pubkeys) => {
            json!({"type": "identity.getBlocked.result", "id": id, "pubkeys": pubkeys})
        }
        IdentityValue::Badges(badges) => {
            json!({"type": "identity.getBadges.result", "id": id, "badges": badges})
        }
    };
    response(value, limits, request)
}

fn error_response(
    action: &str,
    id: &str,
    error: IdentityDataError,
    limits: IdentityProviderLimits,
    request: &ProviderRequest,
) -> Result<ProviderCall, ProviderError> {
    let error = error.to_string();
    let value = match action {
        "getRelays" => {
            json!({"type": "identity.getRelays.result", "id": id, "relays": {}, "error": error})
        }
        "getProfile" => {
            json!({"type": "identity.getProfile.result", "id": id, "profile": null, "error": error})
        }
        "getFollows" => {
            json!({"type": "identity.getFollows.result", "id": id, "pubkeys": [], "error": error})
        }
        "getList" => {
            json!({"type": "identity.getList.result", "id": id, "entries": [], "error": error})
        }
        "getZaps" => {
            json!({"type": "identity.getZaps.result", "id": id, "zaps": [], "error": error})
        }
        "getMutes" => {
            json!({"type": "identity.getMutes.result", "id": id, "pubkeys": [], "error": error})
        }
        "getBlocked" => {
            json!({"type": "identity.getBlocked.result", "id": id, "pubkeys": [], "error": error})
        }
        "getBadges" => {
            json!({"type": "identity.getBadges.result", "id": id, "badges": [], "error": error})
        }
        _ => return Err(invalid_payload(request, "unknown action")),
    };
    response(value, limits, request)
}

fn decode_identity_value(query: &IdentityQuery, value: &BoundedJson) -> Result<IdentityValue, ()> {
    let value = value.decode().map_err(|_| ())?;
    match query {
        IdentityQuery::Relays => serde_json::from_value(value)
            .map(IdentityValue::Relays)
            .map_err(|_| ()),
        IdentityQuery::Profile => serde_json::from_value(value)
            .map(IdentityValue::Profile)
            .map_err(|_| ()),
        IdentityQuery::Follows => serde_json::from_value(value)
            .map(IdentityValue::Follows)
            .map_err(|_| ()),
        IdentityQuery::List { .. } => serde_json::from_value(value)
            .map(IdentityValue::List)
            .map_err(|_| ()),
        IdentityQuery::Zaps => serde_json::from_value(value)
            .map(IdentityValue::Zaps)
            .map_err(|_| ()),
        IdentityQuery::Mutes => serde_json::from_value(value)
            .map(IdentityValue::Mutes)
            .map_err(|_| ()),
        IdentityQuery::Blocked => serde_json::from_value(value)
            .map(IdentityValue::Blocked)
            .map_err(|_| ()),
        IdentityQuery::Badges => serde_json::from_value(value)
            .map(IdentityValue::Badges)
            .map_err(|_| ()),
    }
}

#[cfg(test)]
fn encode_identity_value(value: &IdentityValue, maximum_bytes: usize) -> BoundedJson {
    let value = match value {
        IdentityValue::Relays(value) => serde_json::to_value(value),
        IdentityValue::Profile(value) => serde_json::to_value(value),
        IdentityValue::Follows(value)
        | IdentityValue::List(value)
        | IdentityValue::Mutes(value)
        | IdentityValue::Blocked(value) => serde_json::to_value(value),
        IdentityValue::Zaps(value) => serde_json::to_value(value),
        IdentityValue::Badges(value) => serde_json::to_value(value),
    }
    .expect("identity test value must serialize");
    BoundedJson::from_value(&value, maximum_bytes).expect("identity test value must fit")
}

fn wire_pubkey(identity: &FrozenIdentity) -> &str {
    identity
        .account
        .as_ref()
        .map_or("", |account| account.0.as_ref())
}

fn validate_identity(identity: &FrozenIdentity) -> Result<(), ()> {
    identity
        .account
        .as_ref()
        .map_or(Ok(()), |account| validate_pubkey(&account.0))
}

fn validate_pubkey(pubkey: &str) -> Result<(), ()> {
    if pubkey.len() == 64
        && pubkey
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(())
    }
}

fn validate_text(value: &str, limits: IdentityProviderLimits) -> Result<(), ()> {
    if value.len() <= limits.maximum_text_bytes {
        Ok(())
    } else {
        Err(())
    }
}

fn validate_string_list(values: &[String], limits: IdentityProviderLimits) -> Result<(), ()> {
    if values.len() > limits.maximum_items {
        return Err(());
    }
    values
        .iter()
        .try_for_each(|value| validate_text(value, limits))
}

fn validate_pubkey_list(values: &[String], limits: IdentityProviderLimits) -> Result<(), ()> {
    if values.len() > limits.maximum_items {
        return Err(());
    }
    values.iter().try_for_each(|value| validate_pubkey(value))
}

fn validate_value(
    query: &IdentityQuery,
    value: &IdentityValue,
    limits: IdentityProviderLimits,
) -> Result<(), ()> {
    match (query, value) {
        (IdentityQuery::Relays, IdentityValue::Relays(relays)) => {
            if relays.len() > limits.maximum_relays {
                return Err(());
            }
            relays
                .keys()
                .try_for_each(|relay| validate_text(relay, limits))
        }
        (IdentityQuery::Profile, IdentityValue::Profile(profile)) => {
            if let Some(profile) = profile {
                [
                    &profile.name,
                    &profile.display_name,
                    &profile.about,
                    &profile.picture,
                    &profile.banner,
                    &profile.nip05,
                    &profile.lud16,
                    &profile.website,
                ]
                .into_iter()
                .flatten()
                .try_for_each(|value| validate_text(value, limits))?;
            }
            Ok(())
        }
        (IdentityQuery::Follows, IdentityValue::Follows(values))
        | (IdentityQuery::Mutes, IdentityValue::Mutes(values))
        | (IdentityQuery::Blocked, IdentityValue::Blocked(values)) => {
            validate_pubkey_list(values, limits)
        }
        (IdentityQuery::List { .. }, IdentityValue::List(values)) => {
            validate_string_list(values, limits)
        }
        (IdentityQuery::Zaps, IdentityValue::Zaps(zaps)) => {
            if zaps.len() > limits.maximum_items {
                return Err(());
            }
            for zap in zaps {
                validate_text(&zap.event_id, limits)?;
                validate_pubkey(&zap.sender)?;
                if zap.amount > MAX_SAFE_JSON_INTEGER {
                    return Err(());
                }
                if let Some(content) = &zap.content {
                    validate_text(content, limits)?;
                }
            }
            Ok(())
        }
        (IdentityQuery::Badges, IdentityValue::Badges(badges)) => {
            if badges.len() > limits.maximum_items {
                return Err(());
            }
            for badge in badges {
                validate_text(&badge.id, limits)?;
                validate_pubkey(&badge.awarded_by)?;
                for value in [&badge.name, &badge.description, &badge.image]
                    .into_iter()
                    .flatten()
                {
                    validate_text(value, limits)?;
                }
                if let Some(thumbs) = &badge.thumbs {
                    if thumbs.len() > limits.maximum_thumbnails_per_badge {
                        return Err(());
                    }
                    thumbs
                        .iter()
                        .try_for_each(|thumb| validate_text(thumb, limits))?;
                }
            }
            Ok(())
        }
        _ => Err(()),
    }
}

fn validate_evidence(evidence: &BoundedJson, limits: IdentityProviderLimits) -> Result<(), ()> {
    if evidence.byte_len() > limits.maximum_evidence_bytes {
        return Err(());
    }
    let object = evidence.decode().map_err(|_| ())?;
    let object = object.as_object().ok_or(())?;
    if object.contains_key("synced") || object.contains_key("complete") {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use nmp_native_nap_bridge::{
        BridgeLimits, MemoryActivitySink, ProviderPushObserver, ProviderRegistry, SessionContext,
        SourceWindowId,
    };
    use nmp_native_runtime_core::{
        Cancellation, ExecutionProfile, GrantDecision, GrantLedger, GrantLimits, ResourceClass,
        ResourceLimits, ResourceTracker, Sensitivity,
    };

    use super::*;

    #[derive(Debug)]
    struct FakeObservation {
        closed: AtomicBool,
    }

    impl AccountObservationHandle for FakeObservation {
        fn close(&self) {
            self.closed.store(true, Ordering::Release);
        }
    }

    #[derive(Debug)]
    struct FakeSource {
        identity: Mutex<FrozenIdentity>,
        listener: Mutex<Option<Arc<dyn IdentityChangeListener>>>,
        observation: Arc<FakeObservation>,
        reads: AtomicUsize,
        retarget: AtomicBool,
    }

    impl FakeSource {
        fn new(identity: FrozenIdentity) -> Arc<Self> {
            Arc::new(Self {
                identity: Mutex::new(identity),
                listener: Mutex::new(None),
                observation: Arc::new(FakeObservation {
                    closed: AtomicBool::new(false),
                }),
                reads: AtomicUsize::new(0),
                retarget: AtomicBool::new(false),
            })
        }

        fn change(&self, identity: FrozenIdentity) {
            *self.identity.lock() = identity.clone();
            if let Some(listener) = self.listener.lock().as_ref() {
                listener.changed(identity);
            }
        }
    }

    impl IdentityDataPlane for FakeSource {
        fn freeze_public_identity(&self) -> Result<FrozenIdentity, IdentityDataError> {
            Ok(self.identity.lock().clone())
        }

        fn read_public_identity(
            &self,
            frozen_identity: &FrozenIdentity,
            query: IdentityQuery,
            cancellation: &Cancellation,
            _limits: IdentityReadLimits,
        ) -> Result<IdentityRead, IdentityDataError> {
            if cancellation.is_cancelled() {
                return Err(IdentityDataError::Cancelled);
            }
            self.reads.fetch_add(1, Ordering::Relaxed);
            let value = match query {
                IdentityQuery::Relays => IdentityValue::Relays(BTreeMap::from([(
                    "wss://relay.example".to_owned(),
                    RelayPermission {
                        read: true,
                        write: false,
                    },
                )])),
                IdentityQuery::Profile => IdentityValue::Profile(Some(ProfileData {
                    name: Some("Alice".to_owned()),
                    ..ProfileData::default()
                })),
                IdentityQuery::Follows => IdentityValue::Follows(vec!["b".repeat(64)]),
                IdentityQuery::List { list_type } => {
                    IdentityValue::List(vec![list_type.to_string()])
                }
                IdentityQuery::Zaps => IdentityValue::Zaps(vec![ZapReceipt {
                    event_id: "event".to_owned(),
                    sender: "c".repeat(64),
                    amount: 21_000,
                    content: Some("hello".to_owned()),
                }]),
                IdentityQuery::Mutes => IdentityValue::Mutes(vec!["d".repeat(64)]),
                IdentityQuery::Blocked => IdentityValue::Blocked(vec!["e".repeat(64)]),
                IdentityQuery::Badges => IdentityValue::Badges(vec![Badge {
                    id: "badge".to_owned(),
                    name: Some("Early".to_owned()),
                    description: None,
                    image: None,
                    thumbs: Some(vec!["https://example/thumb.png".to_owned()]),
                    awarded_by: "f".repeat(64),
                }]),
            };
            let returned_identity = if self.retarget.load(Ordering::Acquire) {
                connected_identity(frozen_identity.generation.saturating_add(1), "9".repeat(64))
            } else {
                frozen_identity.clone()
            };
            Ok(IdentityRead {
                frozen_identity: returned_identity,
                value: encode_identity_value(&value, 64 * 1024),
                scoped_evidence: BoundedJson::from_value(
                    &json!({
                        "sources": [{"relay": "wss://relay.example", "status": "requesting"}],
                        "shortfall": [],
                    }),
                    4096,
                )
                .unwrap(),
            })
        }

        fn observe_public_identity(
            &self,
            listener: Arc<dyn IdentityChangeListener>,
        ) -> Result<AccountObservation, IdentityDataError> {
            *self.listener.lock() = Some(listener);
            Ok(AccountObservation {
                current: self.identity.lock().clone(),
                observation: self.observation.clone(),
            })
        }
    }

    #[derive(Debug, Default)]
    struct FakeDiagnostics {
        facts: Mutex<Vec<IdentityDiagnostic>>,
    }

    impl IdentityDiagnosticsSink for FakeDiagnostics {
        fn record(&self, fact: IdentityDiagnostic) {
            self.facts.lock().push(fact);
        }
    }

    fn principal() -> Principal {
        Principal::new("1".repeat(64), "profile", "2".repeat(64)).unwrap()
    }

    fn connected_identity(generation: u64, pubkey: String) -> FrozenIdentity {
        FrozenIdentity {
            generation,
            account: Some(nmp_native_runtime_core::AccountRef(Arc::from(pubkey))),
        }
    }

    fn signed_out_identity(generation: u64) -> FrozenIdentity {
        FrozenIdentity {
            generation,
            account: None,
        }
    }

    fn provider() -> (Arc<IdentityProvider>, Arc<FakeSource>, Arc<FakeDiagnostics>) {
        let source = FakeSource::new(connected_identity(1, "a".repeat(64)));
        let diagnostics = Arc::new(FakeDiagnostics::default());
        let source_dyn: Arc<dyn IdentityDataPlane> = source.clone();
        let diagnostics_dyn: Arc<dyn IdentityDiagnosticsSink> = diagnostics.clone();
        let provider = IdentityProvider::connect(
            source_dyn,
            diagnostics_dyn,
            IdentityProviderLimits::default(),
        )
        .unwrap();
        (provider, source, diagnostics)
    }

    fn opened_session(provider: Arc<IdentityProvider>) -> (ProviderRegistry, ProviderPushObserver) {
        let resources = Arc::new(ResourceTracker::new(ResourceLimits::default()).unwrap());
        let grants =
            Arc::new(GrantLedger::new(GrantLimits::default(), Arc::clone(&resources)).unwrap());
        let activity = Arc::new(MemoryActivitySink::bounded(32));
        let mut registry = ProviderRegistry::new(
            BridgeLimits::default(),
            resources,
            Arc::clone(&grants),
            activity,
        )
        .unwrap();
        let domain = Capability::new(DOMAIN).unwrap();
        grants
            .set(
                principal(),
                domain,
                Sensitivity::Sensitive,
                GrantDecision::AllowExactBuild,
            )
            .unwrap();
        registry.register(provider).unwrap();
        let context = SessionContext {
            id: SessionId(7),
            principal: principal(),
            profile: ExecutionProfile::Legacy,
        };
        let plan = registry
            .negotiate(
                &context.principal,
                context.profile,
                &BTreeSet::from([Capability::new(DOMAIN).unwrap()]),
            )
            .unwrap();
        let observer = registry
            .open_session_bound(&context, &plan, SourceWindowId(11), 0)
            .unwrap();
        registry.mark_session_ready(context.id).unwrap();
        (registry, observer)
    }

    fn drain(observer: &ProviderPushObserver) -> Vec<Value> {
        observer
            .drain(16)
            .unwrap()
            .pushes
            .into_iter()
            .map(|push| push.envelope.decode().unwrap())
            .collect()
    }

    fn request(action: &str, payload: Value) -> ProviderRequest {
        let resources = ResourceTracker::new(ResourceLimits::default()).unwrap();
        let work = resources
            .admit(
                SessionId(7),
                Some(Capability::new(DOMAIN).unwrap()),
                ResourceClass::ProviderCall,
            )
            .unwrap();
        ProviderRequest {
            principal: principal(),
            session: SessionId(7),
            action: Arc::from(action),
            correlation_id: Some(Arc::from("request-1")),
            payload,
            work,
        }
    }

    fn response(provider: &IdentityProvider, action: &str, payload: Value) -> Value {
        provider
            .call(request(action, payload))
            .unwrap()
            .response
            .unwrap()
            .decode()
            .unwrap()
    }

    #[test]
    fn descriptor_covers_every_pinned_request_action() {
        let (provider, _, _) = provider();
        assert_eq!(
            provider.descriptor().actions,
            [
                "getPublicKey",
                "getRelays",
                "getProfile",
                "getFollows",
                "getList",
                "getZaps",
                "getMutes",
                "getBlocked",
                "getBadges",
            ]
            .into_iter()
            .map(Arc::from)
            .collect()
        );
        assert_eq!(
            provider.descriptor().protocol_versions,
            BTreeSet::from([Arc::from(PINNED_NAP_PROTOCOL)])
        );
    }

    #[test]
    fn every_pinned_action_uses_the_exact_flat_result_envelope() {
        let (provider, _, diagnostics) = provider();
        assert_eq!(
            response(&provider, "getPublicKey", json!({})),
            json!({
                "type": "identity.getPublicKey.result",
                "id": "request-1",
                "pubkey": "a".repeat(64),
            })
        );
        assert_eq!(
            response(&provider, "getRelays", json!({}))["type"],
            "identity.getRelays.result"
        );
        assert_eq!(
            response(&provider, "getProfile", json!({}))["profile"]["displayName"],
            Value::Null
        );
        assert_eq!(
            response(&provider, "getFollows", json!({}))["pubkeys"][0],
            "b".repeat(64)
        );
        assert_eq!(
            response(&provider, "getList", json!({"listType": "bookmarks"}))["entries"][0],
            "bookmarks"
        );
        assert_eq!(
            response(&provider, "getZaps", json!({}))["zaps"][0]["amount"],
            21_000
        );
        assert_eq!(
            response(&provider, "getMutes", json!({}))["pubkeys"][0],
            "d".repeat(64)
        );
        assert_eq!(
            response(&provider, "getBlocked", json!({}))["pubkeys"][0],
            "e".repeat(64)
        );
        assert_eq!(
            response(&provider, "getBadges", json!({}))["badges"][0]["awardedBy"],
            "f".repeat(64)
        );
        assert_eq!(
            diagnostics
                .facts
                .lock()
                .iter()
                .filter(|fact| matches!(fact, IdentityDiagnostic::Read { .. }))
                .count(),
            8
        );
    }

    #[test]
    fn malformed_flat_payloads_are_refused_before_source_work() {
        let (provider, source, _) = provider();
        assert!(matches!(
            provider.call(request("getRelays", json!({"payload": {}}))),
            Err(ProviderError::InvalidPayload { .. })
        ));
        assert!(matches!(
            provider.call(request("getList", json!({"list_type": "bookmarks"}))),
            Err(ProviderError::InvalidPayload { .. })
        ));
        assert_eq!(source.reads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn query_is_frozen_and_source_retargeting_fails_closed() {
        let (provider, source, _) = provider();
        source.retarget.store(true, Ordering::Release);
        let result = response(&provider, "getFollows", json!({}));
        assert_eq!(result["pubkeys"], json!([]));
        assert_eq!(
            result["error"],
            IdentityDataError::InvalidSourceData.to_string()
        );
    }

    #[test]
    fn cancellation_returns_the_pinned_default_and_error_shape() {
        let (provider, _, _) = provider();
        let request = request("getBadges", json!({}));
        request.work.cancellation().cancel();
        let result = provider
            .call(request)
            .unwrap()
            .response
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(result["badges"], json!([]));
        assert_eq!(result["error"], IdentityDataError::Cancelled.to_string());
    }

    #[test]
    fn changed_push_has_no_id_and_sign_out_uses_empty_pubkey() {
        let (provider, source, _) = provider();
        let (_registry, observer) = opened_session(Arc::clone(&provider));
        let initial = drain(&observer);
        source.change(connected_identity(2, "b".repeat(64)));
        let changed = drain(&observer);
        source.change(signed_out_identity(3));
        let signed_out = drain(&observer);
        let messages = [initial, changed, signed_out].concat();
        assert_eq!(
            messages[0],
            json!({"type": "identity.changed", "pubkey": "a".repeat(64)})
        );
        assert_eq!(
            messages[1],
            json!({"type": "identity.changed", "pubkey": "b".repeat(64)})
        );
        assert_eq!(
            messages[2],
            json!({"type": "identity.changed", "pubkey": ""})
        );
        assert!(messages.iter().all(|message| message.get("id").is_none()));
    }

    #[test]
    fn stale_change_is_ignored_and_session_close_removes_the_push_lane() {
        let (provider, source, _) = provider();
        let (registry, observer) = opened_session(Arc::clone(&provider));
        assert_eq!(drain(&observer).len(), 1);
        source.change(connected_identity(1, "c".repeat(64)));
        assert!(drain(&observer).is_empty());
        registry.close_session(SessionId(7));
        assert_eq!(provider.active_sessions(), 0);
        assert!(observer.drain(16).unwrap().closed);
        source.change(connected_identity(2, "d".repeat(64)));
        assert!(observer.drain(16).unwrap().pushes.is_empty());
    }

    #[test]
    fn close_is_idempotent_and_closes_the_change_observation() {
        let (provider, source, _) = provider();
        let (_registry, _observer) = opened_session(Arc::clone(&provider));
        provider.close();
        provider.close();
        assert!(source.observation.closed.load(Ordering::Acquire));
        assert_eq!(provider.active_sessions(), 0);
    }

    #[test]
    fn unsafe_source_values_are_bounded_before_crossing_the_bridge() {
        let (provider, source, _) = provider();
        *source.identity.lock() = connected_identity(1, "a".repeat(64));
        let limits = IdentityProviderLimits {
            maximum_items: 1,
            ..IdentityProviderLimits::default()
        };
        assert_eq!(limits.maximum_items, 1);

        let excessive = IdentityValue::Follows(vec!["b".repeat(64), "c".repeat(64)]);
        assert!(validate_value(&IdentityQuery::Follows, &excessive, limits).is_err());
        let unsafe_amount = IdentityValue::Zaps(vec![ZapReceipt {
            event_id: "event".to_owned(),
            sender: "b".repeat(64),
            amount: MAX_SAFE_JSON_INTEGER + 1,
            content: None,
        }]);
        assert!(validate_value(&IdentityQuery::Zaps, &unsafe_amount, limits).is_err());

        let global_claim =
            BoundedJson::from_value(&json!({"sources": [], "synced": true}), 1024).unwrap();
        assert!(validate_evidence(&global_claim, limits).is_err());
        drop(provider);
    }

    #[test]
    fn compiled_contract_matches_the_pinned_inventory_and_tarball_hash() {
        let lock = include_str!("../../../compatibility.lock");
        assert!(lock.contains("nap = \"0.28.0\""));
        assert!(lock.contains(&format!("nap = \"{PINNED_NPM_TARBALL_SHA256}\"")));

        let inventory: Value = serde_json::from_str(include_str!(
            "../../../conformance/envelopes/inventory.json"
        ))
        .unwrap();
        let identity_types = inventory["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["domain"] == DOMAIN)
            .map(|entry| entry["type"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            identity_types,
            BTreeSet::from([
                "identity.changed",
                "identity.getBadges",
                "identity.getBadges.result",
                "identity.getBlocked",
                "identity.getBlocked.result",
                "identity.getFollows",
                "identity.getFollows.result",
                "identity.getList",
                "identity.getList.result",
                "identity.getMutes",
                "identity.getMutes.result",
                "identity.getProfile",
                "identity.getProfile.result",
                "identity.getPublicKey",
                "identity.getPublicKey.result",
                "identity.getRelays",
                "identity.getRelays.result",
                "identity.getZaps",
                "identity.getZaps.result",
            ])
        );
    }
}
