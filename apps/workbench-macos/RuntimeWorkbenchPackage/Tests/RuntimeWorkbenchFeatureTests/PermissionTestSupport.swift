@testable import RuntimeWorkbenchFeature

final class RecordingPermissionManager: PermissionReviewManaging {
    enum Action: Equatable {
        case submit
    }

    private var currentSnapshot: PermissionReviewSnapshot
    var response: PermissionReviewSnapshot?
    private(set) var submissions: [PermissionDecisionBatch] = []
    private(set) var actions: [Action] = []

    init(snapshot: PermissionReviewSnapshot) {
        currentSnapshot = snapshot
    }

    func snapshot() -> PermissionReviewSnapshot {
        currentSnapshot
    }

    func submit(_ batch: PermissionDecisionBatch) async {
        submissions.append(batch)
        actions.append(.submit)
        if let response {
            currentSnapshot = response
        }
    }
}

func permissionPrincipal(hash: Character = "b")
    -> PermissionExactBuildPrincipal
{
    PermissionExactBuildPrincipal(
        manifestAuthorPublicKey: String(repeating: "a", count: 64),
        dTag: "good-morning",
        aggregateHash: String(repeating: hash, count: 64)
    )!
}

func validOptions(
    unavailable: Set<PermissionRequestedDecision> = []
) -> [PermissionDecisionOption] {
    PermissionRequestedDecision.allCases.map { decision in
        if unavailable.contains(decision) {
            PermissionDecisionOption(
                decision: decision,
                isValid: false,
                invalidReason: "This decision is unavailable on the current platform."
            )!
        } else {
            PermissionDecisionOption(
                decision: decision,
                isValid: true
            )!
        }
    }
}

func permissionSnapshot() -> PermissionReviewSnapshot {
    let identity = PermissionCapabilityReview(
        domain: "identity",
        title: "Identity",
        requirement: .required,
        sensitivity: .sensitive,
        rationale: "Reads the active public key and follow list.",
        dependencies: [
            PermissionCapabilityDependency(
                domain: "outbox",
                reason: "Routes identity reads through author relay policy."
            )!
        ],
        platformAvailability: .available,
        existingDecision: .denied,
        isGranted: false,
        requestedDecision: .askEveryTime,
        recommendedDecision: .allowExactBuild,
        decisionOptions: validOptions()
    )!
    let outbox = PermissionCapabilityReview(
        domain: "outbox",
        title: "Outbox",
        requirement: .required,
        sensitivity: .sensitive,
        rationale: "Publishes approved replies through NMP.",
        dependencies: [],
        platformAvailability: .available,
        existingDecision: .askEveryTime,
        isGranted: false,
        requestedDecision: .askEveryTime,
        recommendedDecision: .allowExactBuild,
        decisionOptions: validOptions()
    )!
    let review = PermissionReview(
        principal: permissionPrincipal(),
        publisherDisplayName: "Alice",
        nappletTitle: "Good Morning",
        capabilities: [identity, outbox]
    )!
    return PermissionReviewSnapshot(review: review)
}

func noCapabilitiesPermissionSnapshot() -> PermissionReviewSnapshot {
    let review = PermissionReview(
        principal: permissionPrincipal(hash: "d"),
        publisherDisplayName: nil,
        nappletTitle: "Good Morning",
        capabilities: []
    )!
    return PermissionReviewSnapshot(review: review)
}

func mixedManagedPermissionSnapshot() -> PermissionReviewSnapshot {
    let managed = managedPermissionCapability(isGranted: true)
    let decidable = PermissionCapabilityReview(
        domain: "outbox",
        title: "Outbox",
        requirement: .required,
        sensitivity: .sensitive,
        rationale: "Publishes approved replies through NMP.",
        dependencies: [],
        platformAvailability: .available,
        existingDecision: .askEveryTime,
        isGranted: false,
        requestedDecision: .askEveryTime,
        recommendedDecision: .askEveryTime,
        decisionOptions: validOptions()
    )!
    let review = PermissionReview(
        principal: permissionPrincipal(hash: "e"),
        publisherDisplayName: "Alice",
        nappletTitle: "Good Morning",
        capabilities: [managed, decidable]
    )!
    return PermissionReviewSnapshot(review: review)
}

func managedPermissionCapability(
    isGranted: Bool
) -> PermissionCapabilityReview {
    PermissionCapabilityReview(
        domain: "identity",
        title: "Identity",
        requirement: .required,
        sensitivity: .sensitive,
        rationale: "Reads the active public key.",
        dependencies: [],
        platformAvailability: .available,
        existingDecision: .managed,
        isGranted: isGranted,
        requestedDecision: nil,
        recommendedDecision: nil,
        decisionOptions: PermissionRequestedDecision.allCases.map { decision in
            PermissionDecisionOption(
                decision: decision,
                isValid: false,
                invalidReason: "This capability is managed by host policy."
            )!
        }
    )!
}

func orderingPermissionSnapshot() -> PermissionReviewSnapshot {
    func capability(
        domain: String,
        requirement: PermissionCapabilityRequirement,
        sensitivity: PermissionCapabilitySensitivity
    ) -> PermissionCapabilityReview {
        PermissionCapabilityReview(
            domain: domain,
            title: domain.capitalized,
            requirement: requirement,
            sensitivity: sensitivity,
            rationale: "Rationale for \(domain).",
            dependencies: [],
            platformAvailability: .available,
            existingDecision: .askEveryTime,
            isGranted: false,
            requestedDecision: .askEveryTime,
            recommendedDecision: .askEveryTime,
            decisionOptions: validOptions()
        )!
    }
    let review = PermissionReview(
        principal: permissionPrincipal(hash: "f"),
        publisherDisplayName: "Alice",
        nappletTitle: "Good Morning",
        capabilities: [
            capability(
                domain: "link",
                requirement: .optional,
                sensitivity: .ordinary
            ),
            capability(
                domain: "theme",
                requirement: .required,
                sensitivity: .ordinary
            ),
            capability(
                domain: "outbox",
                requirement: .required,
                sensitivity: .sensitive
            ),
        ]
    )!
    return PermissionReviewSnapshot(review: review)
}

func unavailablePermissionSnapshot() -> PermissionReviewSnapshot {
    let resource = PermissionCapabilityReview(
        domain: "resource",
        title: "Resource",
        requirement: .optional,
        sensitivity: .ordinary,
        rationale: "Loads bounded avatar resources.",
        dependencies: [],
        platformAvailability: .unavailable(
            reason: "No native resource executor is installed."
        ),
        existingDecision: .denied,
        isGranted: false,
        requestedDecision: .deny,
        recommendedDecision: .deny,
        decisionOptions: validOptions(
            unavailable: [.askEveryTime, .allowSession, .allowExactBuild]
        )
    )!
    let review = PermissionReview(
        principal: permissionPrincipal(hash: "c"),
        publisherDisplayName: nil,
        nappletTitle: "Good Morning",
        capabilities: [resource]
    )!
    return PermissionReviewSnapshot(review: review)
}
