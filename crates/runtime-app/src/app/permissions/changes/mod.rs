mod commit;
mod validate;

use std::sync::Arc;

use nmp_native_runtime_core::Principal;

use super::{AppState, RuntimeApp};
use crate::{
    commands::PermissionChangeRequest,
    views::{
        AppErrorCode, PermissionChangeRefusal, PermissionChangeRefusalCode, PermissionChangeResult,
        PermissionChangeSuccess, PermissionReviewError, PermissionReviewView,
    },
};

impl RuntimeApp {
    pub fn apply_permission_changes(
        &self,
        request: PermissionChangeRequest,
    ) -> PermissionChangeResult {
        let now = self.clock.now_millis();
        let mut state = self.state.lock();
        let result = if state.closed {
            Err(self.permission_change_refusal(
                &mut state,
                PermissionChangeRefusalCode::Closed,
                Some(request.principal),
                "runtime is closed",
                None,
                now,
            ))
        } else {
            self.apply_permission_changes_locked(&mut state, request, now)
        };
        self.publish(&mut state);
        result
    }

    pub(crate) fn apply_permission_changes_locked(
        &self,
        state: &mut AppState,
        request: PermissionChangeRequest,
        now: u64,
    ) -> PermissionChangeResult {
        let current = match self.permission_review_locked(state, &request.principal) {
            Ok(review) => review,
            Err(error) => {
                let code = match error {
                    PermissionReviewError::NotInstalled => {
                        PermissionChangeRefusalCode::NotInstalled
                    }
                    PermissionReviewError::Store { .. } => PermissionChangeRefusalCode::Store,
                };
                return Err(self.permission_change_refusal(
                    state,
                    code,
                    Some(request.principal),
                    error.to_string(),
                    None,
                    now,
                ));
            }
        };
        if request.review_revision != current.revision {
            return Err(self.permission_change_refusal(
                state,
                PermissionChangeRefusalCode::StaleReview,
                Some(request.principal),
                "permission review revision is stale",
                Some(current),
                now,
            ));
        }
        let validated = self.validate_permission_changes(
            state,
            &request.principal,
            &current,
            request.decisions,
            now,
        )?;
        if validated.decisions.is_empty() {
            return Ok(PermissionChangeSuccess {
                changed: false,
                review: current,
            });
        }
        self.commit_permission_changes(state, request.principal, validated, now)
    }

    pub(super) fn permission_change_refusal(
        &self,
        state: &mut AppState,
        code: PermissionChangeRefusalCode,
        principal: Option<Principal>,
        detail: impl Into<Arc<str>>,
        current_review: Option<PermissionReviewView>,
        now: u64,
    ) -> PermissionChangeRefusal {
        let detail = detail.into();
        let app_code = match code {
            PermissionChangeRefusalCode::Closed => AppErrorCode::Closed,
            PermissionChangeRefusalCode::NotInstalled => AppErrorCode::NotInstalled,
            PermissionChangeRefusalCode::Store => AppErrorCode::Store,
            _ => AppErrorCode::Grant,
        };
        self.refuse(state, app_code, principal, None, Arc::clone(&detail), now);
        PermissionChangeRefusal {
            code,
            detail,
            current_review: current_review.map(Box::new),
        }
    }
}
