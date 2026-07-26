//! Exact-build permission review and direct grant commands.

use std::sync::{Arc, atomic::Ordering};

use nmp_native_runtime_app::PlatformCommand;
use nmp_native_runtime_core::{Capability, GrantDecision, Principal, Sensitivity};

use super::RuntimeController;
use crate::{
    RuntimeExactBuildCoordinate, RuntimeGrantDecision, RuntimePermissionReviewResult,
    RuntimeSensitivity, VerifiedArtifact, projection::project_permission_review,
    support::bump_signal,
};

#[uniffi::export]
impl RuntimeController {
    /// Returns one bounded Rust-owned review for an installed exact build.
    /// This never grants or launches the napplet.
    pub fn permission_review(
        &self,
        coordinate: RuntimeExactBuildCoordinate,
    ) -> RuntimePermissionReviewResult {
        if self.closed.load(Ordering::Acquire) {
            return RuntimePermissionReviewResult {
                review: None,
                refusal: Some(self.refusal("closed", "runtime is closed")),
            };
        }
        let principal = match Principal::new(
            coordinate.manifest_author,
            coordinate.d_tag,
            coordinate.aggregate_hash,
        ) {
            Ok(principal) => principal,
            Err(error) => {
                let refusal =
                    self.workspace_refusal("invalid-exact-build-coordinate", error.to_string());
                return RuntimePermissionReviewResult {
                    review: None,
                    refusal: Some(refusal),
                };
            }
        };
        match self.app.permission_review(&principal) {
            Ok(review) => RuntimePermissionReviewResult {
                review: Some(project_permission_review(review)),
                refusal: None,
            },
            Err(error) => {
                let refusal = self.workspace_refusal("permission-review", error.to_string());
                RuntimePermissionReviewResult {
                    review: None,
                    refusal: Some(refusal),
                }
            }
        }
    }

    pub fn set_grant(
        &self,
        artifact: Arc<VerifiedArtifact>,
        capability: String,
        sensitivity: RuntimeSensitivity,
        decision: RuntimeGrantDecision,
    ) {
        let Some(principal) = artifact.principal.clone() else {
            self.record_refusal(
                "unsupported-manifest-identity",
                "grant target has no exact-build principal",
            );
            return;
        };
        let capability = match Capability::new(capability) {
            Ok(capability) => capability,
            Err(error) => {
                self.record_refusal("invalid-capability", error.to_string());
                return;
            }
        };
        self.app.dispatch(PlatformCommand::SetGrant {
            principal,
            capability,
            sensitivity: match sensitivity {
                RuntimeSensitivity::Ordinary => Sensitivity::Ordinary,
                RuntimeSensitivity::Sensitive => Sensitivity::Sensitive,
            },
            decision: match decision {
                RuntimeGrantDecision::Denied => GrantDecision::Denied,
                RuntimeGrantDecision::AskEveryTime => GrantDecision::AskEveryTime,
                RuntimeGrantDecision::AllowSession => GrantDecision::AllowSession,
                RuntimeGrantDecision::AllowExactBuild => GrantDecision::AllowExactBuild,
            },
        });
        bump_signal(&self.signal);
    }

    pub fn revoke(&self, artifact: Arc<VerifiedArtifact>, capability: String) {
        let Some(principal) = artifact.principal.clone() else {
            self.record_refusal(
                "unsupported-manifest-identity",
                "revocation target has no exact-build principal",
            );
            return;
        };
        let capability = match Capability::new(capability) {
            Ok(capability) => capability,
            Err(error) => {
                self.record_refusal("invalid-capability", error.to_string());
                return;
            }
        };
        self.app.dispatch(PlatformCommand::Revoke {
            principal,
            capability,
        });
        bump_signal(&self.signal);
    }
}
