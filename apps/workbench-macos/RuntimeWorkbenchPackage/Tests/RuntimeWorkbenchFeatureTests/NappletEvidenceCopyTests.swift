@testable import RuntimeWorkbenchFeature
import Testing

@Test func sharedEvidenceHintMakesNoUniversalProvenanceClaim() {
    let hint = NappletEvidenceCopy.accessibilityHint

    #expect(hint == "Shows the exact technical details behind this screen")
    #expect(!hint.localizedCaseInsensitiveContains("verified"))
    #expect(!hint.localizedCaseInsensitiveContains("runtime"))
}
