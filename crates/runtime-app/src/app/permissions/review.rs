use std::sync::Arc;

use nmp_native_runtime_core::{GrantDecision, Principal};

use super::{
    AppState, RuntimeApp,
    policy::{permission_decision_policy, permission_provider_projection},
    revision::permission_review_revision,
};
use crate::views::{
    PermissionCapabilityView, PermissionDecisionController, PermissionReviewError,
    PermissionReviewView,
};

impl RuntimeApp {
    /// Builds one bounded exact-build permission review from Rust-owned
    /// installation requests, provider metadata, live session grants, and
    /// durable grant rows. The revision covers the effective policy snapshot.
    pub fn permission_review(
        &self,
        principal: &Principal,
    ) -> Result<PermissionReviewView, PermissionReviewError> {
        let state = self.state.lock();
        self.permission_review_locked(&state, principal)
    }

    pub(super) fn permission_review_locked(
        &self,
        state: &AppState,
        principal: &Principal,
    ) -> Result<PermissionReviewView, PermissionReviewError> {
        let build = state
            .installed
            .get(principal)
            .cloned()
            .ok_or(PermissionReviewError::NotInstalled)?;
        let mut capabilities = Vec::with_capacity(build.capability_requests.len());
        for request in &build.capability_requests {
            let persistent = self
                .store
                .grant_entry(principal, &request.capability)
                .map_err(|error| PermissionReviewError::Store {
                    detail: Arc::from(error.to_string()),
                })?;
            let current_entry = self
                .grants
                .decision_entry(principal, &request.capability)
                .or(persistent);
            let current_decision = current_entry.unwrap_or(GrantDecision::Denied);
            let descriptor = self.bridge.permission_descriptor(&request.capability);
            let (sensitivity, dependencies, platform_availability) =
                permission_provider_projection(descriptor);
            let policy = permission_decision_policy(
                current_decision,
                &platform_availability,
                self.permission_default,
                current_entry.is_none(),
            );
            capabilities.push(PermissionCapabilityView {
                capability: request.capability.clone(),
                requirement: request.requirement,
                sensitivity,
                dependencies,
                platform_availability,
                controller: policy.controller,
                current_decision,
                is_granted: current_decision.allows_without_prompt(),
                requested_decision: policy.requested,
                recommended_decision: policy.recommended,
                decision_options: policy.options,
            });
        }
        let read_only = capabilities.iter().all(|capability| {
            matches!(
                capability.controller,
                PermissionDecisionController::HostPolicy { .. }
            )
        });
        Ok(PermissionReviewView {
            principal: principal.clone(),
            revision: permission_review_revision(principal, &capabilities),
            title: build.title,
            capabilities,
            read_only,
        })
    }
}
