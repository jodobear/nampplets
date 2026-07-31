import Foundation

extension RuntimeWorkbenchCatalogClient {
    public func search(
        _ request: CatalogSearchRequest
    ) async -> CatalogSearchResponse {
        if let profileBacking {
            return await profileBacking.browseCatalog(request)
        }
        if let loadIssue {
            return .unavailable(loadIssue)
        }

        let query = request.query
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        let matches = query.isEmpty
            ? records
            : records.filter { $0.searchText.contains(query) }
        let entries = matches.map(\.entry)
        guard
            let evidence = CatalogBrowseEvidence(
                scope: .offlineFixture,
                queryWasLocalFilter: !query.isEmpty,
                locallyFilteredRows: UInt(records.count - matches.count),
                projectedRows: UInt(entries.count),
                projectionLimitedRows: 0,
                refusedRows: 0,
                window: .idle,
                sourceEvidence: [],
                shortfalls: []
            ),
            let page = CatalogSearchPage(
                entries: entries,
                hasMore: false,
                evidence: evidence
            )
        else {
            return .unavailable(
                CatalogIssue(
                    title: "Offline fixture is outside UI limits",
                    message: "The bundled offline projection exceeded its "
                        + "finite page limit and was not displayed."
                )
            )
        }
        return .page(page)
    }

    public func observeChanges(
        _ receive: @escaping @MainActor @Sendable () -> Void
    ) -> CatalogFeedObservation {
        profileBacking?.observeCatalogChanges(receive)
            ?? CatalogFeedObservation()
    }

    static func bundledIndexData(
        named name: String,
        bundle: Bundle = .module
    ) throws -> Data {
        if let nested = bundle.url(
            forResource: "\(name)-index",
            withExtension: "json",
            subdirectory: "Catalog"
        ) {
            return try Data(contentsOf: nested)
        }
        if let flattened = bundle.url(
            forResource: "\(name)-index",
            withExtension: "json"
        ) {
            return try Data(contentsOf: flattened)
        }
        throw CatalogResourceError.missing("\(name)-index.json")
    }
}
