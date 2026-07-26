//! Cucumber scenario runner for `crates/runtime-app`.
//!
//! Scenarios live under `tests/features/*.feature` as Gherkin. Every step
//! below drives the same `Rig` test harness used by `tests/kernel_*.rs`
//! (see `tests/support/mod.rs`), so a scenario and a `#[test]` exercise
//! identical bootstrap and dispatch paths.

#[path = "bdd/section_revision_steps.rs"]
mod section_revision_steps;
mod support;

use std::collections::BTreeSet;

use cucumber::{World, given, then, when};
use nmp_native_nap_bridge::{ProviderPushError, ProviderPushSender};
use nmp_native_runtime_app::{
    AppErrorCode, PermissionChangeRefusalCode, PermissionChangeRequest, PermissionChangeResult,
    PermissionPlatformAvailability, PermissionReviewView, PlatformCommand,
};
use nmp_native_runtime_core::{
    CapabilityRequirement, GrantDecision, Principal, Sensitivity, SessionId,
};
use support::*;

#[derive(Debug, Default, World)]
struct RuntimeWorld {
    rig: Option<Rig>,
    principal: Option<Principal>,
    session: Option<SessionId>,
    sender: Option<ProviderPushSender>,
    review: Option<PermissionReviewView>,
    permission_result: Option<PermissionChangeResult>,
    push_result: Option<Result<u64, ProviderPushError>>,
    before_revisions: Option<nmp_native_runtime_app::SectionRevisions>,
    after_revisions: Option<nmp_native_runtime_app::SectionRevisions>,
}

#[given("an installed napplet mixes a managed setting with a user permission")]
fn given_mixed_managed_review(world: &mut RuntimeWorld) {
    let rig = Rig::new(false);
    let exact = principal('1');
    let managed = nmp_native_runtime_core::Capability::new("managed-setting").unwrap();
    rig.install_with_requests(
        exact.clone(),
        vec![
            request(canary(), CapabilityRequirement::Required),
            request(managed.clone(), CapabilityRequirement::Optional),
        ],
    );
    rig.store
        .set_grant(&exact, &managed, GrantDecision::Managed)
        .unwrap();
    world.review = Some(rig.app.permission_review(&exact).unwrap());
    world.principal = Some(exact);
    world.rig = Some(rig);
}

#[when("the caller allows only the user permission against that review")]
fn when_user_change_applied(world: &mut RuntimeWorld) {
    let principal = world.principal();
    let review = world.review.as_ref().unwrap();
    world.permission_result = Some(world.rig().app.apply_permission_changes(
        PermissionChangeRequest {
            principal,
            review_revision: review.revision.clone(),
            decisions: vec![permission(canary(), GrantDecision::AllowExactBuild)],
        },
    ));
}

#[then("the user permission is allowed")]
fn then_user_permission_allowed(world: &mut RuntimeWorld) {
    assert!(world.permission_result.as_ref().unwrap().is_ok());
    assert_eq!(
        world
            .rig()
            .store
            .grant(&world.principal(), &canary())
            .unwrap(),
        GrantDecision::AllowExactBuild
    );
}

#[then("the managed setting remains managed")]
fn then_managed_setting_remains(world: &mut RuntimeWorld) {
    let managed = world
        .review
        .as_ref()
        .unwrap()
        .capabilities
        .iter()
        .find(|capability| capability.current_decision == GrantDecision::Managed)
        .unwrap();
    assert_eq!(
        world
            .rig()
            .store
            .grant(&world.principal(), &managed.capability)
            .unwrap(),
        GrantDecision::Managed
    );
}

#[given("an installed napplet has only a managed permission")]
fn given_all_managed_review(world: &mut RuntimeWorld) {
    let rig = Rig::new(false);
    let exact = principal('2');
    rig.install_with_requests(
        exact.clone(),
        vec![request(canary(), CapabilityRequirement::Required)],
    );
    rig.store
        .set_grant(&exact, &canary(), GrantDecision::Managed)
        .unwrap();
    world.review = Some(rig.app.permission_review(&exact).unwrap());
    world.principal = Some(exact);
    world.rig = Some(rig);
}

#[when("the caller submits no permission changes")]
fn when_no_changes_submitted(world: &mut RuntimeWorld) {
    let review = world.review.as_ref().unwrap();
    world.permission_result = Some(world.rig().app.apply_permission_changes(
        PermissionChangeRequest {
            principal: world.principal(),
            review_revision: review.revision.clone(),
            decisions: Vec::new(),
        },
    ));
}

