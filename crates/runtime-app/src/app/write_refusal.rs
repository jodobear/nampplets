//! Host-owned terminal delivery for refused provider writes.

use std::sync::Arc;

use nmp_native_runtime_core::{BoundedJson, SessionId};

use super::{ActiveOperation, AppState, RuntimeApp};
use crate::{
    commands::{PlatformEvent, ProviderOperationId},
    views::AppErrorCode,
};

impl RuntimeApp {
    /// Projects terminal output without relying on the provider lane that the
    /// same refusal may revoke.
    pub(super) fn project_write_refusal(
        &self,
        state: &mut AppState,
        session: SessionId,
        response: Option<BoundedJson>,
    ) {
        if let Some(response) = response {
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
    ) {
        let session = operation.session;
        let response = operation.cancel(reason);
        self.project_write_refusal(state, session, response);
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
