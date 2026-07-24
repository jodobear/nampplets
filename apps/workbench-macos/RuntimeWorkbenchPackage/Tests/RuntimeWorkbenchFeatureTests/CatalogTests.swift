import Testing
@testable import RuntimeWorkbenchFeature

private let testManifestAuthor = String(repeating: "a", count: 64)
private let testAggregateHash = String(repeating: "b", count: 64)

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

@MainActor
private final class DeferredCatalogClient: CatalogClient {
    private var searchContinuations:
        [CheckedContinuation<CatalogSearchResponse, Never>?] = []
    private var searchCountWaiters:
        [(count: Int, continuation: CheckedContinuation<Void, Never>)] = []
    private(set) var cancellations = 0

    func search(_ request: CatalogSearchRequest) async -> CatalogSearchResponse {
        _ = request
        return await withCheckedContinuation { continuation in
            searchContinuations.append(continuation)
            signalSearchWaiters()
        }
    }

    func resolveReview(
        _ target: CatalogReviewTarget
    ) async -> CatalogReviewResponse {
        _ = target
        return .unavailable(
            CatalogIssue(title: "Unused", message: "Unused by this test.")
        )
    }

    func cancelPendingCatalogWork() {
        cancellations += 1
    }

    func confirmExactVerifiedInstall(
        _ confirmation: CatalogInstallConfirmation
    ) async -> CatalogInstallResponse {
        _ = confirmation
        return .refused(
            CatalogIssue(title: "Unused", message: "Unused by this test.")
        )
    }

    func waitForSearchCount(_ expectedCount: Int) async {
        guard searchContinuations.count < expectedCount else {
            return
        }
        await withCheckedContinuation { continuation in
            searchCountWaiters.append((expectedCount, continuation))
        }
    }

    func completeSearch(
        at index: Int,
        with response: CatalogSearchResponse
    ) {
        let continuation = searchContinuations[index]
        searchContinuations[index] = nil
        continuation?.resume(returning: response)
    }

    private func signalSearchWaiters() {
        let ready = searchCountWaiters.filter {
            searchContinuations.count >= $0.count
        }
        searchCountWaiters.removeAll {
            searchContinuations.count >= $0.count
        }
        ready.forEach { $0.continuation.resume() }
    }
}

private func catalogEntry(id: String = "gm") -> CatalogEntry {
    CatalogEntry(
        id: id,
        title: "Good Morning",
        summary: "A bounded inbox",
        publisher: CatalogPublisher(
            displayName: "Alice",
            publicKey: testManifestAuthor
        ),
        coordinate: "35129:\(testManifestAuthor):good-morning",
        compatibility: .compatible
    )!
}

