//! Checked bounded activity, event, and refusal publication.

use std::sync::Arc;

use nmp_native_nap_bridge::BridgeError;
use nmp_native_runtime_core::{Principal, SessionError, SessionId};
use nmp_native_runtime_store::{ActivityRecord, StoreError};
use nmp_native_surface::BindingError;

use super::{AppState, RuntimeApp};
use crate::{
    activity::{ActivityDetail, ActivityFact},
    commands::{PlatformEvent, SequencedPlatformEvent},
    views::{AppErrorCode, AppErrorFact, SnapshotSection},
};

impl RuntimeApp {
    pub(super) fn push_event(&self, state: &mut AppState, event: PlatformEvent) {
        if state.terminal_reason.is_some() {
            return;
        }
        let Some(sequence) = state.next_event_sequence.checked_add(1) else {
            self.enter_revision_terminal(state, SnapshotSection::NewestEventSequence);
            return;
        };
        if state
            .events
            .try_push(
                self.limits.maximum_platform_events,
                SequencedPlatformEvent { sequence, event },
            )
            .is_err()
        {
            self.enter_revision_terminal(state, SnapshotSection::NewestEventSequence);
            return;
        }
        state.next_event_sequence = sequence;
    }

    pub(crate) fn record_activity(
        &self,
        state: &mut AppState,
        principal: &Principal,
        category: &str,
        operation: &str,
        outcome: &str,
        now: u64,
    ) {
        self.record_activity_with_details(
            state,
            principal,
            category,
            operation,
            outcome,
            Vec::new(),
            now,
        );
    }

    /// Records one fact with details already classified by the producer.
    pub(crate) fn record_activity_with_details(
        &self,
        state: &mut AppState,
        principal: &Principal,
        category: &str,
        operation: &str,
        outcome: &str,
        details: Vec<ActivityDetail>,
        now: u64,
    ) {
        if state.terminal_reason.is_some() {
            return;
        }
        let fact = ActivityFact::new(
            principal.clone(),
            category,
            operation,
            outcome,
            details,
            now,
        );
        let persisted = ActivityRecord {
            principal: fact.principal.clone(),
            category: Arc::clone(&fact.category),
            operation: Arc::clone(&fact.operation),
            outcome: Arc::clone(&fact.outcome),
            occurred_at_millis: now,
        };
        if state
            .activity
            .try_push(self.limits.maximum_activity_facts, fact)
            .is_err()
        {
            self.enter_revision_terminal(state, SnapshotSection::Activity);
            return;
        }
        if let Err(error) = self.store.append_activity(&persisted) {
            self.record_error(
                state,
                AppErrorFact {
                    code: AppErrorCode::Store,
                    principal: Some(principal.clone()),
                    session: None,
                    detail: Arc::from(error.to_string()),
                    occurred_at_millis: now,
                },
            );
        }
    }

    pub(super) fn refuse(
        &self,
        state: &mut AppState,
        code: AppErrorCode,
        principal: Option<Principal>,
        session: Option<SessionId>,
        detail: impl Into<Arc<str>>,
        now: u64,
    ) {
        let fact = AppErrorFact {
            code,
            principal,
            session,
            detail: detail.into(),
            occurred_at_millis: now,
        };
        self.record_error(state, fact.clone());
        self.push_event(state, PlatformEvent::Refused(fact));
    }

    pub(super) fn record_error(&self, state: &mut AppState, fact: AppErrorFact) {
        if state.terminal_reason.is_some() {
            return;
        }
        if state
            .errors
            .try_push(self.limits.maximum_error_facts, fact)
            .is_err()
        {
            self.enter_revision_terminal(state, SnapshotSection::Errors);
        }
    }

    pub(super) fn refuse_store(
        &self,
        state: &mut AppState,
        principal: Option<Principal>,
        session: Option<SessionId>,
        error: StoreError,
        now: u64,
    ) {
        self.refuse(
            state,
            AppErrorCode::Store,
            principal,
            session,
            error.to_string(),
            now,
        );
    }

    pub(super) fn refuse_bridge(
        &self,
        state: &mut AppState,
        principal: Option<Principal>,
        session: Option<SessionId>,
        error: BridgeError,
        now: u64,
    ) {
        self.refuse(
            state,
            AppErrorCode::Bridge,
            principal,
            session,
            error.to_string(),
            now,
        );
    }

    pub(super) fn refuse_session(
        &self,
        state: &mut AppState,
        principal: Option<Principal>,
        session: Option<SessionId>,
        error: SessionError,
        now: u64,
    ) {
        self.refuse(
            state,
            AppErrorCode::InvalidLifecycle,
            principal,
            session,
            error.to_string(),
            now,
        );
    }

    pub(super) fn refuse_binding(&self, state: &mut AppState, error: BindingError, now: u64) {
        self.refuse(
            state,
            AppErrorCode::Binding,
            None,
            None,
            error.to_string(),
            now,
        );
    }
}
