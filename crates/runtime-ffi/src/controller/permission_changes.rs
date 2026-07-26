//! Revision-bound, atomic permission changes exposed to native callers.

use std::sync::{Arc, atomic::Ordering};

use nmp_native_runtime_app::{
    PermissionChangeRefusalCode, PermissionChangeRequest, PermissionDecision,
};
use nmp_native_runtime_core::{Capability, Principal};

use super::RuntimeController;
use crate::{
    MAXIMUM_PERMISSION_DECISIONS, RuntimePermissionBatchUpdate, RuntimePermissionChangeRefusal,
    RuntimePermissionChangeRefusalCode, RuntimePermissionDecisionBatch,
    projection::{grant_decision, project_permission_review},
    support::bump_signal,
};

#[uniffi::export]
impl RuntimeController {
    /// Applies only the user-owned domains changed from the supplied review.
    /// Rust validates the review revision and commits the changes atomically.
    /// Success never launches the napplet.
    pub fn apply_permission_decisions(
        &self,
        batch: RuntimePermissionDecisionBatch,
    ) -> RuntimePermissionBatchUpdate {
        if self.closed.load(Ordering::Acquire) {
            return refused(
                RuntimePermissionChangeRefusalCode::Closed,
                "runtime is closed",
            );
        }
        if batch.decisions.is_empty() {
            return refused(
                RuntimePermissionChangeRefusalCode::EmptyChanges,
                "permission change set is empty",
            );
        }
        if batch.decisions.len() > MAXIMUM_PERMISSION_DECISIONS {
            return refused(
                RuntimePermissionChangeRefusalCode::Capacity,
                format!(
                    "permission change set has {} decisions; the maximum is {MAXIMUM_PERMISSION_DECISIONS}",
                    batch.decisions.len()
                ),
            );
        }
        if !valid_review_revision(&batch.review_revision) {
            return refused(
                RuntimePermissionChangeRefusalCode::InvalidRevision,
                "permission review revision must be exactly 64 lowercase hexadecimal bytes",
            );
        }
        let principal = match Principal::new(
            batch.coordinate.manifest_author,
            batch.coordinate.d_tag,
            batch.coordinate.aggregate_hash,
        ) {
            Ok(principal) => principal,
            Err(error) => {
                return refused(
                    RuntimePermissionChangeRefusalCode::InvalidCoordinate,
                    error.to_string(),
                );
            }
        };
        let mut decisions = Vec::with_capacity(batch.decisions.len());
        for selection in batch.decisions {
            let capability = match Capability::new(selection.domain) {
                Ok(capability) => capability,
                Err(error) => {
                    return refused(
                        RuntimePermissionChangeRefusalCode::InvalidDomain,
                        error.to_string(),
                    );
                }
            };
            decisions.push(PermissionDecision {
                capability,
                decision: grant_decision(selection.decision),
            });
        }

        let result = self.app.apply_permission_changes(PermissionChangeRequest {
            principal,
            review_revision: Arc::from(batch.review_revision),
            decisions,
        });
        bump_signal(&self.signal);
        match result {
            Ok(success) => RuntimePermissionBatchUpdate {
                applied: true,
                changed: success.changed,
                review: Some(project_permission_review(success.review)),
                refusal: None,
            },
            Err(refusal) => RuntimePermissionBatchUpdate {
                applied: false,
                changed: false,
                review: refusal
                    .current_review
                    .map(|review| project_permission_review(*review)),
                refusal: Some(RuntimePermissionChangeRefusal {
                    code: map_refusal_code(refusal.code),
                    detail: refusal.detail.to_string(),
                }),
            },
        }
    }
}

fn valid_review_revision(revision: &str) -> bool {
    revision.len() == 64
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn refused(
    code: RuntimePermissionChangeRefusalCode,
    detail: impl Into<String>,
) -> RuntimePermissionBatchUpdate {
    RuntimePermissionBatchUpdate {
        applied: false,
        changed: false,
        review: None,
        refusal: Some(RuntimePermissionChangeRefusal {
            code,
            detail: detail.into(),
        }),
    }
}

fn map_refusal_code(code: PermissionChangeRefusalCode) -> RuntimePermissionChangeRefusalCode {
    match code {
        PermissionChangeRefusalCode::Closed => RuntimePermissionChangeRefusalCode::Closed,
        PermissionChangeRefusalCode::NotInstalled => {
            RuntimePermissionChangeRefusalCode::NotInstalled
        }
        PermissionChangeRefusalCode::StaleReview => RuntimePermissionChangeRefusalCode::StaleReview,
        PermissionChangeRefusalCode::EmptyChanges => {
            RuntimePermissionChangeRefusalCode::EmptyChanges
        }
        PermissionChangeRefusalCode::DuplicateCapability => {
            RuntimePermissionChangeRefusalCode::DuplicateCapability
        }
        PermissionChangeRefusalCode::UnknownCapability => {
            RuntimePermissionChangeRefusalCode::UnknownCapability
        }
        PermissionChangeRefusalCode::ManagedCapability => {
            RuntimePermissionChangeRefusalCode::ManagedCapability
        }
        PermissionChangeRefusalCode::InvalidDecision => {
            RuntimePermissionChangeRefusalCode::InvalidDecision
        }
        PermissionChangeRefusalCode::DecisionUnavailable => {
            RuntimePermissionChangeRefusalCode::DecisionUnavailable
        }
        PermissionChangeRefusalCode::DependencyDenied => {
            RuntimePermissionChangeRefusalCode::DependencyDenied
        }
        PermissionChangeRefusalCode::Grant => RuntimePermissionChangeRefusalCode::Grant,
        PermissionChangeRefusalCode::Store => RuntimePermissionChangeRefusalCode::Store,
    }
}
