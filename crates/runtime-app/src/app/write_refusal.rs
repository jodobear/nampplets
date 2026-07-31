//! Host-owned terminal delivery for refused provider writes.

use std::sync::Arc;

use nmp_native_nap_bridge::ProviderCall;
use nmp_native_runtime_core::{BoundedJson, Capability, Principal, SessionId};

use super::{ActiveOperation, AppState, RuntimeApp};
use crate::{
    commands::{PlatformEvent, ProviderOperationId},
    views::AppErrorCode,
};

pub(super) enum ProviderOperationAdmission {
    Accepted(Option<ProviderOperationId>),
    Refused,
}

impl RuntimeApp {
    /// Takes the optional streaming/write ownership returned by one provider.
    /// Every unretainable write is completed and projected before the matching
    /// host refusal, so the caller never loses its correlation-bearing result.
    pub(super) fn admit_provider_operation(
        &self,
        state: &mut AppState,
        principal: &Principal,
        session: SessionId,
        domain: Option<&Capability>,
        call: &mut ProviderCall,
        now: u64,
    ) -> ProviderOperationAdmission {
        let mut handle = call.take_operation();
        let mut proposal = call.take_write_proposal();
        if handle.is_some() && proposal.is_some() {
            if let Some(proposal) = proposal.take() {
                let response = proposal.refuse_system(Arc::from(
                    "provider returned both a streaming operation and a write proposal",
                ));
                self.project_write_refusal(state, principal.clone(), session, response, now);
            }
            if let Some(handle) = handle.take() {
                handle.cancel();
            }
            self.refuse(
                state,
                AppErrorCode::Bridge,
                Some(principal.clone()),
                Some(session),
                "provider returned conflicting operation ownership",
                now,
            );
            return ProviderOperationAdmission::Refused;
        }
        if handle.is_none() && proposal.is_none() {
            return ProviderOperationAdmission::Accepted(None);
        }
        if state.operations.len() >= self.limits.maximum_provider_operations {
            if let Some(proposal) = proposal.take() {
                let response =
                    proposal.refuse_system(Arc::from("provider operation capacity is full"));
                self.project_write_refusal(state, principal.clone(), session, response, now);
            }
            if let Some(handle) = handle.take() {
                handle.cancel();
            }
            self.refuse(
                state,
                AppErrorCode::Capacity,
                Some(principal.clone()),
                Some(session),
                "provider operation ownership capacity is full",
                now,
            );
            return ProviderOperationAdmission::Refused;
        }
        let Some(next) = state.next_operation_id.checked_add(1) else {
            if let Some(proposal) = proposal.take() {
                let response = proposal.refuse_system(Arc::from(
                    "provider operation identifier space is exhausted",
                ));
                self.project_write_refusal(state, principal.clone(), session, response, now);
            }
            if let Some(handle) = handle.take() {
                handle.cancel();
            }
            self.refuse(
                state,
                AppErrorCode::Capacity,
                Some(principal.clone()),
                Some(session),
                "provider operation identifier space is exhausted",
                now,
            );
            return ProviderOperationAdmission::Refused;
        };
        let domain = domain
            .cloned()
            .unwrap_or_else(|| Capability::new("unknown").expect("static capability is valid"));
        let id = ProviderOperationId(next);
        state.next_operation_id = next;
        state.operations.insert(
            id,
            ActiveOperation {
                session,
                principal: principal.clone(),
                domain,
                handle,
                proposal,
            },
        );
        ProviderOperationAdmission::Accepted(Some(id))
    }

    /// Projects terminal output without relying on the provider lane that the
    /// same refusal may revoke.
    pub(super) fn project_write_refusal(
        &self,
        state: &mut AppState,
        principal: Principal,
        session: SessionId,
        response: Option<BoundedJson>,
        now: u64,
    ) {
        if let Some(response) = response {
            if let Err(error) = self.bridge.validate_response(&response) {
                self.refuse(
                    state,
                    AppErrorCode::Bridge,
                    Some(principal),
                    Some(session),
                    error.to_string(),
                    now,
                );
                return;
            }
            self.push_event(
                state,
                PlatformEvent::EnvelopeHandled {
                    session,
                    operation: None,
                    response: Some(response),
                },
            );
        }
    }

    pub(super) fn cancel_provider_operation(
        &self,
        state: &mut AppState,
        operation: ActiveOperation,
        reason: Arc<str>,
        now: u64,
    ) {
        let principal = operation.principal.clone();
        let session = operation.session;
        let response = operation.cancel(reason);
        self.project_write_refusal(state, principal, session, response, now);
    }

    pub(super) fn complete_operation(
        &self,
        state: &mut AppState,
        operation_id: ProviderOperationId,
        now: u64,
    ) {
        let Some(operation) = state.operations.remove(&operation_id) else {
            self.refuse(
                state,
                AppErrorCode::Bridge,
                None,
                None,
                "unknown provider operation",
                now,
            );
            return;
        };
        if operation.proposal.is_some() {
            self.cancel_provider_operation(
                state,
                operation,
                Arc::from("pending write requires an approval decision"),
                now,
            );
            self.refuse(
                state,
                AppErrorCode::Bridge,
                None,
                None,
                "a pending provider write cannot be completed without approval",
                now,
            );
            return;
        }
        operation.complete();
        self.push_event(
            state,
            PlatformEvent::ProviderOperationFinished {
                operation: operation_id,
            },
        );
    }
}