#[then("the permission change is refused as empty")]
fn then_empty_change_refused(world: &mut RuntimeWorld) {
    assert_eq!(
        world
            .permission_result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .code,
        PermissionChangeRefusalCode::EmptyChanges
    );
}

#[given("an installed napplet has two user permissions under review")]
fn given_two_user_permissions(world: &mut RuntimeWorld) {
    let rig = Rig::new(false);
    let exact = principal('3');
    let second = nmp_native_runtime_core::Capability::new("second").unwrap();
    rig.install_with_requests(
        exact.clone(),
        vec![
            request(canary(), CapabilityRequirement::Required),
            request(second, CapabilityRequirement::Optional),
        ],
    );
    world.review = Some(rig.app.permission_review(&exact).unwrap());
    world.principal = Some(exact);
    world.rig = Some(rig);
}

#[when("host policy takes control before the caller applies one permission")]
fn when_policy_changes_before_apply(world: &mut RuntimeWorld) {
    let principal = world.principal();
    let second = nmp_native_runtime_core::Capability::new("second").unwrap();
    world.rig().app.dispatch(PlatformCommand::SetGrant {
        principal: principal.clone(),
        capability: second,
        sensitivity: Sensitivity::Sensitive,
        decision: GrantDecision::Managed,
    });
    world.permission_result = Some(world.rig().app.apply_permission_changes(
        PermissionChangeRequest {
            principal,
            review_revision: world.review.as_ref().unwrap().revision.clone(),
            decisions: vec![permission(canary(), GrantDecision::AllowExactBuild)],
        },
    ));
}

#[then("the permission change is refused as stale")]
fn then_stale_change_refused(world: &mut RuntimeWorld) {
    assert_eq!(
        world
            .permission_result
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .code,
        PermissionChangeRefusalCode::StaleReview
    );
}

#[then("no user permission change was applied")]
fn then_no_user_change(world: &mut RuntimeWorld) {
    assert_eq!(
        world
            .rig()
            .store
            .grant(&world.principal(), &canary())
            .unwrap(),
        GrantDecision::Denied
    );
}

impl RuntimeWorld {
    fn rig(&self) -> &Rig {
        self.rig
            .as_ref()
            .expect("Given step must open the runtime app first")
    }

    fn principal(&self) -> Principal {
        self.principal
            .clone()
            .expect("Given step must install a napplet first")
    }
}

// ---- Permission review is exact-build bounded ----

#[given(
    regex = "^an installed napplet that requires the \"canary\" capability and optionally \
             requests the \"missing\" capability$"
)]
fn given_installed_napplet_with_requests(world: &mut RuntimeWorld) {
    let rig = Rig::new(false);
    let exact = principal('b');
    rig.install_with_requests(
        exact.clone(),
        vec![
            request(canary(), CapabilityRequirement::Required),
            request(
                nmp_native_runtime_core::Capability::new("missing").unwrap(),
                CapabilityRequirement::Optional,
            ),
        ],
    );
    world.principal = Some(exact);
    world.rig = Some(rig);
}

#[when("the caller requests a permission review for the installed napplet")]
fn when_review_requested(world: &mut RuntimeWorld) {
    let principal = world.principal();
    world.review = Some(world.rig().app.permission_review(&principal).unwrap());
}

#[then("the review is scoped to that exact build's principal")]
fn then_review_scoped(world: &mut RuntimeWorld) {
    let principal = world.principal();
    assert_eq!(world.review.as_ref().unwrap().principal, principal);
}

#[then(regex = "^the review lists exactly (\\d+) capabilities$")]
fn then_lists_n_capabilities(world: &mut RuntimeWorld, count: usize) {
    assert_eq!(world.review.as_ref().unwrap().capabilities.len(), count);
}

#[then("the \"canary\" capability is available with ordinary sensitivity")]
fn then_canary_available_ordinary(world: &mut RuntimeWorld) {
    let review = world.review.as_ref().unwrap();
    let capability = &review.capabilities[0];
    assert_eq!(capability.capability, canary());
    assert_eq!(
        capability.platform_availability,
        PermissionPlatformAvailability::Available
    );
    assert_eq!(capability.sensitivity, Some(Sensitivity::Ordinary));
}

#[then("the \"missing\" capability has unknown platform availability")]
fn then_missing_unknown_availability(world: &mut RuntimeWorld) {
    let review = world.review.as_ref().unwrap();
    assert_eq!(
        review.capabilities[1].platform_availability,
        PermissionPlatformAvailability::Unknown {
            reason: std::sync::Arc::from(
                "no provider metadata is registered for this capability on this runtime"
            )
        }
    );
}

