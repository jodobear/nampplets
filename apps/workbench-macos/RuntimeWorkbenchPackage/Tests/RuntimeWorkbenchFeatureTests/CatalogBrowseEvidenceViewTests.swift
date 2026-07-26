@testable import RuntimeWorkbenchFeature
import Testing

@MainActor
@Test func sourceShortfallNeverInventsAdditionalRows() {
    let view = CatalogBrowseEvidenceView(
        evidence: browseEvidence(
            projectedRows: 0,
            shortfalls: [.noPlannedSource]
        ),
        hasMore: false
    )

    #expect(view.summary == "0 napplets — some sources were unavailable")
    #expect(!view.summary.contains("more are available"))
}

@MainActor
@Test func displayBoundsAndHasMoreAreTheOnlyClaimsOfAdditionalRows() {
    let hasMore = CatalogBrowseEvidenceView(
        evidence: browseEvidence(projectedRows: 2),
        hasMore: true
    )
    let projectionBound = CatalogBrowseEvidenceView(
        evidence: browseEvidence(
            projectedRows: 2,
            projectionLimitedRows: 1
        ),
        hasMore: false
    )

    #expect(hasMore.summary == "2 napplets shown — more are available")
    #expect(projectionBound.summary == "2 napplets shown — more are available")
}

@MainActor
@Test func activeAcquisitionSaysOnlyThatItIsStillLooking() {
    let view = CatalogBrowseEvidenceView(
        evidence: browseEvidence(projectedRows: 1, window: .requesting),
        hasMore: false
    )

    #expect(view.summary == "1 napplet so far — still looking…")
}

private func browseEvidence(
    projectedRows: UInt,
    projectionLimitedRows: UInt = 0,
    window: CatalogBrowseWindowState = .returned(addedRows: 0),
    shortfalls: [CatalogBrowseShortfall] = []
) -> CatalogBrowseEvidence {
    CatalogBrowseEvidence(
        scope: .liveNMPWindow,
        queryWasLocalFilter: false,
        locallyFilteredRows: 0,
        projectedRows: projectedRows,
        projectionLimitedRows: projectionLimitedRows,
        refusedRows: 0,
        window: window,
        sourceEvidence: [],
        shortfalls: shortfalls
    )!
}
