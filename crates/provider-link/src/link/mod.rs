use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use nmp_native_nap_bridge::{
    Provider, ProviderCall, ProviderDescriptor, ProviderError, ProviderPlatformAvailability,
    ProviderPushSender, ProviderRequest, ProviderSession, ProviderSessionContext,
    ProviderSessionEnd,
};
use nmp_native_runtime_core::{Capability, Principal, SessionId, WorkLease};
use parking_lot::Mutex;

use crate::PINNED_NAP_PROTOCOL;

pub const LINK_DOMAIN: &str = "link";

mod types;
mod url;
mod wire;
pub use types::*;
use wire::{LinkTerminal, invalid, lifecycle_error, terminal_fields};

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
#[cfg(test)]
mod tests;
