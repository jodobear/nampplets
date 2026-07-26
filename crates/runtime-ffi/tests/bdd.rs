//! Cucumber scenario runner for the public runtime-ffi facade.

mod support;

use cucumber::{World, given, then, when};
use nmp_native_runtime_ffi::{RuntimePermissionReviewSnapshot, RuntimeSnapshot};
use support::PermissionReviewRig;

#[derive(Debug, Default, World)]
struct RuntimeFfiWorld {
    rig: Option<PermissionReviewRig>,
    review: Option<RuntimePermissionReviewSnapshot>,
    snapshot: Option<RuntimeSnapshot>,
}

impl RuntimeFfiWorld {
    fn rig(&self) -> &PermissionReviewRig {
        self.rig
            .as_ref()
            .expect("Given step must prepare the published exact build")
    }
}

#[given("a verified published manifest with no signed requires tags")]
fn given_verified_manifest_without_requires(world: &mut RuntimeFfiWorld) {
    let rig = PermissionReviewRig::new();
    assert!(
        rig.has_no_signed_requirements(),
        "the immutable manifest must not gain synthetic requires tags"
    );
    world.rig = Some(rig);
}

#[given("its hash-matching entry document declares bounded napplet requirements")]
fn given_hash_matching_entry_requirements(world: &mut RuntimeFfiWorld) {
    assert_eq!(world.rig().embedded_domains().len(), 6);
}

#[when("the exact build is requested through the runtime FFI permission facade")]
fn when_permission_review_requested(world: &mut RuntimeFfiWorld) {
    world.review = Some(world.rig().permission_review());
}

#[then("the review contains exactly the authenticated normalized domains")]
fn then_review_contains_authenticated_domains(world: &mut RuntimeFfiWorld) {
    let mut actual = world
        .review
        .as_ref()
        .expect("When step must request a permission review")
        .capabilities
        .iter()
        .map(|capability| capability.domain.clone())
        .collect::<Vec<_>>();
    actual.sort();
    let mut expected = world.rig().embedded_domains().to_vec();
    expected.sort();
    assert_eq!(actual, expected);
}

#[then("the review principal is bound to manifest author, dTag, and aggregateHash")]
fn then_review_principal_is_exact(world: &mut RuntimeFfiWorld) {
    assert_eq!(
        &world
            .review
            .as_ref()
            .expect("When step must request a permission review")
            .coordinate,
        world.rig().coordinate()
    );
}

#[when("launch is attempted without granting the required domains")]
fn when_launch_attempted_without_grants(world: &mut RuntimeFfiWorld) {
    world.rig().launch_without_grants();
    world.snapshot = Some(world.rig().snapshot());
}

#[then("no session crosses the runtime FFI boundary")]
fn then_no_session_crosses_ffi(world: &mut RuntimeFfiWorld) {
    assert!(
        world
            .snapshot
            .as_ref()
            .expect("When step must attempt launch")
            .sessions
            .is_empty()
    );
}

#[then("the exact build receives typed bridge refusal evidence")]
fn then_exact_build_receives_refusal(world: &mut RuntimeFfiWorld) {
    let refusal = world
        .snapshot
        .as_ref()
        .expect("When step must attempt launch")
        .recent_errors
        .last()
        .expect("launch refusal evidence");
    let coordinate = world.rig().coordinate();
    assert_eq!(refusal.code, "bridge");
    assert_eq!(
        refusal.author.as_deref(),
        Some(coordinate.manifest_author.as_str())
    );
    assert_eq!(refusal.d_tag.as_deref(), Some(coordinate.d_tag.as_str()));
    assert_eq!(
        refusal.aggregate_hash.as_deref(),
        Some(coordinate.aggregate_hash.as_str())
    );
}

#[tokio::main]
async fn main() {
    RuntimeFfiWorld::run("tests/features/permission_review.feature").await;
}
