@testable import RuntimeWorkbenchFeature
import Testing

@MainActor
@Test func cancelDiscardsFormStateWithoutCallingPermissionManager() {
    let manager = RecordingPermissionManager(snapshot: permissionSnapshot())
    let model = PermissionReviewSheetModel(manager: manager)
    let identity = model.review.capabilities[0]

    model.select(.allowExactBuild, for: identity)
    #expect(model.selection(for: identity) == .allowExactBuild)

    model.cancel()

    #expect(model.selection(for: identity) == identity.requestedDecision)
    #expect(manager.submissions.isEmpty)
}

@MainActor
@Test func confirmationSendsOneExactBuildScopedBatchAndNeverLaunches() async {
    let initial = permissionSnapshot()
    let manager = RecordingPermissionManager(snapshot: initial)
    manager.response = PermissionReviewSnapshot(
        review: initial.review,
        submissionState: .applied
    )
    let model = PermissionReviewSheetModel(manager: manager)

    model.select(.allowExactBuild, for: model.review.capabilities[0])
    model.select(.allowSession, for: model.review.capabilities[1])
    await model.confirm()

    #expect(manager.submissions.count == 1)
    #expect(manager.submissions.first?.principal == initial.review.principal)
    #expect(
        manager.submissions.first?.decisions == [
            PermissionDecisionSelection(
                domain: "identity",
                decision: .allowExactBuild
            )!,
            PermissionDecisionSelection(
                domain: "outbox",
                decision: .allowSession
            )!,
        ]
    )
    #expect(manager.actions == [.submit])
    #expect(model.isApplied)
}

@MainActor
@Test func selectAllRecommendedUsesRustsValidDecisionAndConfirmLaunches() async {
    let initial = permissionSnapshot()
    let manager = RecordingPermissionManager(snapshot: initial)
    manager.response = PermissionReviewSnapshot(
        review: initial.review,
        submissionState: .applied
    )
    let model = PermissionReviewSheetModel(manager: manager)

    // The Rust-requested default on every capability here is `.askEveryTime`,
    // which can never satisfy launch -- confirming it un-edited previously
    // looked like it worked and then silently failed to launch the napplet.
    #expect(model.review.capabilities.allSatisfy { $0.requestedDecision == .askEveryTime })

    model.selectAllRecommended()

    #expect(model.selection(for: model.review.capabilities[0]) == .allowExactBuild)
    #expect(model.selection(for: model.review.capabilities[1]) == .allowExactBuild)

    await model.confirm()

    #expect(manager.submissions.count == 1)
    #expect(
        (manager.submissions.first?.decisions ?? [])
            .sorted { $0.domain < $1.domain } == [
                PermissionDecisionSelection(domain: "identity", decision: .allowExactBuild)!,
                PermissionDecisionSelection(domain: "outbox", decision: .allowExactBuild)!,
            ]
    )
    #expect(model.isApplied)
}

@MainActor
@Test func grantSwitchUsesRustsProjectedGrantAndRecommendation() {
    let model = PermissionReviewSheetModel(
        manager: RecordingPermissionManager(snapshot: permissionSnapshot())
    )
    let identity = model.review.capabilities[0]

    #expect(!model.isGranted(identity))
    #expect(model.hasAffirmativeOption(identity))

    model.setGranted(true, for: identity)
    #expect(model.selection(for: identity) == identity.recommendedDecision)
    #expect(model.isGranted(identity))

    model.setGranted(false, for: identity)
    #expect(model.selection(for: identity) == .deny)
    #expect(!model.isGranted(identity))
}

@MainActor
@Test func grantSwitchStaysDisabledWithoutRustAffirmativeRecommendation() {
    let model = PermissionReviewSheetModel(
        manager: RecordingPermissionManager(
            snapshot: unavailablePermissionSnapshot()
        )
    )

    #expect(!model.hasAffirmativeOption(model.review.capabilities[0]))
}

@MainActor
@Test func dependencyRefusalIsRenderedFromManagerOwnedErrorState() async {
    let initial = permissionSnapshot()
    let manager = RecordingPermissionManager(snapshot: initial)
    let refusal = PermissionReviewIssue(
        title: "Required dependency denied",
        message: "identity requires outbox for this verified build.",
        affectedDomains: ["identity", "outbox"]
    )!
    manager.response = PermissionReviewSnapshot(
        review: initial.review,
        submissionState: .refused(refusal)
    )
    let model = PermissionReviewSheetModel(manager: manager)

    model.select(.allowExactBuild, for: model.review.capabilities[0])
    model.select(.deny, for: model.review.capabilities[1])
    await model.confirm()

    #expect(manager.submissions.count == 1)
    #expect(model.issue == refusal)
    #expect(!model.isApplied)
    #expect(model.canConfirm)
}

