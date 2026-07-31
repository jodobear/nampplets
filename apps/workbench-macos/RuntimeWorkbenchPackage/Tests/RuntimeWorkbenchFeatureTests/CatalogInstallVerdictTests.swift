@testable import RuntimeWorkbenchFeature
import Testing

@Test func installCopyDoesNotClaimTheUserControlsManagedAccess() {
    #expect(
        CatalogInstallPlainCopy.optionalHeading
            == "The napplet lists these as optional"
    )
    #expect(
        CatalogInstallPlainCopy.reassurance
            == "Adding doesn't grant access or open the napplet. "
                + "Access is reviewed separately."
    )
    #expect(!CatalogInstallPlainCopy.reassurance.contains("You choose"))
}

@MainActor
@Test func installVerdictNeverLeaksRawRuntimeDiagnostics() {
    let diagnostic = "Only named manifests can mint an exact-build runtime principal."
    let blocked = sheet(
        warnings: [
            CatalogWarning(
                id: "missing-d-tag",
                severity: .blocking,
                message: diagnostic
            )
        ],
        canInstall: false
    )

    #expect(blocked.verdict == .blocked("This napplet can't be added right now."))
    #expect(blocked.verdict.message?.contains(diagnostic) == false)

    let incompatible = sheet(platformCompatibility: [
        CatalogPlatformCompatibility(
            id: "macos",
            platform: "macOS",
            status: .incompatible,
            detail: "native-runtime-compat-v2 rejected this exact aggregate"
        ),
    ])

    #expect(
        incompatible.verdict
            == .blocked("This napplet doesn't run on this device.")
    )
}

@MainActor
private func sheet(
    platformCompatibility: [CatalogPlatformCompatibility] = [],
    warnings: [CatalogWarning] = [],
    canInstall: Bool = true
) -> CatalogInstallReviewSheet {
    let author = String(repeating: "a", count: 64)
    let review = CatalogInstallReview(
        id: "review-1",
        title: "Good Morning",
        publisher: CatalogPublisher(displayName: "Alice", publicKey: author),
        coordinate: "35129:\(author):good-morning",
        exactAggregateHash: String(repeating: "b", count: 64),
        sources: [],
        requiredDomains: [],
        optionalDomains: [],
        platformCompatibility: platformCompatibility,
        warnings: warnings,
        updateRelationship: .firstInstall,
        canInstall: canInstall
    )!
    return CatalogInstallReviewSheet(
        review: review,
        isInstalling: false,
        issuePresentation: nil,
        onCancel: {},
        onConfirm: {}
    )
}
