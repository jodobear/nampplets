@testable import RuntimeWorkbenchFeature
import Testing

/// The sentence a person reads when something fails must be the shell's own,
/// and it must not quote the runtime.
///
/// `CatalogIssue` is built at eighteen sites, most projecting a refusal
/// verbatim -- byte ceilings, "the application runtime profile is
/// unavailable", "manual manifest coordinate". Those are correct as evidence
/// and wrong as a verdict, which is the distinction
/// `docs/adr/0008-verdicts-on-the-path.md` exists to hold.
@Test func everyFailureContextHasAPlainSentenceOfItsOwn() {
    let forbidden = [
        "utf-8", "byte", "coordinate", "manifest", "projection", "profile",
        "exact build", "aggregate", "principal", "runtime", "facade",
    ]
    for intent: CatalogIssueNotice.Intent in [
        .browsePartial,
        .browseBlocked,
        .resolveBlocked,
        .installBlocked,
    ] {
        let sentence = intent.sentence
        #expect(!sentence.isEmpty)
        // Says what happened, not what a validator measured.
        #expect(sentence.hasSuffix("."))
        for term in forbidden {
            #expect(
                !sentence.lowercased().contains(term),
                "\(intent) leaks “\(term)” onto the plain path: \(sentence)"
            )
        }
    }
}

/// The three contexts must actually differ. A single generic apology reused
/// everywhere tells a person nothing about which of their actions failed.
@Test func failureContextsAreDistinguishable() {
    let sentences = Set(
        [
            CatalogIssueNotice.Intent.browsePartial,
            .resolveBlocked,
            .installBlocked,
        ]
            .map(\.sentence)
    )
    #expect(sentences.count == 3)
}

@Test func failureContextOwnsTruthfulSeverity() {
    #expect(
        CatalogIssueNotice.Intent.browsePartial.verdict
            == .caution("Some napplets couldn't be loaded.")
    )
    #expect(
        CatalogIssueNotice.Intent.browseBlocked.verdict
            == .blocked("Couldn't load napplets just now.")
    )
    #expect(
        CatalogIssueNotice.Intent.resolveBlocked.verdict
            == .blocked("Couldn't open that napplet.")
    )
    #expect(
        CatalogIssueNotice.Intent.installBlocked.verdict
            == .blocked("Couldn't add that napplet.")
    )
}

/// The runtime's own words survive. Relocating evidence is the rule; deleting
/// it is not, and a person who wants to know exactly what failed must still
/// be able to find out.
@MainActor
@Test func theRuntimesOwnWordsAreKeptForTheEvidenceTier() {
    let issue = CatalogIssue(
        title: "Coordinate is invalid",
        message: "Enter a non-empty coordinate no larger than 2048 UTF-8 bytes."
    )
    let notice = CatalogIssueNotice(
        presentation: CatalogIssueNotice.Presentation(
            issue: issue,
            intent: .resolveBlocked
        )
    )

    #expect(notice.presentation.issue.title == issue.title)
    #expect(notice.presentation.issue.message == issue.message)
}
