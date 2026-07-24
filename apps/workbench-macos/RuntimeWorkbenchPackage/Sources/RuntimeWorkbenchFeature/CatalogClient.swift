@MainActor
public protocol CatalogClient: AnyObject {
    /// Searches only sources approved by the Rust runtime.
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
