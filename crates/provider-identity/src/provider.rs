use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushError, ProviderPushSender, ProviderRequest, ProviderSession,
    ProviderSessionContext, ProviderSessionEnd,
};
use nmp_native_runtime_core::{BoundedJson, Capability, Principal, SessionId};
use parking_lot::Mutex;
use serde_json::json;
use thiserror::Error;

use crate::{
    AccountObservationHandle, DOMAIN, FrozenIdentity, IdentityChangeListener, IdentityDataError,
    IdentityDataPlane, IdentityDiagnostic, IdentityDiagnosticsSink, IdentityProviderLimits,
    IdentityQuery, IdentityReadLimits, PINNED_NAP_PROTOCOL,
    validate::{validate_evidence, validate_identity, validate_value, wire_pubkey},
    wire::{
        correlation_id, decode_identity_value, error_response, exact_payload, invalid_payload,
        required_string, response, success_response,
    },
};

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