#[then("the \"missing\" capability defaults to a denied decision")]
fn then_missing_defaults_denied(world: &mut RuntimeWorld) {
    let review = world.review.as_ref().unwrap();
    assert_eq!(
        review.capabilities[1].requested_decision,
        Some(GrantDecision::Denied)
    );
    assert!(
        review.capabilities[1]
            .decision_options
            .iter()
            .all(|option| option.decision == GrantDecision::Denied || !option.valid)
    );
}

#[given("the caller has denied every requested capability in one batch")]
fn given_denied_batch(world: &mut RuntimeWorld) {
    let principal = world.principal();
    world
        .rig()
        .app
        .dispatch(PlatformCommand::ApplyPermissionChanges(permission_changes(
            &world.rig().app,
            principal,
            vec![
                permission(canary(), GrantDecision::Denied),
                permission(
                    nmp_native_runtime_core::Capability::new("missing").unwrap(),
                    GrantDecision::Denied,
                ),
            ],
        )));
}

#[when("the caller attempts to launch the installed napplet")]
fn when_attempt_launch(world: &mut RuntimeWorld) {
    let principal = world.principal();
    world.rig().app.dispatch(PlatformCommand::Launch {
        principal,
        profile: nmp_native_runtime_core::ExecutionProfile::Legacy,
        required_domains: BTreeSet::from([canary()]),
    });
}

#[then("no session is admitted")]
fn then_no_session_admitted(world: &mut RuntimeWorld) {
    assert!(world.rig().app.snapshot().sessions.is_empty());
}

#[then("the most recent runtime error is a bridge refusal")]
fn then_bridge_refusal(world: &mut RuntimeWorld) {
    assert_eq!(
        world
            .rig()
            .app
            .snapshot()
            .recent_errors
            .last()
            .unwrap()
            .code,
        AppErrorCode::Bridge
    );
}

// ---- Provider push authority is fail-closed ----

#[given("an installed and running napplet with a live provider push sender")]
fn given_running_napplet_with_sender(world: &mut RuntimeWorld) {
    let rig = Rig::new(false);
    let exact = principal('b');
    rig.install(exact.clone());
    rig.allow_runtime(exact.clone());
    let session = rig.launch(exact.clone());
    let sender = rig.provider.sender(session);
    world.principal = Some(exact);
    world.session = Some(session);
    world.sender = Some(sender);
    world.rig = Some(rig);
}

#[when("the provider pushes \"canary.state\" with a spoofed principal field")]
fn when_push_spoofed_principal(world: &mut RuntimeWorld) {
    let principal = world.principal();
    let sender = world.sender.clone().unwrap();
    world.push_result = Some(sender.push(
        "canary.state",
        serde_json::Map::from_iter([("principal".to_owned(), serde_json::json!(principal))]),
        None,
    ));
}

#[when("the provider pushes \"other.state\" with no payload")]
fn when_push_wrong_domain(world: &mut RuntimeWorld) {
    let sender = world.sender.clone().unwrap();
    world.push_result = Some(sender.push("other.state", serde_json::Map::new(), None));
}

#[then("the push is refused with an authority-field error")]
fn then_authority_field_error(world: &mut RuntimeWorld) {
    assert_eq!(
        world.push_result.take(),
        Some(Err(ProviderPushError::AuthorityField))
    );
}

#[then("the push is refused with a domain-mismatch error")]
fn then_domain_mismatch_error(world: &mut RuntimeWorld) {
    assert_eq!(
        world.push_result.take(),
        Some(Err(ProviderPushError::DomainMismatch))
    );
}

#[given("the session has become ready")]
fn given_session_ready(world: &mut RuntimeWorld) {
    let session = world.session.unwrap();
    world.rig().ready(session);
}

#[when("the capability is revoked")]
fn when_capability_revoked(world: &mut RuntimeWorld) {
    let principal = world.principal();
    world.rig().app.dispatch(PlatformCommand::Revoke {
        principal,
        capability: canary(),
    });
}

#[then("a further push on that domain is refused as revoked")]
fn then_push_refused_revoked(world: &mut RuntimeWorld) {
    let sender = world.sender.clone().unwrap();
    assert_eq!(
        sender.push("canary.state", serde_json::Map::new(), None),
        Err(ProviderPushError::Revoked)
    );
}

#[then("the provider observed exactly one revocation for that session")]
fn then_one_revocation_observed(world: &mut RuntimeWorld) {
    let session = world.session.unwrap();
    let revoked = world.rig().provider.revoked.lock();
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0].session, session);
}

#[tokio::main]
async fn main() {
    RuntimeWorld::run("tests/features").await;
}
