import Testing
@testable import RuntimeWorkbenchFeature

@MainActor
private final class RoutingCatalogClient: CatalogClient {
    var searchResponse: CatalogSearchResponse
    var reviewResponse: CatalogReviewResponse
    var installResponse: CatalogInstallResponse
    private(set) var reviewTargets: [CatalogReviewTarget] = []

    init(
        search: CatalogSearchResponse,
        review: CatalogReviewResponse,
        install: CatalogInstallResponse
    ) {
        searchResponse = search
        reviewResponse = review
        installResponse = install
    }

    func search(_: CatalogSearchRequest) async -> CatalogSearchResponse {
        searchResponse
    }

    func resolveReview(
        _ target: CatalogReviewTarget
    ) async -> CatalogReviewResponse {
        reviewTargets.append(target)
        return reviewResponse
    }

    func cancelPendingCatalogWork() {}

    func confirmExactVerifiedInstall(
        _: CatalogInstallConfirmation
    ) async -> CatalogInstallResponse {
        installResponse
    }
}

private let routingAuthor = String(repeating: "a", count: 64)
private let routingHash = String(repeating: "b", count: 64)
private let routingIssue = CatalogIssue(
    title: "Runtime profile unavailable",
    message: "Exact projection refused."
)

private func routingEntry() -> CatalogEntry {
    CatalogEntry(
        id: "entry",
        title: "Example",
        summary: "A napplet",
        publisher: CatalogPublisher(
            displayName: "Alice",
            publicKey: routingAuthor
        ),
        coordinate: "35129:\(routingAuthor):example",
        compatibility: .compatible
    )!
}

private func routingPage() -> CatalogSearchPage {
    CatalogSearchPage(
        entries: [routingEntry()],
        hasMore: false,
        evidence: CatalogBrowseEvidence(
            scope: .liveNMPWindow,
            queryWasLocalFilter: false,
            locallyFilteredRows: 0,
            projectedRows: 1,
            projectionLimitedRows: 0,
            refusedRows: 0,
            window: .returned(addedRows: 1),
            sourceEvidence: [],
            shortfalls: []
        )!
    )!
}

private func routingReview() -> CatalogInstallReview {
    CatalogInstallReview(
        id: "review",
        title: "Example",
        publisher: CatalogPublisher(
            displayName: "Alice",
            publicKey: routingAuthor
        ),
        coordinate: "35129:\(routingAuthor):example",
        exactAggregateHash: routingHash,
        sources: [],
        requiredDomains: [],
        optionalDomains: [],
        platformCompatibility: [],
        warnings: [],
        updateRelationship: .firstInstall,
        canInstall: true
    )!
}

@MainActor
private func routingClient() -> RoutingCatalogClient {
    RoutingCatalogClient(
        search: .page(routingPage()),
        review: .unavailable(routingIssue),
        install: .refused(routingIssue)
    )
}

@MainActor
@Test func invalidFilterRetainsRowsAndUsesPartialBrowseIntent() async {
    let client = routingClient()
    let model = CatalogViewModel(client: client)
    await model.start()

    model.query = String(
        repeating: "x",
        count: CatalogLimits.maximumQueryUTF8Bytes + 1
    )
    await model.search()

    #expect(model.entries == [routingEntry()])
    #expect(model.presentedIssue?.intent == .browsePartial)
    #expect(
        model.presentedIssue?.intent.verdict
            == .caution("Some napplets couldn't be loaded.")
    )
}

@MainActor
@Test func unavailableBrowseClearsRowsAndUsesBlockedIntent() async {
    let client = routingClient()
    client.searchResponse = .unavailable(routingIssue)
    let model = CatalogViewModel(client: client)
    await model.start()

    #expect(model.entries.isEmpty)
    #expect(model.presentedIssue?.intent == .browseBlocked)
    #expect(
        model.presentedIssue?.intent.verdict
            == .blocked("Couldn't load napplets just now.")
    )
}

@MainActor
@Test func EntryAndManualFailuresKeepResolveIntentAfterInputChanges() async {
    let client = routingClient()
    let model = CatalogViewModel(client: client)
    await model.start()

    model.manualCoordinate = "35129:\(routingAuthor):example"
    await model.reviewManualCoordinate()
    model.manualCoordinate = ""
    #expect(model.presentedIssue?.intent == .resolveBlocked)

    model.manualCoordinate = "unrelated-current-value"
    await model.review(entry: routingEntry())
    #expect(model.presentedIssue?.intent == .resolveBlocked)
    #expect(client.reviewTargets.count == 2)

    await model.search()
    #expect(model.presentedIssue?.intent == .resolveBlocked)
}

@MainActor
@Test func installRefusalKeepsReviewOpenAndUsesInstallIntent() async {
    let client = routingClient()
    client.reviewResponse = .ready(routingReview())
    let model = CatalogViewModel(client: client)
    await model.start()
    await model.review(entry: routingEntry())

    client.searchResponse = .unavailable(routingIssue)
    await model.search()
    #expect(model.browseIssue?.intent == .browseBlocked)
    #expect(model.installIssuePresentation == nil)

    let installed = await model.confirmInstall()

    #expect(installed == nil)
    #expect(model.review == routingReview())
    #expect(model.presentedIssue?.intent == .installBlocked)
    #expect(
        model.presentedIssue?.intent.verdict
            == .blocked("Couldn't add that napplet.")
    )

    model.cancelReview()
    #expect(model.presentedIssue?.intent == .browseBlocked)
}
