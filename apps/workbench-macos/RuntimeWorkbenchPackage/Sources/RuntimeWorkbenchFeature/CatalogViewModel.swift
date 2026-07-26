import Observation

@MainActor
@Observable
public final class CatalogViewModel {
    public var query = ""
    public var manualCoordinate = ""

    public private(set) var entries: [CatalogEntry] = []
    public private(set) var hasMore = false
    public private(set) var evidence: CatalogBrowseEvidence?
    public private(set) var review: CatalogInstallReview?
    public private(set) var installedBuild: CatalogInstalledBuild?
    var browseIssue: CatalogIssueNotice.Presentation?
    var operationIssue: CatalogIssueNotice.Presentation?
    var presentedIssue: CatalogIssueNotice.Presentation? {
        operationIssue ?? browseIssue
    }
    var installIssuePresentation: CatalogIssueNotice.Presentation? {
        operationIssue?.intent == .installBlocked ? operationIssue : nil
    }
    public var issue: CatalogIssue? { presentedIssue?.issue }
    public internal(set) var isResolvingReview = false
    public internal(set) var isInstalling = false
    public internal(set) var feedGenerationExhaustion:
        CatalogRequestGenerationExhaustion?
    public internal(set) var operationGenerationExhaustion:
        CatalogRequestGenerationExhaustion?

    /// Live profiles expose a connecting replacement before the first relay
    /// frame arrives, so the UI never turns a permanent subscription into an
    /// empty or missing catalog surface.
    public var connectingEvidence: CatalogBrowseEvidence? {
        guard client.feedScope == .liveNMPWindow, evidence == nil else {
            return nil
        }
        return CatalogBrowseEvidence(
            scope: .liveNMPWindow,
            queryWasLocalFilter: !query.isEmpty,
            locallyFilteredRows: 0,
            projectedRows: 0,
            projectionLimitedRows: 0,
            refusedRows: 0,
            window: .requesting,
            sourceEvidence: [],
            shortfalls: []
        )
    }

    let client: any CatalogClient
    private let onInstalled: @MainActor (CatalogInstalledBuild) -> Void
    var operationGeneration: CatalogRequestGenerationCounter
    var feedGeneration: CatalogRequestGenerationCounter
    var feedObservation: CatalogFeedObservation?
    private var started = false

    public init(
        client: any CatalogClient,
        onInstalled: @escaping @MainActor (CatalogInstalledBuild) -> Void = {
            _ in
        }
    ) {
        self.client = client
        self.onInstalled = onInstalled
        operationGeneration = CatalogRequestGenerationCounter(
            lane: .transientOperation
        )
        feedGeneration = CatalogRequestGenerationCounter(lane: .feed)
    }

    init(
        client: any CatalogClient,
        feedGenerationStart: UInt,
        operationGenerationStart: UInt,
        onInstalled: @escaping @MainActor (CatalogInstalledBuild) -> Void = {
            _ in
        }
    ) {
        self.client = client
        self.onInstalled = onInstalled
        operationGeneration = CatalogRequestGenerationCounter(
            lane: .transientOperation,
            current: operationGenerationStart
        )
        feedGeneration = CatalogRequestGenerationCounter(
            lane: .feed,
            current: feedGenerationStart
        )
    }

    /// Attaches to the profile-owned permanent feed and renders its latest
    /// bounded replacement immediately.
    public func start() async {
        guard !started else {
            return
        }
        started = true
        if client.feedScope == .liveNMPWindow, evidence == nil {
            evidence = CatalogBrowseEvidence(
                scope: .liveNMPWindow,
                queryWasLocalFilter: !query.isEmpty,
                locallyFilteredRows: 0,
                projectedRows: 0,
                projectionLimitedRows: 0,
                refusedRows: 0,
                window: .requesting,
                sourceEvidence: [],
                shortfalls: []
            )
        }
        feedObservation = client.observeChanges { [weak self] in
            guard let self else {
                return
            }
            Task { @MainActor in
                await self.refreshFeed()
            }
        }
        await refreshFeed()
    }

