import Observation

@MainActor
@Observable
public final class CatalogViewModel {
    public var query = ""
    public var manualCoordinate = ""

    public private(set) var entries: [CatalogEntry] = []
    public private(set) var hasMore = false
    public private(set) var review: CatalogInstallReview?
    public private(set) var installedBuild: CatalogInstalledBuild?
    public private(set) var issue: CatalogIssue?
    public private(set) var isSearching = false
    public private(set) var isResolvingReview = false
    public private(set) var isInstalling = false

    private let client: any CatalogClient
    private var operationGeneration: UInt = 0

    public init(client: any CatalogClient) {
        self.client = client
    }

    public func search() async {
        guard let request = CatalogSearchRequest(query: query) else {
            issue = CatalogIssue(
                title: "Search is too long",
                message: "Use at most \(CatalogLimits.maximumQueryUTF8Bytes) UTF-8 bytes."
            )
            return
        }

        cancelTransientWork()
        let generation = operationGeneration
        isSearching = true
        issue = nil
        let response = await client.search(request)
        guard generation == operationGeneration else {
            return
        }
        isSearching = false

        switch response {
        case let .page(page):
            entries = page.entries
            hasMore = page.hasMore
        case let .unavailable(problem):
            entries = []
            hasMore = false
            issue = problem
        }
    }

    public func review(entry: CatalogEntry) async {
        await resolveReview(.entryID(entry.id))
    }

    public func reviewManualCoordinate() async {
        guard let request = CatalogManualCoordinateRequest(
            coordinate: manualCoordinate
        ) else {
            issue = CatalogIssue(
                title: "Coordinate is invalid",
                message: "Enter a non-empty coordinate no larger than "
                    + "\(CatalogLimits.maximumCoordinateUTF8Bytes) UTF-8 bytes."
            )
            return
        }
        await resolveReview(.manualCoordinate(request))
    }

    public func cancelReview() {
        cancelTransientWork()
        review = nil
        issue = nil
    }

    public func confirmInstall() async {
        guard let review, review.canInstall, !isInstalling else {
            return
        }

        isInstalling = true
        issue = nil
        let response = await client.confirmExactVerifiedInstall(
            CatalogInstallConfirmation(review: review)
        )
        isInstalling = false

        switch response {
        case let .installed(build):
            installedBuild = build
            self.review = nil
        case let .refused(problem):
            issue = problem
        }
    }

    private func resolveReview(_ target: CatalogReviewTarget) async {
        cancelTransientWork()
        let generation = operationGeneration
        isResolvingReview = true
        issue = nil
        let response = await client.resolveReview(target)
        guard generation == operationGeneration else {
            return
        }
        isResolvingReview = false

        switch response {
        case let .ready(review):
            self.review = review
        case let .unavailable(problem):
            review = nil
            issue = problem
        }
    }

    private func cancelTransientWork() {
        operationGeneration &+= 1
        isSearching = false
        isResolvingReview = false
        client.cancelPendingCatalogWork()
    }
}
