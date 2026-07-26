use std::collections::{BTreeMap, BTreeSet};

use nmp_native_runtime_core::{Capability, GrantDecision, Principal, Sensitivity};

use super::{AppState, RuntimeApp};
use crate::views::{
    PermissionChangeRefusal, PermissionChangeRefusalCode, PermissionDecision,
    PermissionDecisionController, PermissionReviewView,
};

pub(super) struct ValidatedPermissionChanges {
    pub decisions: Vec<PermissionDecision>,
    pub previous: BTreeMap<Capability, GrantDecision>,
    pub ledger_changes: Vec<(Capability, Sensitivity, GrantDecision)>,
}

impl RuntimeApp {
    pub(super) fn validate_permission_changes(
        &self,
        state: &mut AppState,
        principal: &Principal,
        review: &PermissionReviewView,
        decisions: Vec<PermissionDecision>,
        now: u64,
    ) -> Result<ValidatedPermissionChanges, PermissionChangeRefusal> {
        if decisions.is_empty() {
            return Err(self.permission_change_refusal(
                state,
                PermissionChangeRefusalCode::EmptyChanges,
                Some(principal.clone()),
                "permission change intent must name at least one user-owned capability",
                Some(review.clone()),
                now,
            ));
        }
        let capabilities = review
            .capabilities
            .iter()
            .map(|capability| (capability.capability.clone(), capability))
            .collect::<BTreeMap<_, _>>();
        let mut unique = BTreeSet::new();
        let mut selected = BTreeMap::new();
        for decision in decisions {
            if !unique.insert(decision.capability.clone()) {
                return Err(self.permission_change_refusal(
                    state,
                    PermissionChangeRefusalCode::DuplicateCapability,
                    Some(principal.clone()),
                    format!(
                        "permission change repeats capability {}",
                        decision.capability
                    ),
                    Some(review.clone()),
                    now,
                ));
            }
            let Some(capability) = capabilities.get(&decision.capability) else {
                return Err(self.permission_change_refusal(
                    state,
                    PermissionChangeRefusalCode::UnknownCapability,
                    Some(principal.clone()),
                    format!(
                        "permission change names unrequested capability {}",
                        decision.capability
                    ),
                    Some(review.clone()),
                    now,
                ));
            };
            if matches!(
                capability.controller,
                PermissionDecisionController::HostPolicy { .. }
            ) {
                return Err(self.permission_change_refusal(
                    state,
                    PermissionChangeRefusalCode::ManagedCapability,
                    Some(principal.clone()),
                    format!(
                        "capability {} is controlled by host policy",
                        decision.capability
                    ),
                    Some(review.clone()),
                    now,
                ));
            }
            if decision.decision == GrantDecision::Managed {
                return Err(self.permission_change_refusal(
                    state,
                    PermissionChangeRefusalCode::InvalidDecision,
                    Some(principal.clone()),
                    "managed decisions may be set only by host policy",
                    Some(review.clone()),
                    now,
                ));
            }
            let offered = capability
                .decision_options
                .iter()
                .find(|option| option.decision == decision.decision);
            if !offered.is_some_and(|option| option.valid) {
                return Err(self.permission_change_refusal(
                    state,
                    PermissionChangeRefusalCode::DecisionUnavailable,
                    Some(principal.clone()),
                    format!(
                        "decision {:?} is unavailable for capability {}",
                        decision.decision, decision.capability
                    ),
                    Some(review.clone()),
                    now,
                ));
            }
            selected.insert(decision.capability, decision.decision);
        }
        self.validate_change_dependencies(state, principal, review, &selected, now)?;

        let mut actual = Vec::new();
        let mut previous = BTreeMap::new();
        let mut ledger_changes = Vec::new();
        for (capability, decision) in selected {
            let projected = capabilities[&capability];
            if projected.current_decision == decision {
                continue;
            }
            previous.insert(capability.clone(), projected.current_decision);
            let sensitivity = projected.sensitivity.unwrap_or(Sensitivity::Sensitive);
            ledger_changes.push((capability.clone(), sensitivity, decision));
            actual.push(PermissionDecision {
                capability,
                decision,
            });
        }
        Ok(ValidatedPermissionChanges {
            decisions: actual,
            previous,
            ledger_changes,
        })
    }

    fn validate_change_dependencies(
        &self,
        state: &mut AppState,
        principal: &Principal,
        review: &PermissionReviewView,
        selected: &BTreeMap<Capability, GrantDecision>,
        now: u64,
    ) -> Result<(), PermissionChangeRefusal> {
        let current = review
            .capabilities
            .iter()
            .map(|capability| (capability.capability.clone(), capability.current_decision))
            .collect::<BTreeMap<_, _>>();
        for capability in &review.capabilities {
            let decision = selected
                .get(&capability.capability)
                .copied()
                .unwrap_or(capability.current_decision);
            if !decision.allows_without_prompt() {
                continue;
            }
            for dependency in &capability.dependencies {
                let dependency_decision = selected
                    .get(dependency)
                    .copied()
                    .or_else(|| current.get(dependency).copied())
                    .or_else(|| self.current_grant_decision(principal, dependency).ok())
                    .unwrap_or(GrantDecision::Denied);
                if !dependency_decision.allows_without_prompt() {
                    return Err(self.permission_change_refusal(
                        state,
                        PermissionChangeRefusalCode::DependencyDenied,
                        Some(principal.clone()),
                        format!(
                            "capability {} requires allowed dependency {}",
                            capability.capability, dependency
                        ),
                        Some(review.clone()),
                        now,
                    ));
                }
            }
        }
        Ok(())
    }
}