@MainActor
@Test func invalidPlatformDecisionIsRefusedBeforeSubmission() async {
    let initial = unavailablePermissionSnapshot()
    let manager = RecordingPermissionManager(snapshot: initial)
    let model = PermissionReviewSheetModel(manager: manager)
    let resource = model.review.capabilities[0]

    model.select(.allowExactBuild, for: resource)
    await model.confirm()

    #expect(model.selection(for: resource) == .deny)
    #expect(model.issue?.title == "Decision unavailable")
    #expect(manager.submissions.isEmpty)
}

@MainActor
@Test func confirmDismissesImmediatelyWhenNoCapabilitiesAreRequested() async {
    let initial = noCapabilitiesPermissionSnapshot()
    let manager = RecordingPermissionManager(snapshot: initial)
    let model = PermissionReviewSheetModel(manager: manager)

    #expect(model.review.capabilities.isEmpty)
    #expect(model.canConfirm)

    await model.confirm()

    #expect(manager.submissions.isEmpty)
    #expect(model.isApplied)
}

@MainActor
@Test func aReviewMixingManagedAndDecidableCapabilitiesStaysNonConfirmable()
    async
{
    let initial = mixedManagedPermissionSnapshot()
    let manager = RecordingPermissionManager(snapshot: initial)
    let model = PermissionReviewSheetModel(manager: manager)

    #expect(model.review.capabilities.count == 2)
    #expect(model.decidableCapabilities.map(\.domain) == ["outbox"])
    #expect(model.managedCapabilities.map(\.domain) == ["identity"])
    #expect(!model.canConfirm)

    await model.confirm()

    // Rust requires exact key-set equality while also refusing a user-supplied
    // managed decision. Swift must not fabricate a partial-batch workaround.
    #expect(manager.submissions.isEmpty)
    #expect(!model.isApplied)
    #expect(model.isManagedReviewBlocked)
}

@MainActor
@Test func allowingTheRecommendedChoiceSubmitsRustsOwnRecommendation() async {
    let initial = permissionSnapshot()
    let manager = RecordingPermissionManager(snapshot: initial)
    manager.response = PermissionReviewSnapshot(
        review: initial.review,
        submissionState: .applied
    )
    let model = PermissionReviewSheetModel(manager: manager)

    await model.allowRecommended()

    // Every submitted decision is the `recommendedDecision` Rust projected --
    // the sheet never invents or ranks one of its own.
    #expect(
        manager.submissions.first?.decisions == [
            PermissionDecisionSelection(
                domain: "identity",
                decision: .allowExactBuild
            )!,
            PermissionDecisionSelection(
                domain: "outbox",
                decision: .allowExactBuild
            )!,
        ]
    )
    #expect(model.isApplied)
}

@MainActor
@Test func allowingTheRecommendedChoiceNeverWidensBeyondAnOfferedOption()
    async
{
    // `resource` recommends `.deny` and offers nothing broader, so the one
    // gesture must leave it denied rather than reaching for a wider grant.
    let initial = unavailablePermissionSnapshot()
    let manager = RecordingPermissionManager(snapshot: initial)
    manager.response = PermissionReviewSnapshot(
        review: initial.review,
        submissionState: .applied
    )
    let model = PermissionReviewSheetModel(manager: manager)

    await model.allowRecommended()

    #expect(
        manager.submissions.first?.decisions == [
            PermissionDecisionSelection(domain: "resource", decision: .deny)!,
        ]
    )
}

@MainActor
@Test func sensitiveCapabilitiesArePresentedWhereTheyWillBeRead() {
    let manager = RecordingPermissionManager(
        snapshot: orderingPermissionSnapshot()
    )
    let model = PermissionReviewSheetModel(manager: manager)

    // Rust owns sensitivity and requirement; the sheet only orders by them.
    #expect(
        model.orderedCapabilities.map(\.domain) == ["outbox", "theme", "link"]
    )
    #expect(model.requiredCapabilities.map(\.domain) == ["outbox", "theme"])
    #expect(model.optionalCapabilities.map(\.domain) == ["link"])
}
