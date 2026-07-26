use std::{collections::BTreeMap, sync::Arc};

use nmp_native_nap_bridge::{
    ProviderError, ProviderRequest, ProviderSession, ProviderSessionContext,
};
use nmp_native_runtime_core::{Cancellation, Principal, SessionId};

use crate::{
    DOMAIN, ResourceActivity, ResourceActivityAction, ResourceActivityOutcome, ResourceErrorCode,
    ResourceFailure,
    provider::{ActiveRequest, ResourceSession, ResourceShared},
    wire::{decrement_principal, failed, lifecycle_error, take_rate_tokens},
};

impl ResourceShared {
    pub(crate) fn wire_limit(&self) -> usize {
        self.limits
            .maximum_bulk_response_bytes
            .saturating_mul(2)
            .saturating_add(64 * 1024)
    }

    pub(crate) fn open_session(&self, session: ProviderSession) -> Result<(), ProviderError> {
        if session.outbound.domain().as_str() != DOMAIN
            || session.outbound.session() != session.context.session
        {
            return Err(lifecycle_error(
                "outbound resource lane does not match mapped session",
            ));
        }
        let mut state = self.state.lock();
        if state.closed {
            return Err(lifecycle_error("resource provider is closed"));
        }
        if let Some(existing) = state.sessions.get(&session.context.session) {
            return if existing.principal == session.context.principal
                && existing.outbound.source_window() == session.context.source_window
            {
                Ok(())
            } else {
                Err(lifecycle_error("mapped resource session identity changed"))
            };
        }
        if state.sessions.len() >= self.limits.maximum_sessions {
            return Err(lifecycle_error("resource session capacity is full"));
        }
        state.sessions.insert(
            session.context.session,
            ResourceSession {
                principal: session.context.principal,
                outbound: session.outbound,
                ready: false,
                active: BTreeMap::new(),
            },
        );
        Ok(())
    }

    pub(crate) fn ready_session(
        &self,
        context: &ProviderSessionContext,
    ) -> Result<(), ProviderError> {
        let mut state = self.state.lock();
        let session = state
            .sessions
            .get_mut(&context.session)
            .ok_or_else(|| lifecycle_error("resource session was not opened"))?;
        if session.principal != context.principal
            || session.outbound.source_window() != context.source_window
        {
            return Err(lifecycle_error("mapped resource session identity changed"));
        }
        session.ready = true;
        Ok(())
    }

    pub(crate) fn validate_call_context(
        &self,
        request: &ProviderRequest,
    ) -> Result<(), ProviderError> {
        let state = self.state.lock();
        let session = state
            .sessions
            .get(&request.session)
            .ok_or_else(|| failed(request, "resource session is not open"))?;
        if session.principal != request.principal || !session.ready {
            return Err(failed(
                request,
                "resource session is not ready for this exact principal",
            ));
        }
        Ok(())
    }