private func installReview() -> CatalogInstallReview {
    CatalogInstallReview(
        id: "review-1",
        title: "Good Morning",
        publisher: CatalogPublisher(
            displayName: "Alice",
            publicKey: testManifestAuthor
        ),
        coordinate: "35129:\(testManifestAuthor):good-morning",
        exactAggregateHash: testAggregateHash,
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

private func liveEvidence(
    projectedRows: UInt = 1,
    locallyFilteredRows: UInt = 0
) -> CatalogBrowseEvidence {
    CatalogBrowseEvidence(
        scope: .liveNMPWindow,
        queryWasLocalFilter: locallyFilteredRows > 0,
        locallyFilteredRows: locallyFilteredRows,
        projectedRows: projectedRows,
        projectionLimitedRows: 0,
        refusedRows: 0,
        window: .returned(addedRows: UInt64(projectedRows)),
        sourceEvidence: [],
        shortfalls: []
    )!
}

@MainActor
private func fakeClient(review: CatalogInstallReview? = nil) -> FakeCatalogClient {
    let page = CatalogSearchPage(
        entries: [catalogEntry()],
        hasMore: false,
        evidence: liveEvidence()
    )!
    let review = review ?? installReview()
    return FakeCatalogClient(
        searchResponse: .page(page),
        reviewResponse: .ready(review),
        installResponse: .installed(
            CatalogInstalledBuild(
                title: review.title,
                manifestAuthor: review.publisher.publicKey,
                dTag: "good-morning",
                exactAggregateHash: review.exactAggregateHash
            )!
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
    #expect(
        CatalogSearchPage(
            entries: tooManyEntries,
            hasMore: false,
            evidence: liveEvidence(
                projectedRows: UInt(CatalogLimits.maximumEntriesPerPage)
            )
        ) == nil
    )
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
    #expect(model.evidence == liveEvidence())
    #expect(model.issue == nil)
}

@MainActor
@Test func openingCatalogStartsOneInitialBoundedBrowse() async {
    let client = fakeClient()
    let model = CatalogViewModel(client: client)

    await model.start()
    await model.start()

    #expect(client.searches == [CatalogSearchRequest(query: "")!])
    #expect(model.entries == [catalogEntry()])
}

@MainActor
@Test func manualCoordinateIsResolvedBeforeReview() async {
    let client = fakeClient()
    let model = CatalogViewModel(client: client)
    model.manualCoordinate = "35129:\(testManifestAuthor):good-morning"

    await model.reviewManualCoordinate()

    #expect(
        client.reviewTargets == [
            .manualCoordinate(
                CatalogManualCoordinateRequest(
                    coordinate: "35129:\(testManifestAuthor):good-morning"
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
                title: "Good Morning",
                manifestAuthor: testManifestAuthor,
                dTag: "good-morning",
                exactAggregateHash: testAggregateHash
            )
    )
    #expect(model.review == nil)
}

@MainActor
@Test func installedBuildIsDeliveredExactlyOnceToSheetCallback() async {
    let review = installReview()
    let expected = CatalogInstalledBuild(
        title: review.title,
        manifestAuthor: review.publisher.publicKey,
        dTag: "good-morning",
        exactAggregateHash: review.exactAggregateHash
    )!
    let client = fakeClient(review: review)
    var callbacks: [CatalogInstalledBuild] = []
    let model = CatalogViewModel(client: client) { build in
        callbacks.append(build)
    }

    await model.review(entry: catalogEntry())
    await model.confirmInstall()
    await model.confirmInstall()

    #expect(callbacks == [expected])
}

@MainActor
@Test func staleFeedReadCannotReplaceNewerProjectionOrCancelSubscription() async {
    let client = DeferredCatalogClient()
    let model = CatalogViewModel(client: client)

    model.query = "first"
    let first = Task { await model.search() }
    await client.waitForSearchCount(1)

    model.query = "second"
    let second = Task { await model.search() }
    await client.waitForSearchCount(2)

    client.completeSearch(
        at: 1,
        with: .page(
            CatalogSearchPage(
                entries: [catalogEntry(id: "second")],
                hasMore: false,
                evidence: liveEvidence()
            )!
        )
    )
    await second.value

    client.completeSearch(
        at: 0,
        with: .page(
            CatalogSearchPage(
                entries: [catalogEntry(id: "first")],
                hasMore: false,
                evidence: liveEvidence()
            )!
        )
    )
    await first.value

    #expect(model.entries.map(\.id) == ["second"])
    #expect(client.cancellations == 0)
}

@Test func liveCatalogProjectionRejectsUnboundedSourceEvidence() {
    let sources = (0 ... CatalogLimits.maximumBrowseSources).map { index in
        CatalogBrowseSourceEvidence(
            id: "source-\(index)",
            source: "wss://relay-\(index).example",
            access: .public,
            status: .requesting,
            reconciledThrough: nil
        )!
    }

    #expect(
        CatalogBrowseEvidence(
            scope: .liveNMPWindow,
            queryWasLocalFilter: false,
            locallyFilteredRows: 0,
            projectedRows: 0,
            projectionLimitedRows: 0,
            refusedRows: 0,
            window: .atBound(maximumRows: 512),
            sourceEvidence: sources,
            shortfalls: []
        ) == nil
    )
    #expect(
        CatalogBrowseSourceEvidence(
            id: "oversized",
            source: String(
                repeating: "x",
                count: CatalogLimits.maximumSourceLabelUTF8Bytes + 1
            ),
            access: .public,
            status: .error,
            reconciledThrough: nil
        ) == nil
    )
}

@MainActor
@Test func catalogSheetBuildsWithoutNetworkOrProductionFixtures() {
    let view = CatalogSheet(client: fakeClient())
    #expect(String(describing: type(of: view)) == "CatalogSheet")
}
