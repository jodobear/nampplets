import NMPNativeRuntime

/// Native names for the generated permission transaction boundary.
///
/// These remain exact projections of Rust-owned values. The Apple package does
/// not infer capability policy, apply grants one at a time, or expose launch as
/// part of the permission transaction.
public typealias NativeRuntimePermissionCoordinate =
    RuntimeExactBuildCoordinate
public typealias NativeRuntimePermissionReviewResult =
    RuntimePermissionReviewResult
public typealias NativeRuntimePermissionReviewSnapshot =
    RuntimePermissionReviewSnapshot
public typealias NativeRuntimePermissionCapabilitySnapshot =
    RuntimePermissionCapabilitySnapshot
public typealias NativeRuntimePermissionRequirement =
    RuntimePermissionRequirement
public typealias NativeRuntimePermissionSensitivity =
    RuntimePermissionSensitivity
public typealias NativeRuntimePermissionPlatformAvailability =
    RuntimePermissionPlatformAvailability
public typealias NativeRuntimePermissionExistingDecision =
    RuntimePermissionExistingDecision
public typealias NativeRuntimeGrantDecision =
    RuntimeGrantDecision
public typealias NativeRuntimePermissionDecisionOption =
    RuntimePermissionDecisionOption
public typealias NativeRuntimePermissionDecisionSelection =
    RuntimePermissionDecisionSelection
public typealias NativeRuntimePermissionDecisionBatch =
    RuntimePermissionDecisionBatch
public typealias NativeRuntimePermissionBatchUpdate =
    RuntimePermissionBatchUpdate
public typealias NativeRuntimePermissionRefusal = RuntimeRefusal

/// The two-operation native permission boundary for one runtime profile.
///
/// Review is read-only. Applying decisions commits one complete exact-build
/// batch atomically in Rust and never launches the napplet.
public protocol NativeRuntimePermissionManaging: AnyObject, Sendable {
    func permissionReview(
        for coordinate: NativeRuntimePermissionCoordinate
    ) -> NativeRuntimePermissionReviewResult

    func applyPermissionDecisions(
        _ batch: NativeRuntimePermissionDecisionBatch
    ) -> NativeRuntimePermissionBatchUpdate
}

extension NativeRuntimeProfile: NativeRuntimePermissionManaging {}
