import NMPNativeRuntimeApple
@testable import RuntimeWorkbenchFeature

enum GoodMorningTestSupportError: Error {
    case missingReview
    case permissionRefused
}

/// Exercises the production three-step boundary while keeping focused tests
/// concise. No test grants one capability at a time or launches as a side
/// effect of installation.
@MainActor
func installApproveAndLaunchGoodMorning(
    fixture: GoodMorningFixture,
    profile: WorkbenchRuntimeProfile
) throws -> NappletArtifact {
    let installed = try fixture.install(profile: profile)
    let result = profile.native.permissionReview(
        for: installed.permissionCoordinate
    )
    guard result.refusal == nil, let review = result.review else {
        throw GoodMorningTestSupportError.missingReview
    }
    // The published Good Morning fixture declares no `requires` tags, so
    // there is nothing to decide -- applying an empty decision batch is
    // itself refused by Rust. Only decide when the review has capabilities.
    if !review.capabilities.isEmpty {
        let update = profile.native.applyPermissionDecisions(
            NativeRuntimePermissionDecisionBatch(
                coordinate: installed.permissionCoordinate,
                decisions: review.capabilities.map {
                    NativeRuntimePermissionDecisionSelection(
                        domain: $0.domain,
                        decision: $0.requirement == .required
                            ? .allowExactBuild
                            : .denied
                    )
                }
            )
        )
        guard update.applied, update.refusal == nil else {
            throw GoodMorningTestSupportError.permissionRefused
        }
    }
    return try profile.native.launchInstalled(installed)
}