    /// Stops only this view's bounded native fanout. The profile-owned NMP
    /// subscription remains open until the profile closes.
    public func stop() {
        if feedGeneration.issue() == nil {
            recordFeedGenerationExhaustion()
        }
        feedObservation?.cancel()
        feedObservation = nil
        cancelTransientWork()
    }

    public func search() async {
        await refreshFeed()
    }

    private func refreshFeed() async {
        guard let request = CatalogSearchRequest(query: query) else {
            // These two are the shell's own words, not a projected refusal,
            // so they say what to do rather than quoting a byte ceiling.
            browseIssue = CatalogIssueNotice.Presentation(
                issue: CatalogIssue(
                    title: "Search is too long",
                    message: "Try a shorter search."
                ),
                intent: entries.isEmpty ? .browseBlocked : .browsePartial
            )
            return
        }

        guard let generation = feedGeneration.issue() else {
            recordFeedGenerationExhaustion()
            return
        }
        let response = await client.search(request)
        guard feedGeneration.isCurrent(generation) else {
            return
        }

        switch response {
        case let .page(page):
            entries = page.entries
            hasMore = page.hasMore
            evidence = page.evidence
            browseIssue = nil
        case let .unavailable(problem):
            entries = []
            hasMore = false
            evidence = nil
            browseIssue = CatalogIssueNotice.Presentation(
                issue: problem,
                intent: .browseBlocked
            )
        }
    }

    public func review(entry: CatalogEntry) async {
        await resolveReview(.entryID(entry.id))
    }

    public func reviewManualCoordinate() async {
        cancelTransientWork()
        guard let request = CatalogManualCoordinateRequest(
            coordinate: manualCoordinate
        ) else {
            operationIssue = CatalogIssueNotice.Presentation(
                issue: CatalogIssue(
                    title: "That address doesn't look right",
                    message: "Check you copied the whole thing."
                ),
                intent: .resolveBlocked
            )
            return
        }
        await resolveReview(
            .manualCoordinate(request),
            cancelCurrentWork: false
        )
    }

    public func cancelReview() {
        cancelTransientWork()
        review = nil
        operationIssue = nil
    }

    @discardableResult
    public func confirmInstall() async -> CatalogInstalledBuild? {
        guard let review, review.canInstall, !isInstalling else {
            return nil
        }

        guard let generation = beginTransientOperation() else {
            return nil
        }
        isInstalling = true
        operationIssue = nil
        let response = await client.confirmExactVerifiedInstall(
            CatalogInstallConfirmation(review: review)
        )
        guard operationGeneration.isCurrent(generation) else {
            return nil
        }
        isInstalling = false

        switch response {
        case let .installed(build):
            installedBuild = build
            self.review = nil
            onInstalled(build)
            return build
        case let .refused(problem):
            operationIssue = CatalogIssueNotice.Presentation(
                issue: problem,
                intent: .installBlocked
            )
            return nil
        }
    }

    private func resolveReview(
        _ target: CatalogReviewTarget,
        cancelCurrentWork: Bool = true
    ) async {
        if cancelCurrentWork {
            cancelTransientWork()
        }
        guard let generation = beginTransientOperation() else {
            return
        }
        isResolvingReview = true
        operationIssue = nil
        let response = await client.resolveReview(target)
        guard operationGeneration.isCurrent(generation) else {
            return
        }
        isResolvingReview = false

        switch response {
        case let .ready(review):
            self.review = review
        case let .unavailable(problem):
            review = nil
            operationIssue = CatalogIssueNotice.Presentation(
                issue: problem,
                intent: .resolveBlocked
            )
        }
    }

    private func cancelTransientWork() {
        if operationGeneration.issue() == nil {
            recordOperationGenerationExhaustion()
        }
        isResolvingReview = false
        isInstalling = false
        client.cancelPendingCatalogWork()
    }
}
