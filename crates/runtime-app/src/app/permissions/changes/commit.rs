use std::collections::BTreeSet;

use nmp_native_runtime_core::{Capability, GrantBatchError, Principal};

use super::{AppState, RuntimeApp, validate::ValidatedPermissionChanges};
use crate::{
    commands::PlatformEvent,
    views::{PermissionChangeRefusalCode, PermissionChangeResult, PermissionChangeSuccess},
};

impl RuntimeApp {
    pub(super) fn commit_permission_changes(
        &self,
        state: &mut AppState,
        principal: Principal,
        validated: ValidatedPermissionChanges,
        now: u64,
    ) -> PermissionChangeResult {
        let persistent = validated
            .decisions
            .iter()
            .map(|decision| (decision.capability.clone(), decision.decision))
            .collect::<Vec<_>>();
        match self
            .grants
            .commit_batch(principal.clone(), &validated.ledger_changes, || {
                self.store.set_grants_atomic(&principal, &persistent)
            }) {
            Ok(()) => {}
            Err(GrantBatchError::Grant(error)) => {
                return Err(self.permission_change_refusal(
                    state,
                    PermissionChangeRefusalCode::Grant,
                    Some(principal),
                    error.to_string(),
                    None,
                    now,
                ));
            }
            Err(GrantBatchError::Commit(error)) => {
                return Err(self.permission_change_refusal(
                    state,
                    PermissionChangeRefusalCode::Store,
                    Some(principal),
                    error.to_string(),
                    None,
                    now,
                ));
            }
        }

        let mut revoked_domains = BTreeSet::<Capability>::new();
        for decision in &validated.decisions {
            let prior = validated.previous[&decision.capability];
            if prior.allows_without_prompt() && !decision.decision.allows_without_prompt() {
                revoked_domains.extend(self.bridge.revocation_scope(&decision.capability));
            }
            self.record_activity(
                state,
                &principal,
                "grant",
                decision.capability.as_str(),
                super::super::grant_outcome(decision.decision),
                now,
            );
        }
        self.cancel_provider_operations_for_domains(state, &principal, &revoked_domains, now);
        for domain in revoked_domains {
            self.bridge.cancel_capability_work(&principal, &domain);
        }
        self.push_event(
            state,
            PlatformEvent::PermissionChangesApplied {
                principal: principal.clone(),
                decisions: validated.decisions,
            },
        );
        match self.permission_review_locked(state, &principal) {
            Ok(review) => Ok(PermissionChangeSuccess {
                changed: true,
                review,
            }),
            Err(error) => Err(self.permission_change_refusal(
                state,
                PermissionChangeRefusalCode::Store,
                Some(principal),
                error.to_string(),
                None,
                now,
            )),
        }
    }
}
