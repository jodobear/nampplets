@testable import RuntimeWorkbenchFeature
import Testing

@Test func unavailableSheetsKeepRawFailureOffThePlainPath() {
    let raw = "exact-build runtime profile projection refused: INC unavailable"
    let presentations: [WorkbenchUnavailablePresentation] = [
        .activity(detail: raw),
        .permission(detail: raw),
        .relays(detail: raw),
    ]

    for presentation in presentations {
        #expect(presentation.verdict == .blocked(presentation.message))
        #expect(!presentation.title.contains(raw))
        #expect(!presentation.message.contains(raw))
        #expect(presentation.evidenceFields.map(\.value) == [raw])
    }
}
