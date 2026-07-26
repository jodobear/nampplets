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
    let update = profile.native.applyPermissionDecisions(
        NativeRuntimePermissionDecisionBatch(
            coordinate: installed.permissionCoordinate,
            // Decide on provider availability, not on requirement. The
            // fixture declares every domain it wants as required, and a
            // domain the runtime registers no provider for can only ever be
            // denied -- `permission_decision_policy` invalidates every other
            // option for it, and launch drops it rather than injecting it.
            decisions: review.capabilities.map { capability in
                let available: Bool
                if case .available = capability.platformAvailability {
                    available = true
                } else {
                    available = false
                }
                return NativeRuntimePermissionDecisionSelection(
                    domain: capability.domain,
                    decision: available ? .allowExactBuild : .denied
                )
            }
        )
    )
    guard update.applied, update.refusal == nil else {
        throw GoodMorningTestSupportError.permissionRefused
    }
    return try profile.native.launchInstalled(installed)
}
