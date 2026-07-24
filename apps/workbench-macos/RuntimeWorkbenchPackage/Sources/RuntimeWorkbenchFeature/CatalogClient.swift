@MainActor
public final class CatalogFeedObservation {
    private var cancellation: (@Sendable () -> Void)?

    public init(cancellation: @escaping @Sendable () -> Void = {}) {
        self.cancellation = cancellation
    }

    public func cancel() {
        let cancellation = cancellation
        self.cancellation = nil
        cancellation?()
    }

    deinit {
        cancellation?()
    }
}

@MainActor
public protocol CatalogClient: AnyObject {
    /// Observes replacement changes from the profile-owned catalog feed.
    /// The callback is a redraw signal; `search` reads the latest bounded
    /// replacement and applies the current local filter without opening a new
    /// relay subscription.
    func observeChanges(
        _ receive: @escaping @MainActor @Sendable () -> Void
    ) -> CatalogFeedObservation

    /// Opens one bounded runtime-approved catalog projection.
    ///
    /// On the pinned NMP facade, a non-empty query is a local filter over the
    /// current finite window. It must not be presented as NIP-50 or a globally
    /// complete network search.
    func search(_ request: CatalogSearchRequest) async -> CatalogSearchResponse

    /// Resolves a result or coordinate and returns the Rust-owned verified review projection.
    func resolveReview(_ target: CatalogReviewTarget) async -> CatalogReviewResponse

    /// Cancels transient lookup work. It must not mutate installs, grants, or workspaces.
    func cancelPendingCatalogWork()

    /// Requests installation of the exact verified build represented by the review.
    ///
    /// Implementations must not launch the build or grant capabilities as a side effect.
    func confirmExactVerifiedInstall(
        _ confirmation: CatalogInstallConfirmation
    ) async -> CatalogInstallResponse
}

public extension CatalogClient {
    func observeChanges(
        _: @escaping @MainActor @Sendable () -> Void
    ) -> CatalogFeedObservation {
        CatalogFeedObservation()
    }
}
