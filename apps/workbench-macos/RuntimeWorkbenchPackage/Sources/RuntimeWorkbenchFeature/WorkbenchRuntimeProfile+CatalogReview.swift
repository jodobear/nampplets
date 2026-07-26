import NMPNativeRuntimeApple

@MainActor
extension WorkbenchRuntimeProfile {
    static func projectCatalogReview(
        _ review: NativeRuntimeCatalogReview
    ) -> CatalogInstallReview? {
        let lookupSources = review.provenance.enumerated().map {
            index,
            fact in
            let evidence: String
            switch fact.state {
            case let .observed(rows):
                evidence = "Observed \(rows) matching canonical row(s)."
            case let .shortfall(reason):
                evidence = "Source shortfall: \(reason)"
            case let .selected(eventID):
                evidence = "Selected exact signed event \(eventID)."
            }
            return CatalogSourceProvenance(
                id: "lookup-\(index)",
                kind: .manifestEvent,
                source: fact.source,
                evidence: evidence
            )
        }
        let blobSources = review.blobSources.enumerated().map {
            index,
            source in
            CatalogSourceProvenance(
                id: "artifact-\(index)",
                kind: .artifact,
                source: source,
                evidence: "HTTPS source declared by the exact signed manifest."
            )
        }
        let requiredDomains = review.capabilities.compactMap {
            $0.requirement == .required ? $0.domain : nil
        }
        let optionalDomains = review.capabilities.compactMap {
            $0.requirement == .optional ? $0.domain : nil
        }
        let eligibility = review.installEligibility
        return CatalogInstallReview(
            id: review.token,
            title: CatalogTitlePresentation.displayTitle(review.title),
            publisher: CatalogPublisher(
                displayName: nil,
                publicKey: review.manifestAuthor
            ),
            coordinate: review.coordinate,
            exactAggregateHash: review.aggregateHash,
            sources: lookupSources + blobSources,
            requiredDomains: requiredDomains,
            optionalDomains: optionalDomains,
            // Rust projects install eligibility and verified artifact identity,
            // not a platform-support verdict. Empty is the only truthful
            // native presentation until that typed verdict exists.
            platformCompatibility: [],
            warnings: WorkbenchCatalogInstallEligibility.warnings(for: eligibility),
            updateRelationship: .unknown(
                reason: "The exact installed-library relationship is resolved during installation."
            ),
            canInstall: eligibility.canInstall
        )
    }

    static func catalogIssue(
        _ failure: NativeRuntimeCatalogFailure
    ) -> CatalogIssue {
        CatalogIssue(
            title: failure.code == "cancelled"
                ? "Catalog operation cancelled"
                : "Catalog operation refused",
            message: failure.detail
        )
    }
}
