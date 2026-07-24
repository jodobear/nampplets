import Testing
@testable import RuntimeWorkbenchFeature

@MainActor
private final class FakeCatalogClient: CatalogClient {
    var searchResponse: CatalogSearchResponse
    var reviewResponse: CatalogReviewResponse
    var installResponse: CatalogInstallResponse

    private(set) var searches: [CatalogSearchRequest] = []
    private(set) var reviewTargets: [CatalogReviewTarget] = []
    private(set) var cancellations = 0
    private(set) var confirmations: [CatalogInstallConfirmation] = []

    init(
        searchResponse: CatalogSearchResponse,
        reviewResponse: CatalogReviewResponse,
        installResponse: CatalogInstallResponse
    ) {
        self.searchResponse = searchResponse
        self.reviewResponse = reviewResponse
        self.installResponse = installResponse
    }

    func search(_ request: CatalogSearchRequest) async -> CatalogSearchResponse {
        searches.append(request)
        return searchResponse
    }

    func resolveReview(_ target: CatalogReviewTarget) async -> CatalogReviewResponse {
        reviewTargets.append(target)
        return reviewResponse
    }

    func cancelPendingCatalogWork() {
        cancellations += 1
    }

    func confirmExactVerifiedInstall(
        _ confirmation: CatalogInstallConfirmation
    ) async -> CatalogInstallResponse {
        confirmations.append(confirmation)
        return installResponse
    }
}

private func catalogEntry(id: String = "gm") -> CatalogEntry {
    CatalogEntry(
        id: id,
        title: "Good Morning",
        summary: "A bounded inbox",
        publisher: CatalogPublisher(
            displayName: "Alice",
            publicKey: "publisher-key"
        ),
        coordinate: "31990:publisher-key:good-morning",
        compatibility: .compatible
    )!
}

private func installReview() -> CatalogInstallReview {
    CatalogInstallReview(
        id: "review-1",
        title: "Good Morning",
        publisher: CatalogPublisher(
            displayName: "Alice",
            publicKey: "publisher-key"
        ),
        coordinate: "31990:publisher-key:good-morning",
        exactAggregateHash: "aggregate-hash",
        sources: [
            CatalogSourceProvenance(
                id: "manifest",
                kind: .manifestEvent,
                source: "wss://relay.example",
                evidence: "Signed manifest event"
            )
        ],
        requiredDomains: ["identity", "inc"],
        optionalDomains: ["theme"],
        platformCompatibility: [
            CatalogPlatformCompatibility(
                id: "macos",
                platform: "macOS",
                status: .compatible,
                detail: "All required providers are available"
            )
        ],
        warnings: [],
        updateRelationship: .firstInstall,
        canInstall: true
    )!
}

@MainActor
private func fakeClient(review: CatalogInstallReview? = nil) -> FakeCatalogClient {
    let page = CatalogSearchPage(entries: [catalogEntry()], hasMore: false)!
    let review = review ?? installReview()
    return FakeCatalogClient(
        searchResponse: .page(page),
        reviewResponse: .ready(review),
        installResponse: .installed(
            CatalogInstalledBuild(
                publisherPublicKey: review.publisher.publicKey,
                coordinate: review.coordinate,
                exactAggregateHash: review.exactAggregateHash
            )
        )
    )
}

@Test func catalogModelsRejectUnboundedRequestsAndPages() {
    let longQuery = String(
        repeating: "x",
        count: CatalogLimits.maximumQueryUTF8Bytes + 1
    )
    let longCoordinate = String(
        repeating: "x",
        count: CatalogLimits.maximumCoordinateUTF8Bytes + 1
    )
    let tooManyEntries = (0...CatalogLimits.maximumEntriesPerPage).map {
        catalogEntry(id: "\($0)")
    }
    let oversizedEntry = CatalogEntry(
        id: "oversized",
        title: String(
            repeating: "x",
            count: CatalogLimits.maximumFieldUTF8Bytes + 1
        ),
        summary: "",
        publisher: CatalogPublisher(
            displayName: nil,
            publicKey: "publisher-key"
        ),
        coordinate: "31990:publisher-key:oversized",
        compatibility: .compatible
    )
    let oversizedReview = CatalogInstallReview(
        id: "oversized",
        title: "Oversized",
        publisher: CatalogPublisher(
            displayName: nil,
            publicKey: "publisher-key"
        ),
        coordinate: "31990:publisher-key:oversized",
        exactAggregateHash: "aggregate-hash",
        sources: [],
        requiredDomains: Array(
            repeating: "identity",
            count: CatalogLimits.maximumDomainsPerClass + 1
        ),
        optionalDomains: [],
        platformCompatibility: [],
        warnings: [],
        updateRelationship: .firstInstall,
        canInstall: true
    )

    #expect(CatalogSearchRequest(query: longQuery) == nil)
    #expect(CatalogManualCoordinateRequest(coordinate: "") == nil)
    #expect(CatalogManualCoordinateRequest(coordinate: longCoordinate) == nil)
    #expect(oversizedEntry == nil)
    #expect(CatalogSearchPage(entries: tooManyEntries, hasMore: false) == nil)
    #expect(oversizedReview == nil)
}

@MainActor
@Test func searchDelegatesLiteralQueryAndPublishesBoundedResults() async {
    let client = fakeClient()
    let model = CatalogViewModel(client: client)
    model.query = "morning tools"

    await model.search()

    #expect(client.searches == [CatalogSearchRequest(query: "morning tools")!])
    #expect(model.entries == [catalogEntry()])
    #expect(!model.hasMore)
    #expect(model.issue == nil)
}

@MainActor
@Test func manualCoordinateIsResolvedBeforeReview() async {
    let client = fakeClient()
    let model = CatalogViewModel(client: client)
    model.manualCoordinate = "31990:publisher-key:good-morning"

    await model.reviewManualCoordinate()

    #expect(
        client.reviewTargets == [
            .manualCoordinate(
                CatalogManualCoordinateRequest(
                    coordinate: "31990:publisher-key:good-morning"
                )!
            )
        ]
    )
    #expect(model.review == installReview())
    #expect(client.confirmations.isEmpty)
}

@MainActor
@Test func cancelingReviewMutatesNoInstallState() async {
    let client = fakeClient()
    let model = CatalogViewModel(client: client)

    await model.review(entry: catalogEntry())
    model.cancelReview()

    #expect(model.review == nil)
    #expect(model.installedBuild == nil)
    #expect(client.confirmations.isEmpty)
    #expect(client.cancellations == 2)
}

@MainActor
@Test func confirmationPinsPublisherCoordinateAndExactVerifiedHash() async {
    let review = installReview()
    let client = fakeClient(review: review)
    let model = CatalogViewModel(client: client)

    await model.review(entry: catalogEntry())
    await model.confirmInstall()

    #expect(
        client.confirmations == [
            CatalogInstallConfirmation(review: review)
        ]
    )
    #expect(
        model.installedBuild
            == CatalogInstalledBuild(
                publisherPublicKey: "publisher-key",
                coordinate: "31990:publisher-key:good-morning",
                exactAggregateHash: "aggregate-hash"
            )
    )
    #expect(model.review == nil)
}

@MainActor
@Test func catalogSheetBuildsWithoutNetworkOrProductionFixtures() {
    let view = CatalogSheet(client: fakeClient())
    #expect(String(describing: type(of: view)) == "CatalogSheet")
}
