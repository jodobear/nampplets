use std::sync::Arc;

use nmp_native_runtime_core::{CapabilityRequirement, GrantDecision, Principal, Sensitivity};
use sha2::{Digest, Sha256};

use crate::views::{
    PermissionCapabilityView, PermissionDecisionController, PermissionPlatformAvailability,
};

pub(super) fn permission_review_revision(
    principal: &Principal,
    capabilities: &[PermissionCapabilityView],
) -> Arc<str> {
    let mut digest = Sha256::new();
    put(&mut digest, b"nmp-native-permission-review-v1");
    put(&mut digest, principal.manifest_author().as_bytes());
    put(&mut digest, principal.d_tag().as_bytes());
    put(&mut digest, principal.aggregate_hash().as_bytes());
    put(&mut digest, &capabilities.len().to_be_bytes());
    for capability in capabilities {
        put(&mut digest, capability.capability.as_str().as_bytes());
        put(
            &mut digest,
            &[match capability.requirement {
                CapabilityRequirement::Required => 0,
                CapabilityRequirement::Optional => 1,
            }],
        );
        put(
            &mut digest,
            &[match capability.sensitivity {
                None => 0,
                Some(Sensitivity::Ordinary) => 1,
                Some(Sensitivity::Sensitive) => 2,
            }],
        );
        put_availability(&mut digest, &capability.platform_availability);
        put_controller(&mut digest, &capability.controller);
        put_decision(&mut digest, Some(capability.current_decision));
        put_decision(&mut digest, capability.requested_decision);
        put_decision(&mut digest, capability.recommended_decision);
        for dependency in &capability.dependencies {
            put(&mut digest, dependency.as_str().as_bytes());
        }
        for option in &capability.decision_options {
            put_decision(&mut digest, Some(option.decision));
            put(&mut digest, &[u8::from(option.valid)]);
            put(
                &mut digest,
                option.invalid_reason.as_deref().unwrap_or("").as_bytes(),
            );
        }
    }
    Arc::from(hex::encode(digest.finalize()))
}

fn put(digest: &mut Sha256, value: &[u8]) {
    digest.update(value.len().to_be_bytes());
    digest.update(value);
}

fn put_decision(digest: &mut Sha256, decision: Option<GrantDecision>) {
    let value = match decision {
        None => 0,
        Some(GrantDecision::Denied) => 1,
        Some(GrantDecision::AskEveryTime) => 2,
        Some(GrantDecision::AllowSession) => 3,
        Some(GrantDecision::AllowExactBuild) => 4,
        Some(GrantDecision::Managed) => 5,
    };
    put(digest, &[value]);
}

fn put_availability(digest: &mut Sha256, availability: &PermissionPlatformAvailability) {
    match availability {
        PermissionPlatformAvailability::Available => put(digest, &[0]),
        PermissionPlatformAvailability::Unknown { reason } => {
            put(digest, &[1]);
            put(digest, reason.as_bytes());
        }
        PermissionPlatformAvailability::Unavailable { reason } => {
            put(digest, &[2]);
            put(digest, reason.as_bytes());
        }
    }
}

fn put_controller(digest: &mut Sha256, controller: &PermissionDecisionController) {
    match controller {
        PermissionDecisionController::User => put(digest, &[0]),
        PermissionDecisionController::HostPolicy { reason } => {
            put(digest, &[1]);
            put(digest, reason.as_bytes());
        }
    }
}