    pub(crate) fn reserve(
        &self,
        principal: &Principal,
        session_id: SessionId,
        id: Arc<str>,
        cancellation: Cancellation,
        url_count: usize,
    ) -> Result<u64, ResourceFailure> {
        let now = self.clock.monotonic_millis();
        let mut state = self.state.lock();
        if state.closed {
            return Err(ResourceFailure::new(
                ResourceErrorCode::NetworkError,
                "resource provider is closed",
            ));
        }
        {
            let session = state.sessions.get(&session_id).ok_or_else(|| {
                ResourceFailure::new(
                    ResourceErrorCode::InvalidRequest,
                    "resource session is not open",
                )
            })?;
            if &session.principal != principal || !session.ready {
                return Err(ResourceFailure::new(
                    ResourceErrorCode::BlockedByPolicy,
                    "resource session identity is not ready",
                ));
            }
            if session.active.contains_key(&id) {
                return Err(ResourceFailure::new(
                    ResourceErrorCode::InvalidRequest,
                    "resource request id is already active",
                ));
            }
        }
        if state.total_in_flight.saturating_add(url_count)
            > self.limits.maximum_total_in_flight_urls
            || state
                .principal_in_flight
                .get(principal)
                .copied()
                .unwrap_or(0)
                .saturating_add(url_count)
                > self.limits.maximum_in_flight_urls_per_napplet
        {
            return Err(ResourceFailure::new(
                ResourceErrorCode::BlockedByPolicy,
                "resource in-flight capacity is full",
            ));
        }
        if !take_rate_tokens(
            &mut state.rate,
            principal,
            now,
            url_count,
            self.limits.maximum_requests_per_napplet_per_minute,
        ) {
            return Err(ResourceFailure::new(
                ResourceErrorCode::BlockedByPolicy,
                "resource per-napplet rate limit was exceeded",
            ));
        }
        state.next_token = state.next_token.wrapping_add(1).max(1);
        let token = state.next_token;
        state.total_in_flight += url_count;
        *state
            .principal_in_flight
            .entry(principal.clone())
            .or_default() += url_count;
        state
            .sessions
            .get_mut(&session_id)
            .expect("session validated while lock is held")
            .active
            .insert(
                id,
                ActiveRequest {
                    token,
                    cancellation,
                    url_count,
                },
            );
        Ok(token)
    }

    pub(crate) fn release(&self, session_id: SessionId, id: &str, token: u64) {
        let mut state = self.state.lock();
        let removed = state.sessions.get_mut(&session_id).and_then(|session| {
            session
                .active
                .get(id)
                .is_some_and(|active| active.token == token)
                .then(|| session.active.remove(id))
                .flatten()
                .map(|active| (session.principal.clone(), active.url_count))
        });
        if let Some((principal, url_count)) = removed {
            state.total_in_flight = state.total_in_flight.saturating_sub(url_count);
            decrement_principal(&mut state.principal_in_flight, &principal, url_count);
        }
    }

    pub(crate) fn cancel_request(
        &self,
        principal: &Principal,
        session: SessionId,
        id: &str,
    ) -> bool {
        let state = self.state.lock();
        let Some(session) = state.sessions.get(&session) else {
            return false;
        };
        if &session.principal != principal {
            return false;
        }
        session
            .active
            .get(id)
            .is_some_and(|request| request.cancellation.cancel())
    }

    pub(crate) fn remove_session(&self, context: &ProviderSessionContext) {
        let removed = {
            let mut state = self.state.lock();
            let Some(session) = state.sessions.get(&context.session) else {
                return;
            };
            if session.principal != context.principal
                || session.outbound.source_window() != context.source_window
            {
                return;
            }
            let session = state
                .sessions
                .remove(&context.session)
                .expect("session was present while lock is held");
            for active in session.active.values() {
                active.cancellation.cancel();
                state.total_in_flight = state.total_in_flight.saturating_sub(active.url_count);
                decrement_principal(
                    &mut state.principal_in_flight,
                    &session.principal,
                    active.url_count,
                );
            }
            let url_count = session.active.values().map(|active| active.url_count).sum();
            if !state
                .sessions
                .values()
                .any(|remaining| remaining.principal == session.principal)
            {
                state.rate.remove(&session.principal);
            }
            (session.principal, url_count)
        };
        self.activity.record(ResourceActivity {
            principal: removed.0,
            session: context.session,
            action: ResourceActivityAction::LifecycleCleanup,
            outcome: ResourceActivityOutcome::Cancelled,
            url_count: removed.1,
            delivered_bytes: 0,
        });
    }

    pub(crate) fn close(&self) {
        let sessions = {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.total_in_flight = 0;
            state.principal_in_flight.clear();
            state.rate.clear();
            std::mem::take(&mut state.sessions)
        };
        for session in sessions.into_values() {
            for active in session.active.into_values() {
                active.cancellation.cancel();
            }
        }
    }
}
